//! 試聴用の周波数帯フィルタ。
//!
//! 録音全体を一度に FFT し、周波数ごとのゲインを掛けて戻す（位相は変えない）。
//! 境界は二乗余弦でなだらかにつなぎ、急峻な遮断による「リンギング」を抑える。
//! 遷移幅は境界周波数の 3%（5〜100 Hz）で、境界ちょうどのゲインは -6 dB。
//! 解析には使わず、再生する音だけに掛ける。

use realfft::RealFftPlanner;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandMode {
    /// 範囲内だけを残す（バンドパス）
    Keep,
    /// 範囲内だけを取り除く（バンドストップ）
    Remove,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub low_hz: f64,
    pub high_hz: f64,
    pub mode: BandMode,
}

pub fn transition_width(edge_hz: f64) -> f64 {
    (edge_hz * 0.03).clamp(5.0, 100.0)
}

/// 0→1 へなめらかに上がる段差。edge で 0.5。
fn smooth_step(f: f64, edge: f64) -> f64 {
    let w = transition_width(edge);
    let x = ((f - (edge - w / 2.0)) / w).clamp(0.0, 1.0);
    0.5 - 0.5 * (std::f64::consts::PI * x).cos()
}

/// 周波数 f でのゲイン（0..1）
pub fn gain(band: &Band, f: f64) -> f64 {
    let lo = band.low_hz.min(band.high_hz).max(0.0);
    let hi = band.low_hz.max(band.high_hz);
    // 下限 0 Hz のときは DC まで通す
    let above_lo = if lo <= 0.0 { 1.0 } else { smooth_step(f, lo) };
    let below_hi = 1.0 - smooth_step(f, hi);
    let inside = above_lo * below_hi;
    match band.mode {
        BandMode::Keep => inside,
        BandMode::Remove => 1.0 - inside,
    }
}

/// インターリーブされた音声にフィルタを掛ける。
pub fn apply(interleaved: &[f32], channels: usize, sample_rate: u32, band: &Band) -> Vec<f32> {
    let channels = channels.max(1);
    let frames = interleaved.len() / channels;
    if frames == 0 {
        return interleaved.to_vec();
    }
    // 末尾が先頭へ回り込まないよう 0.5 秒以上のゼロを足し、2のべき乗にそろえる
    let n = (frames + sample_rate as usize / 2).next_power_of_two();
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf = fwd.make_input_vec();
    let mut spec = fwd.make_output_vec();
    let bin_hz = sample_rate as f64 / n as f64;
    let gains: Vec<f32> = (0..spec.len()).map(|b| gain(band, b as f64 * bin_hz) as f32).collect();
    let mut out = vec![0f32; interleaved.len()];

    for c in 0..channels {
        buf.iter_mut().for_each(|v| *v = 0.0);
        for i in 0..frames {
            buf[i] = interleaved[i * channels + c];
        }
        fwd.process(&mut buf, &mut spec).expect("forward fft");
        for (s, g) in spec.iter_mut().zip(&gains) {
            *s *= *g / n as f32;
        }
        // 実数信号の DC とナイキストの虚部は 0
        spec[0].im = 0.0;
        let last = spec.len() - 1;
        spec[last].im = 0.0;
        inv.process(&mut spec, &mut buf).expect("inverse fft");
        for i in 0..frames {
            out[i * channels + c] = buf[i];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{WindowSize, analyze};
    use crate::test_tones::{SR, inharmonic_750};

    fn levels(x: &[f32]) -> (f32, f32) {
        let a = analyze(x, SR, 220.0, WindowSize::Auto).unwrap();
        let f = a.frame_at(1.0);
        let s = &a.spectrogram;
        (a.level_db(f, 1).unwrap(), s.value_at(1.0, 750.0).unwrap())
    }

    #[test]
    fn keep_band_isolates_750() {
        let x = inharmonic_750();
        let (h1, c750) = levels(&x);
        let y = apply(&x, 1, SR, &Band { low_hz: 600.0, high_hz: 900.0, mode: BandMode::Keep });
        let (h1f, c750f) = levels(&y);
        assert!(h1f < h1 - 80.0, "220 Hz が残っている: {h1} → {h1f}");
        assert!((c750f - c750).abs() < 0.1, "750 Hz が変わった: {c750} → {c750f}");
    }

    #[test]
    fn remove_band_drops_only_750() {
        let x = inharmonic_750();
        let (h1, _) = levels(&x);
        let y = apply(&x, 1, SR, &Band { low_hz: 600.0, high_hz: 900.0, mode: BandMode::Remove });
        let (h1f, c750f) = levels(&y);
        assert!((h1f - h1).abs() < 0.1, "220 Hz が変わった: {h1} → {h1f}");
        assert!(c750f < -90.0, "750 Hz が残っている: {c750f}");
    }

    #[test]
    fn stereo_channels_stay_separate() {
        // 両端がフェードした 220 Hz を左だけに入れる
        let x: Vec<f32> = crate::test_tones::sine_220().into_iter().flat_map(|v| [v, 0.0]).collect();
        let y = apply(&x, 2, SR, &Band { low_hz: 0.0, high_hz: 20000.0, mode: BandMode::Keep });
        assert!(y.iter().skip(1).step_by(2).all(|v| v.abs() < 1e-4));
        let d = x.iter().zip(&y).step_by(2).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(d < 1e-3, "通過域で波形が変わった: {d}");
    }
}
