//! 録音から基音を推定する（YIN 法）。
//!
//! 音量のある区間から最大 40 か所を取り出し、それぞれで周期を求めて中央値を取る。
//! 立ち上がりや減衰の端で多少外れても、中央値なので結果は揺れにくい。

/// 推定する基音の範囲（Hz）
pub const MIN_HZ: f64 = 25.0;
pub const MAX_HZ: f64 = 4000.0;

/// YIN の「周期らしさ」のしきい値（小さいほど厳しい）
const THRESHOLD: f64 = 0.15;
/// 一番大きい区間からこれ以上小さい区間は使わない（dB）
const QUIET_DB: f64 = 30.0;
const MAX_FRAMES: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    pub hz: f64,
    /// 調べた区間のうち、周期がはっきり見つかった割合（0..1）
    pub confidence: f64,
}

/// 1 区間の周期（サンプル数、小数）と、その周期らしさ（0 に近いほど周期的）
fn yin(frame: &[f32], tau_min: usize, tau_max: usize) -> Option<(f64, f64)> {
    let w = frame.len() - tau_max;
    // 差分関数 d(τ) と、累積平均で正規化した d'(τ)
    let mut d = vec![0f64; tau_max + 2];
    for (tau, v) in d.iter_mut().enumerate().take(tau_max + 2).skip(1) {
        if tau + w > frame.len() {
            break;
        }
        *v = (0..w).map(|i| (frame[i] as f64 - frame[i + tau] as f64).powi(2)).sum();
    }
    let mut cmnd = vec![1f64; d.len()];
    let mut running = 0.0;
    for tau in 1..d.len() {
        running += d[tau];
        cmnd[tau] = if running > 0.0 { d[tau] * tau as f64 / running } else { 1.0 };
    }
    // しきい値を下回った最初の谷を取る（下回らなければ最小値）。短い周期を優先してオクターブ下への誤りを防ぐ
    let hi = tau_max.min(cmnd.len() - 2);
    let mut best = None;
    let mut tau = tau_min.max(2);
    while tau <= hi {
        if cmnd[tau] < THRESHOLD {
            while tau < hi && cmnd[tau + 1] < cmnd[tau] {
                tau += 1;
            }
            best = Some(tau);
            break;
        }
        tau += 1;
    }
    let tau = best.unwrap_or_else(|| (tau_min.max(2)..=hi).min_by(|&a, &b| cmnd[a].total_cmp(&cmnd[b])).unwrap_or(tau_min));
    // 放物線補間で谷の位置を小数まで求める
    let (a, b, c) = (cmnd[tau - 1], cmnd[tau], cmnd[tau + 1]);
    let den = a - 2.0 * b + c;
    let shift = if den.abs() > 1e-12 { (0.5 * (a - c) / den).clamp(-1.0, 1.0) } else { 0.0 };
    Some((tau as f64 + shift, b))
}

/// モノラルの音から基音を推定する。無音や周期の無い音なら None。
pub fn estimate_f0(samples: &[f32], sample_rate: u32) -> Option<Estimate> {
    let sr = sample_rate as f64;
    let max_hz = MAX_HZ.min(sr / 4.0);
    let tau_min = (sr / max_hz).floor() as usize;
    let tau_max = (sr / MIN_HZ).ceil() as usize;
    // 窓は最長の周期の 2 倍以上
    let len = 2 * tau_max + 2;
    if samples.len() < len {
        return None;
    }
    let hop = (len / 2).max(1);
    let starts: Vec<usize> = (0..=samples.len() - len).step_by(hop).collect();
    let rms: Vec<f64> = starts
        .iter()
        .map(|&s| (samples[s..s + len].iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / len as f64).sqrt())
        .collect();
    let loudest = rms.iter().cloned().fold(0.0, f64::max);
    if loudest < 1e-5 {
        return None;
    }
    let floor = loudest * 10f64.powf(-QUIET_DB / 20.0);
    let loud: Vec<usize> = starts.iter().zip(&rms).filter(|&(_, &r)| r >= floor).map(|(&s, _)| s).collect();
    // 多すぎるときは等間隔に間引く
    let step = loud.len().div_ceil(MAX_FRAMES).max(1);
    let mut periods = Vec::new();
    let mut tried = 0usize;
    for &s in loud.iter().step_by(step) {
        tried += 1;
        if let Some((tau, aperiodicity)) = yin(&samples[s..s + len], tau_min, tau_max)
            && aperiodicity < THRESHOLD
        {
            periods.push(tau);
        }
    }
    if periods.is_empty() {
        return None;
    }
    periods.sort_by(f64::total_cmp);
    let tau = periods[periods.len() / 2];
    Some(Estimate { hz: sr / tau, confidence: periods.len() as f64 / tried as f64 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_tones::{SR, inharmonic_750, sine_220, sine_220_plus_half_440, third_harmonic_decay};
    use std::f64::consts::TAU;

    fn tone(f: impl Fn(f64) -> f64) -> Vec<f32> {
        (0..SR as usize).map(|i| f(i as f64 / SR as f64) as f32).collect()
    }

    fn assert_near(est: Option<Estimate>, hz: f64) {
        let e = est.expect("推定できなかった");
        let cents = 1200.0 * (e.hz / hz).log2();
        assert!(cents.abs() < 2.0, "{hz} Hz のはずが {:.3} Hz（{cents:+.2} cent）", e.hz);
        assert!(e.confidence > 0.8, "信頼度が低い: {}", e.confidence);
    }

    #[test]
    fn finds_fundamental_of_test_tones() {
        assert_near(estimate_f0(&sine_220(), SR), 220.0);
        assert_near(estimate_f0(&sine_220_plus_half_440(), SR), 220.0);
        assert_near(estimate_f0(&third_harmonic_decay(), SR), 220.0);
    }

    #[test]
    fn finds_detuned_and_low_and_high_notes() {
        for hz in [41.2, 233.08, 1046.5] {
            assert_near(estimate_f0(&tone(|t| 0.5 * (TAU * hz * t).sin()), SR), hz);
        }
    }

    #[test]
    fn finds_missing_fundamental() {
        // 第2〜第4倍音だけで基音（150 Hz）の成分が無くても、周期は 150 Hz
        let x = tone(|t| (2..=4).map(|k| 0.3 * (TAU * 150.0 * k as f64 * t).sin()).sum());
        assert_near(estimate_f0(&x, SR), 150.0);
    }

    #[test]
    fn weak_fundamental_is_not_mistaken_for_octave_up() {
        // 基音が第2倍音より 8 dB 小さくても、第3倍音があれば 220 Hz と分かる
        let x = tone(|t| [(1, 0.2), (2, 0.5), (3, 0.3)].iter().map(|&(k, a)| a * (TAU * 220.0 * k as f64 * t).sin()).sum());
        assert_near(estimate_f0(&x, SR), 220.0);
    }

    #[test]
    fn inharmonic_partial_gives_common_period() {
        // 220 Hz と非整数倍の 750 Hz を含む音は、両方がほぼ整数倍になる低い周期（約 108 Hz）を返す。
        // 「基音」があいまいな音なので、そのときは利用者が手で直す前提
        let e = estimate_f0(&inharmonic_750(), SR).expect("推定できなかった");
        let ratio = 220.0 / e.hz;
        assert!((ratio - ratio.round()).abs() < 0.05, "220 Hz の約数になっていない: {:.2} Hz", e.hz);
    }

    #[test]
    fn silence_and_noise_give_nothing() {
        assert!(estimate_f0(&vec![0.0; SR as usize], SR).is_none());
        // 疑似乱数の雑音
        let mut x = 12345u32;
        let noise: Vec<f32> = (0..SR as usize)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32 - 0.5) * 0.5
            })
            .collect();
        let e = estimate_f0(&noise, SR);
        assert!(e.is_none_or(|e| e.confidence < 0.5), "雑音で自信を持って推定した: {e:?}");
    }
}
