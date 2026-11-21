//! WAV の読み込み。PCM 8/16/24/32 bit 整数と 32 bit 浮動小数点、任意のチャンネル数に対応する。
//! 解析には全チャンネルの平均（モノラル）を使い、再生には元のチャンネルを使う。

use std::path::Path;

#[derive(Clone, Debug)]
pub struct AudioClip {
    pub sample_rate: u32,
    pub channels: usize,
    /// 再生用（インターリーブ、-1.0..1.0）
    pub interleaved: Vec<f32>,
    /// 解析用（全チャンネル平均）
    pub mono: Vec<f32>,
    pub format_desc: String,
}

impl AudioClip {
    pub fn duration(&self) -> f64 {
        self.mono.len() as f64 / self.sample_rate as f64
    }

    pub fn from_mono(samples: Vec<f32>, sample_rate: u32) -> Self {
        AudioClip {
            sample_rate,
            channels: 1,
            interleaved: samples.clone(),
            mono: samples,
            format_desc: "32-bit float".into(),
        }
    }
}

pub fn load_wav(path: &Path) -> Result<AudioClip, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("WAV を開けません: {e}"))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    if channels == 0 {
        return Err("チャンネル数が 0 です".into());
    }
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                return Err(format!("{} bit 浮動小数点には未対応です", spec.bits_per_sample));
            }
            reader.samples::<f32>().collect::<Result<_, _>>().map_err(|e| e.to_string())?
        }
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            if !(matches!(bits, 8 | 16 | 24 | 32)) {
                return Err(format!("{bits} bit 整数には未対応です"));
            }
            let full = (1u64 << (bits - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / full))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    let mono: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|fr| fr.iter().sum::<f32>() / channels as f32)
        .collect();
    let kind = match spec.sample_format {
        hound::SampleFormat::Float => "浮動小数点",
        hound::SampleFormat::Int => "整数",
    };
    Ok(AudioClip {
        sample_rate: spec.sample_rate,
        channels,
        interleaved,
        mono,
        format_desc: format!("{} bit {}", spec.bits_per_sample, kind),
    })
}

/// 32 bit 浮動小数点のモノラル WAV を書き出す（試験音の生成用）。
pub fn write_wav_mono(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for &s in samples {
        w.write_sample(s).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}
