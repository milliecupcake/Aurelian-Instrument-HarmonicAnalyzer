//! 入力デバイス（マイク・オーディオインターフェイス）または特定のアプリの音の取り込み。
//!
//! 直近 `seconds` 秒をモノラル（全チャンネルの平均）でリングバッファに保持する。
//! 解析は呼び出し側が `snapshot` で取り出した音に対して行う。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample};
use std::sync::{Arc, Mutex};

struct Ring {
    buf: Vec<f32>,
    /// 次に書き込む位置
    write: usize,
    /// これまでに受け取ったサンプル数（新しい音が来たかの判定用）
    total: u64,
    /// 直近のブロックのピーク（入力レベル表示用）
    peak: f32,
    /// 一度でも 0 でない音が来たか
    heard: bool,
}

/// 何から取り込むか
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// 入力デバイス（None = 既定）
    Device(Option<String>),
    /// アプリが出している音（アプリ名）
    App(String),
}

enum Backend {
    Device(#[allow(dead_code)] cpal::Stream),
    #[cfg(target_os = "macos")]
    App(#[allow(dead_code)] crate::app_tap::AppTap),
}

pub struct LiveInput {
    _backend: Backend,
    ring: Arc<Mutex<Ring>>,
    pub sample_rate: u32,
    pub channels: usize,
    pub device_name: String,
    error: Arc<Mutex<Option<String>>>,
}

/// 入力デバイスの名前一覧（既定のデバイスが先頭）
pub fn input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.description().ok()).map(|d| d.name().to_string());
    let mut names: Vec<String> = host
        .input_devices()
        .map(|ds| ds.filter_map(|d| d.description().ok().map(|d| d.name().to_string())).collect())
        .unwrap_or_default();
    if let Some(def) = default {
        names.retain(|n| *n != def);
        names.insert(0, def);
    }
    names
}

fn new_ring(seconds: f64, rate: u32) -> Arc<Mutex<Ring>> {
    let len = (seconds * rate as f64).round() as usize;
    Arc::new(Mutex::new(Ring { buf: vec![0.0; len.max(1)], write: 0, total: 0, peak: 0.0, heard: false }))
}

/// インターリーブされた音を、チャンネル平均のモノラルにしてリングへ書く
fn push(ring: &Mutex<Ring>, data: &[f32], ch: usize) {
    let Ok(mut r) = ring.lock() else { return };
    let ch = ch.max(1);
    let mut peak = 0f32;
    for frame in data.chunks_exact(ch) {
        let v = frame.iter().sum::<f32>() / ch as f32;
        peak = peak.max(v.abs());
        let w = r.write;
        r.buf[w] = v;
        r.write = (w + 1) % r.buf.len();
    }
    r.total += (data.len() / ch) as u64;
    r.peak = peak;
    r.heard |= peak > 0.0;
}

impl LiveInput {
    pub fn open(source: &Source, seconds: f64) -> Result<Self, String> {
        match source {
            Source::Device(d) => Self::open_device(d.as_deref(), seconds),
            Source::App(name) => Self::open_app(name, seconds),
        }
    }

    #[cfg(target_os = "macos")]
    fn open_app(name: &str, seconds: f64) -> Result<Self, String> {
        use crate::app_tap::{AppTap, list_audio_apps};
        let app = list_audio_apps()
            .into_iter()
            .find(|a| a.name == name)
            .ok_or_else(|| format!("「{name}」は音を出していません（一度音を鳴らしてから選び直してください）"))?;
        let mut tap = AppTap::create(&app)?;
        let ring = new_ring(seconds, tap.sample_rate);
        let r = ring.clone();
        tap.start(Box::new(move |data, ch| push(&r, data, ch)))?;
        Ok(LiveInput {
            sample_rate: tap.sample_rate,
            channels: tap.channels,
            device_name: format!("アプリ: {name}"),
            _backend: Backend::App(tap),
            ring,
            error: Arc::new(Mutex::new(None)),
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn open_app(_name: &str, _seconds: f64) -> Result<Self, String> {
        Err("アプリの音の取り込みは macOS だけで使えます".into())
    }

    /// `device` が None なら既定の入力デバイスを使う。
    fn open_device(device: Option<&str>, seconds: f64) -> Result<Self, String> {
        let host = cpal::default_host();
        let dev = match device {
            Some(name) => host
                .input_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.description().map(|x| x.name() == name).unwrap_or(false))
                .ok_or_else(|| format!("入力デバイス「{name}」が見つかりません"))?,
            None => host.default_input_device().ok_or("入力デバイスが見つかりません")?,
        };
        let device_name = dev.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "（不明）".into());
        let supported = dev.default_input_config().map_err(|e| format!("入力デバイスを開けません: {e}"))?;
        let format = supported.sample_format();
        let config = supported.config();
        let sample_rate = config.sample_rate;
        let channels = config.channels as usize;
        let ring = new_ring(seconds, sample_rate);
        let error = Arc::new(Mutex::new(None));

        let stream = match format {
            SampleFormat::F32 => build::<f32>(&dev, config, ring.clone(), error.clone(), |v| v),
            SampleFormat::I16 => build::<i16>(&dev, config, ring.clone(), error.clone(), |v| v as f32 / 32768.0),
            SampleFormat::I32 => build::<i32>(&dev, config, ring.clone(), error.clone(), |v| v as f32 / 2147483648.0),
            SampleFormat::U16 => build::<u16>(&dev, config, ring.clone(), error.clone(), |v| (v as f32 - 32768.0) / 32768.0),
            f => return Err(format!("未対応の入力形式です: {f:?}")),
        }?;
        stream.play().map_err(|e| format!("入力を開始できません: {e}"))?;
        Ok(LiveInput { _backend: Backend::Device(stream), ring, sample_rate, channels, device_name, error })
    }

    /// 直近の音（古い順）と、これまでに受け取ったサンプル数
    pub fn snapshot(&self) -> (Vec<f32>, u64) {
        let r = self.ring.lock().unwrap();
        let mut out = Vec::with_capacity(r.buf.len());
        out.extend_from_slice(&r.buf[r.write..]);
        out.extend_from_slice(&r.buf[..r.write]);
        (out, r.total)
    }

    /// 直近の入力ピーク（0..1）
    pub fn peak(&self) -> f32 {
        self.ring.lock().unwrap().peak
    }

    /// 2 秒以上受け取っているのに完全な無音のままか。
    /// アプリの音は、macOS の許可が無いとエラーにならず 0 だけが届くので、その目安にする。
    pub fn silent_so_far(&self) -> bool {
        let r = self.ring.lock().unwrap();
        !r.heard && r.total > 2 * self.sample_rate as u64
    }

    pub fn is_app(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            matches!(self._backend, Backend::App(_))
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}

fn build<T: SizedSample + Copy + Send + 'static>(
    dev: &cpal::Device,
    config: cpal::StreamConfig,
    ring: Arc<Mutex<Ring>>,
    error: Arc<Mutex<Option<String>>>,
    to_f32: fn(T) -> f32,
) -> Result<cpal::Stream, String> {
    let ch = (config.channels as usize).max(1);
    dev.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| {
            let f: Vec<f32> = data.iter().map(|&s| to_f32(s)).collect();
            push(&ring, &f, ch);
        },
        move |e| *error.lock().unwrap() = Some(e.to_string()),
        None,
    )
    .map_err(|e| format!("入力ストリームを作れません: {e}"))
}

/// 取り込めるアプリ
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    /// 取り込み対象の名前（`Source::App` に渡す）
    pub name: String,
    /// いま音を出しているか
    pub playing: bool,
    /// ふつうのアプリ（`.app`）か。false はシステムの内部プロセスなど
    pub bundled: bool,
}

/// 取り込めるアプリ（音を出している順。macOS 以外では空）
pub fn audio_apps() -> Vec<AppInfo> {
    #[cfg(target_os = "macos")]
    {
        crate::app_tap::list_audio_apps()
            .into_iter()
            .map(|a| AppInfo { name: a.name, playing: a.playing, bundled: a.bundled })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_averages_channels_and_wraps() {
        let ring = new_ring(1.0, 4);
        push(&ring, &[1.0, 0.0, 0.5, 0.5, -1.0, -1.0], 2);
        push(&ring, &[0.2, 0.2, 0.4, 0.4], 2);
        let r = ring.lock().unwrap();
        assert_eq!(r.total, 5);
        // 4 サンプルのリング: 0.5, 0.5, -1.0, 0.2 の後に 0.4 が先頭を上書き
        assert_eq!(r.buf, vec![0.4, 0.5, -1.0, 0.2]);
        assert_eq!(r.write, 1);
    }
}
