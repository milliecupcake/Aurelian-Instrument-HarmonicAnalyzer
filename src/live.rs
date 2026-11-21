//! 入力デバイス（マイク・オーディオインターフェイス）からの取り込み。
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
}

pub struct LiveInput {
    _stream: cpal::Stream,
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

impl LiveInput {
    /// `device` が None なら既定の入力デバイスを使う。
    pub fn open(device: Option<&str>, seconds: f64) -> Result<Self, String> {
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
        let len = (seconds * sample_rate as f64).round() as usize;
        let ring = Arc::new(Mutex::new(Ring { buf: vec![0.0; len.max(1)], write: 0, total: 0, peak: 0.0 }));
        let error = Arc::new(Mutex::new(None));

        let stream = match format {
            SampleFormat::F32 => build::<f32>(&dev, config, ring.clone(), error.clone(), |v| v),
            SampleFormat::I16 => build::<i16>(&dev, config, ring.clone(), error.clone(), |v| v as f32 / 32768.0),
            SampleFormat::I32 => build::<i32>(&dev, config, ring.clone(), error.clone(), |v| v as f32 / 2147483648.0),
            SampleFormat::U16 => build::<u16>(&dev, config, ring.clone(), error.clone(), |v| (v as f32 - 32768.0) / 32768.0),
            f => return Err(format!("未対応の入力形式です: {f:?}")),
        }?;
        stream.play().map_err(|e| format!("入力を開始できません: {e}"))?;
        Ok(LiveInput { _stream: stream, ring, sample_rate, channels, device_name, error })
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
            let Ok(mut r) = ring.lock() else { return };
            let mut peak = 0f32;
            for frame in data.chunks(ch) {
                let v = frame.iter().map(|&s| to_f32(s)).sum::<f32>() / ch as f32;
                peak = peak.max(v.abs());
                let w = r.write;
                r.buf[w] = v;
                r.write = (w + 1) % r.buf.len();
            }
            r.total += (data.len() / ch) as u64;
            r.peak = peak;
        },
        move |e| *error.lock().unwrap() = Some(e.to_string()),
        None,
    )
    .map_err(|e| format!("入力ストリームを作れません: {e}"))
}
