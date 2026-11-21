//! 特定のアプリが出している音の取り込み（macOS 14.2 以降の Core Audio プロセスタップ）。
//!
//! - Core Audio に音を出したことのあるプロセスを列挙し、所属する `.app` ごとにまとめる。
//!   ブラウザは別プロセス（Chrome なら「Google Chrome Helper」）から音を出すので、
//!   実行ファイルのパスで一番外側の `.app` を見て、本体と同じアプリとして扱う。
//! - まとめたプロセスの音をステレオにミックスするタップを作り、それを入力に持つ
//!   非公開の集約デバイスを作って、IOProc で受け取る。
//! - タップは「ミュートしない」設定なので、アプリの音はそのままスピーカーからも聞こえる。

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, CATapDescription, CATapMuteBehavior, kAudioAggregateDeviceIsPrivateKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioDevicePropertyNominalSampleRate, kAudioHardwarePropertyProcessObjectList,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    kAudioProcessPropertyBundleID, kAudioProcessPropertyIsRunningOutput, kAudioProcessPropertyPID,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsNonInterleaved,
};
use objc2_foundation::{NSArray, NSMutableDictionary, NSNumber, NSString};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};

/// 音を出す（出したことのある）アプリ
#[derive(Clone, Debug)]
pub struct AudioApp {
    /// 表示名（`.app` の名前。無ければバンドルIDや実行ファイル名）
    pub name: String,
    /// いま音を出しているか
    pub playing: bool,
    /// `.app` の中のプロセスか（false ならシステムの内部プロセスなど）
    pub bundled: bool,
    /// 属するプロセスの Core Audio オブジェクト
    process_objects: Vec<AudioObjectID>,
}

fn addr(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn os(status: i32, what: &str) -> Result<(), String> {
    if status == 0 { Ok(()) } else { Err(format!("{what} に失敗しました（OSStatus {status}）")) }
}

/// 固定長のプロパティを読む
fn get<T: Copy>(obj: AudioObjectID, selector: u32) -> Option<T> {
    let a = addr(selector);
    let mut v = std::mem::MaybeUninit::<T>::uninit();
    let mut size = std::mem::size_of::<T>() as u32;
    let st = unsafe {
        AudioObjectGetPropertyData(
            obj,
            NonNull::from(&a),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(v.as_mut_ptr() as *mut c_void).unwrap(),
        )
    };
    (st == 0 && size as usize == std::mem::size_of::<T>()).then(|| unsafe { v.assume_init() })
}

fn get_object_list(obj: AudioObjectID, selector: u32) -> Vec<AudioObjectID> {
    let a = addr(selector);
    let mut size = 0u32;
    if unsafe { AudioObjectGetPropertyDataSize(obj, NonNull::from(&a), 0, std::ptr::null(), NonNull::from(&mut size)) }
        != 0
    {
        return Vec::new();
    }
    let mut ids = vec![0 as AudioObjectID; size as usize / std::mem::size_of::<AudioObjectID>()];
    if ids.is_empty() {
        return ids;
    }
    let st = unsafe {
        AudioObjectGetPropertyData(
            obj,
            NonNull::from(&a),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(ids.as_mut_ptr() as *mut c_void).unwrap(),
        )
    };
    if st != 0 {
        return Vec::new();
    }
    ids.truncate(size as usize / std::mem::size_of::<AudioObjectID>());
    ids
}

fn get_string(obj: AudioObjectID, selector: u32) -> Option<String> {
    let p: *mut c_void = get(obj, selector)?;
    // CFString（+1 で返る）を NSString として受け取る（toll-free bridge）
    let s: Retained<NSString> = unsafe { Retained::from_raw(p as *mut NSString)? };
    Some(s.to_string())
}

fn exe_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// パスの中で一番外側の `.app` の名前（拡張子なし）
fn outer_app_name(path: &str) -> Option<String> {
    path.split('/').find_map(|c| c.strip_suffix(".app")).map(str::to_string)
}

/// Core Audio に音を出す（出したことのある）アプリの一覧。音を出している順、名前順。
pub fn list_audio_apps() -> Vec<AudioApp> {
    let me = std::process::id() as i32;
    let mut groups: BTreeMap<String, AudioApp> = BTreeMap::new();
    for obj in get_object_list(kAudioObjectSystemObject as AudioObjectID, kAudioHardwarePropertyProcessObjectList) {
        let Some(pid) = get::<i32>(obj, kAudioProcessPropertyPID) else { continue };
        if pid == me {
            continue;
        }
        let path = exe_path(pid);
        let app_name = path.as_deref().and_then(outer_app_name);
        let bundled = app_name.is_some();
        let name = app_name
            .or_else(|| get_string(obj, kAudioProcessPropertyBundleID).filter(|s| !s.is_empty()))
            .or_else(|| path.as_deref().and_then(|p| p.rsplit('/').next()).map(str::to_string))
            .unwrap_or_else(|| format!("PID {pid}"));
        let playing = get::<u32>(obj, kAudioProcessPropertyIsRunningOutput).unwrap_or(0) != 0;
        let g = groups.entry(name.clone()).or_insert(AudioApp { name, playing: false, bundled, process_objects: Vec::new() });
        g.playing |= playing;
        g.process_objects.push(obj);
    }
    let mut apps: Vec<AudioApp> = groups.into_values().collect();
    apps.sort_by_key(|a| !a.playing);
    apps
}

/// IOProc に渡す受け取り先
type Sink = Box<dyn FnMut(&[f32], usize) + Send>;

struct Ctx {
    sink: Sink,
    non_interleaved: bool,
    scratch: Vec<f32>,
}

/// 動いているタップ。drop で止めて片付ける。
pub struct AppTap {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    ctx: *mut Ctx,
    pub sample_rate: u32,
    pub channels: usize,
}

// IOProc からは ctx をポインタ経由で使うだけで、AppTap 自体はスレッドをまたいで動かしてよい
unsafe impl Send for AppTap {}

static COUNTER: AtomicU32 = AtomicU32::new(0);

impl AppTap {
    /// タップと集約デバイスを作る（まだ音は流れない）。
    pub fn create(app: &AudioApp) -> Result<Self, String> {
        let ids: Vec<Retained<NSNumber>> =
            app.process_objects.iter().map(|&id| NSNumber::new_u32(id)).collect();
        let list = NSArray::from_retained_slice(&ids);
        let desc =
            unsafe { CATapDescription::initStereoMixdownOfProcesses(CATapDescription::alloc(), &list) };
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        unsafe {
            desc.setName(&NSString::from_str(&format!("倍音解析エディタ {pid}.{n}")));
            desc.setPrivate(true);
            desc.setMuteBehavior(CATapMuteBehavior::Unmuted);
        }
        let mut tap: AudioObjectID = 0;
        os(unsafe { AudioHardwareCreateProcessTap(Some(&desc), &mut tap) }, "タップの作成")?;
        let fmt: Option<AudioStreamBasicDescription> = get(tap, kAudioTapPropertyFormat);
        let Some(fmt) = fmt.filter(|f| f.mFormatFlags & kAudioFormatFlagIsFloat != 0) else {
            unsafe { AudioHardwareDestroyProcessTap(tap) };
            return Err("タップの音声形式が浮動小数点ではありません".into());
        };

        let tap_uid = unsafe { desc.UUID().UUIDString() };
        let sub: Retained<NSMutableDictionary<NSString, AnyObject>> = NSMutableDictionary::new();
        sub.insert(&*key(kAudioSubTapUIDKey), &*tap_uid);
        sub.insert(&*key(kAudioSubTapDriftCompensationKey), &*NSNumber::new_bool(true));
        let taps = NSArray::from_retained_slice(&[sub]);
        let props: Retained<NSMutableDictionary<NSString, AnyObject>> = NSMutableDictionary::new();
        let name = NSString::from_str(&format!("倍音解析エディタ タップ {pid}.{n}"));
        let uid = NSString::from_str(&format!("harmonic-analyzer.tap.{pid}.{n}"));
        let yes = NSNumber::new_bool(true);
        props.insert(&*key(kAudioAggregateDeviceNameKey), &*name);
        props.insert(&*key(kAudioAggregateDeviceUIDKey), &*uid);
        props.insert(&*key(kAudioAggregateDeviceTapListKey), &*taps);
        props.insert(&*key(kAudioAggregateDeviceTapAutoStartKey), &*yes);
        props.insert(&*key(kAudioAggregateDeviceIsPrivateKey), &*yes);
        let mut aggregate: AudioObjectID = 0;
        let st = unsafe {
            let cf = &*(Retained::as_ptr(&props) as *const objc2_core_foundation::CFDictionary);
            AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate))
        };
        if let Err(e) = os(st, "集約デバイスの作成") {
            unsafe { AudioHardwareDestroyProcessTap(tap) };
            return Err(e);
        }
        let rate = get::<f64>(aggregate, kAudioDevicePropertyNominalSampleRate)
            .filter(|r| *r > 0.0)
            .unwrap_or(fmt.mSampleRate);
        let ctx = Box::into_raw(Box::new(Ctx {
            sink: Box::new(|_, _| {}),
            non_interleaved: fmt.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0,
            scratch: Vec::new(),
        }));
        Ok(AppTap {
            tap,
            aggregate,
            proc_id: None,
            ctx,
            sample_rate: rate.round() as u32,
            channels: (fmt.mChannelsPerFrame as usize).max(1),
        })
    }

    /// 音を流し始める。`sink(インターリーブされた f32, チャンネル数)` が音声スレッドから呼ばれる。
    pub fn start(&mut self, sink: Sink) -> Result<(), String> {
        unsafe { (*self.ctx).sink = sink };
        let mut id: AudioDeviceIOProcID = None;
        os(
            unsafe { AudioDeviceCreateIOProcID(self.aggregate, Some(io_proc), self.ctx as *mut c_void, NonNull::from(&mut id)) },
            "音声コールバックの登録",
        )?;
        self.proc_id = id;
        os(unsafe { AudioDeviceStart(self.aggregate, self.proc_id) }, "取り込みの開始")
    }
}

fn key(k: &'static std::ffi::CStr) -> Retained<NSString> {
    NSString::from_str(k.to_str().unwrap())
}

impl Drop for AppTap {
    fn drop(&mut self) {
        unsafe {
            if self.proc_id.is_some() {
                AudioDeviceStop(self.aggregate, self.proc_id);
                AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
            }
            AudioHardwareDestroyAggregateDevice(self.aggregate);
            AudioHardwareDestroyProcessTap(self.tap);
            if !self.ctx.is_null() {
                drop(Box::from_raw(self.ctx));
            }
        }
    }
}

unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    let ctx = unsafe { &mut *(client as *mut Ctx) };
    let list = unsafe { input.as_ref() };
    let n = list.mNumberBuffers as usize;
    if n == 0 {
        return 0;
    }
    let bufs: &[AudioBuffer] = unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), n) };
    let slice = |b: &AudioBuffer| -> &[f32] {
        if b.mData.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(b.mData as *const f32, b.mDataByteSize as usize / 4) }
        }
    };
    if n == 1 && !ctx.non_interleaved {
        let ch = (bufs[0].mNumberChannels as usize).max(1);
        (ctx.sink)(slice(&bufs[0]), ch);
    } else {
        // チャンネルごとに別のバッファ → インターリーブし直す
        let frames = bufs.iter().map(|b| slice(b).len()).min().unwrap_or(0);
        ctx.scratch.clear();
        for i in 0..frames {
            for b in bufs {
                ctx.scratch.push(slice(b)[i]);
            }
        }
        (ctx.sink)(&ctx.scratch, n);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::outer_app_name;

    #[test]
    fn helper_process_belongs_to_outer_app() {
        let p = "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/1/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper";
        assert_eq!(outer_app_name(p).as_deref(), Some("Google Chrome"));
        assert_eq!(outer_app_name("/usr/bin/afplay"), None);
    }
}
