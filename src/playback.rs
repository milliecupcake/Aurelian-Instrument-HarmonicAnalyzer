//! cpal による再生。出力デバイスのサンプルレートが違う場合は線形補間で変換する。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

struct Clip {
    data: Vec<f32>,
    channels: usize,
    rate: u32,
    frames: usize,
}

#[derive(Default)]
struct Shared {
    clip: Mutex<Option<Arc<Clip>>>,
    /// 再生位置（秒、f64 のビット列）
    pos_bits: AtomicU64,
    playing: AtomicBool,
}

pub struct Player {
    _stream: Option<cpal::Stream>,
    shared: Arc<Shared>,
    pub error: Option<String>,
}

impl Player {
    pub fn new() -> Self {
        let shared = Arc::new(Shared::default());
        match open_stream(shared.clone()) {
            Ok(stream) => Player { _stream: Some(stream), shared, error: None },
            Err(e) => Player { _stream: None, shared, error: Some(e) },
        }
    }

    pub fn set_clip(&self, interleaved: &[f32], channels: usize, rate: u32) {
        self.stop();
        let clip = Clip { data: interleaved.to_vec(), channels, rate, frames: interleaved.len() / channels.max(1) };
        *self.shared.clip.lock().unwrap() = Some(Arc::new(clip));
    }

    pub fn play_from(&self, t: f64) {
        self.shared.pos_bits.store(t.max(0.0).to_bits(), Ordering::SeqCst);
        self.shared.playing.store(true, Ordering::SeqCst);
    }

    pub fn stop(&self) {
        self.shared.playing.store(false, Ordering::SeqCst);
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::SeqCst)
    }

    pub fn position(&self) -> f64 {
        f64::from_bits(self.shared.pos_bits.load(Ordering::SeqCst))
    }
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

fn open_stream(shared: Arc<Shared>) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("出力デバイスが見つかりません")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?.config();
    let out_ch = config.channels as usize;
    let out_rate = config.sample_rate as f64;
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out: &mut [f32], _| {
                out.iter_mut().for_each(|s| *s = 0.0);
                if !shared.playing.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(guard) = shared.clip.try_lock() else { return };
                let Some(clip) = guard.as_ref() else { return };
                let mut pos = f64::from_bits(shared.pos_bits.load(Ordering::Relaxed));
                let dt = 1.0 / out_rate;
                for frame in out.chunks_mut(out_ch) {
                    let sp = pos * clip.rate as f64;
                    let i0 = sp.floor() as usize;
                    if i0 + 1 >= clip.frames {
                        shared.playing.store(false, Ordering::Relaxed);
                        break;
                    }
                    let frac = (sp - i0 as f64) as f32;
                    for (c, s) in frame.iter_mut().enumerate() {
                        let sc = c % clip.channels;
                        let a = clip.data[i0 * clip.channels + sc];
                        let b = clip.data[(i0 + 1) * clip.channels + sc];
                        *s = a + (b - a) * frac;
                    }
                    pos += dt;
                }
                shared.pos_bits.store(pos.to_bits(), Ordering::Relaxed);
            },
            |e| eprintln!("audio error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}
