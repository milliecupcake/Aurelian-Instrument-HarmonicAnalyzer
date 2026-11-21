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
    // 差分関数 d(τ)
    let mut d = vec![0f64; tau_max + 2];
    for (tau, v) in d.iter_mut().enumerate().take(tau_max + 2).skip(1) {
        if tau + w > frame.len() {
            break;
        }
        *v = (0..w).map(|i| (frame[i] as f64 - frame[i + tau] as f64).powi(2)).sum();
    }
    Some(pick_period(&d, tau_min, tau_max))
}

/// 差分関数 d(τ) から、累積平均で正規化した d'(τ) の谷を選ぶ。（周期, その谷の d'）
fn pick_period(d: &[f64], tau_min: usize, tau_max: usize) -> (f64, f64) {
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
    (tau as f64 + shift, b)
}

/// 探す周期の範囲（サンプル数）。最短 = fs / min(MAX_HZ, fs/4)、最長 = fs / MIN_HZ
pub fn period_range(sample_rate: u32) -> (usize, usize) {
    let sr = sample_rate as f64;
    let max_hz = MAX_HZ.min(sr / 4.0);
    ((sr / max_hz).floor() as usize, (sr / MIN_HZ).ceil() as usize)
}

/// 推定する基音の上限（Hz）。fs/4 を超えない
pub fn max_hz(sample_rate: u32) -> f64 {
    MAX_HZ.min(sample_rate as f64 / 4.0)
}

/// 時間ごとの推定で使う区間の長さ（サンプル数、最長の周期の 2 倍 + 2）
pub fn track_window(sample_rate: u32) -> usize {
    2 * period_range(sample_rate).1 + 2
}

/// 時間ごとの推定で、最も大きい区間からこれ以上小さい区間は推定しない（dB）
pub const TRACK_QUIET_DB: f64 = 50.0;
/// これより小さい RMS の区間は推定しない（−100 dB）
pub const TRACK_SILENT_RMS: f64 = 1.0e-5;
/// 周期らしさのしきい値（これ未満なら voiced）
pub const TRACK_THRESHOLD: f64 = THRESHOLD;

/// 時刻ごとの基音推定の結果
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrackStatus {
    /// 周期を検出した
    Voiced,
    /// 音はあるが、周期がはっきりしない（雑音・複数の音・急な変化など）
    Unvoiced,
    /// 音が小さすぎるので推定しない
    Quiet,
    /// 推定の区間が音声の端をはみ出すので推定しない
    Edge,
}

impl TrackStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TrackStatus::Voiced => "voiced",
            TrackStatus::Unvoiced => "unvoiced",
            TrackStatus::Quiet => "quiet",
            TrackStatus::Edge => "edge",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackPoint {
    /// 区間の中心（サンプル位置）
    pub center: usize,
    pub status: TrackStatus,
    /// voiced のときだけ
    pub hz: Option<f64>,
    /// 選んだ谷の正規化差分 d'（voiced・unvoiced のとき。小さいほど周期的）
    pub aperiodicity: Option<f64>,
    /// 区間の RMS（edge 以外）
    pub rms: Option<f64>,
}

/// 指定した中心ごとに基音を推定する（YIN、差分関数は FFT で計算する）。
/// 区間の長さは `track_window`。中心がその半分より端に近いものは `Edge`。
pub fn track_f0(samples: &[f32], sample_rate: u32, centers: &[usize]) -> Vec<TrackPoint> {
    use realfft::RealFftPlanner;
    let (tau_min, tau_max) = period_range(sample_rate);
    let len = track_window(sample_rate);
    let w = len - tau_max - 1; // d(τ) を τ = tau_max + 1 まで求める
    let n = len.next_power_of_two();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf_a = fwd.make_input_vec();
    let mut buf_b = fwd.make_input_vec();
    let mut spec_a = fwd.make_output_vec();
    let mut spec_b = fwd.make_output_vec();
    let mut corr = inv.make_output_vec();
    let mut prefix = vec![0f64; len + 1];
    let mut d = vec![0f64; tau_max + 2];

    // 1 回目: 区間を取り出して RMS を求める
    let windows: Vec<Option<(usize, f64)>> = centers
        .iter()
        .map(|&c| {
            let start = c.checked_sub(len / 2)?;
            (start + len <= samples.len()).then(|| {
                let e: f64 = samples[start..start + len].iter().map(|&v| (v as f64).powi(2)).sum();
                (start, (e / len as f64).sqrt())
            })
        })
        .collect();
    let loudest = windows.iter().flatten().map(|w| w.1).fold(0.0, f64::max);
    let quiet = (loudest * 10f64.powf(-TRACK_QUIET_DB / 20.0)).max(TRACK_SILENT_RMS);

    centers
        .iter()
        .zip(&windows)
        .map(|(&center, win)| {
            let Some((start, rms)) = *win else {
                return TrackPoint { center, status: TrackStatus::Edge, hz: None, aperiodicity: None, rms: None };
            };
            if rms < quiet {
                return TrackPoint { center, status: TrackStatus::Quiet, hz: None, aperiodicity: None, rms: Some(rms) };
            }
            let mut period = |start: usize| -> (f64, f64) {
                let x = &samples[start..start + len];
                // r(τ) = Σ_{i<w} x[i]·x[i+τ] を FFT の相互相関で求める（n ≥ len なので循環の影響は無い）
                buf_a.iter_mut().for_each(|v| *v = 0.0);
                buf_b.iter_mut().for_each(|v| *v = 0.0);
                for i in 0..len {
                    buf_b[i] = x[i] as f64;
                }
                buf_a[..w].copy_from_slice(&buf_b[..w]);
                let _ = fwd.process(&mut buf_a, &mut spec_a);
                let _ = fwd.process(&mut buf_b, &mut spec_b);
                for (a, b) in spec_a.iter_mut().zip(&spec_b) {
                    *a = a.conj() * b;
                }
                let _ = inv.process(&mut spec_a, &mut corr);
                for i in 0..len {
                    prefix[i + 1] = prefix[i] + buf_b[i] * buf_b[i];
                }
                let e0 = prefix[w];
                for (tau, v) in d.iter_mut().enumerate() {
                    let r = corr[tau] / n as f64;
                    *v = (e0 + prefix[tau + w] - prefix[tau] - 2.0 * r).max(0.0);
                }
                d[0] = 0.0;
                pick_period(&d, tau_min, tau_max)
            };
            // 周期 τ の比較に使うのは区間の先頭から w + τ サンプル。その中心が指定の中心に来るよう、
            // 1 回目の周期で区間をずらしてもう一度求める（ピッチが動くときの時刻のずれを防ぐ）
            let first = period(start);
            let used = w + first.0.round() as usize;
            let shifted = (center - used / 2).min(samples.len() - len).max(start);
            let (tau, ap) = if shifted == start { first } else { period(shifted) };
            let voiced = ap < TRACK_THRESHOLD;
            TrackPoint {
                center,
                status: if voiced { TrackStatus::Voiced } else { TrackStatus::Unvoiced },
                hz: voiced.then(|| sample_rate as f64 / tau),
                aperiodicity: Some(ap),
                rms: Some(rms),
            }
        })
        .collect()
}

/// モノラルの音から基音を推定する。無音や周期の無い音なら None。
pub fn estimate_f0(samples: &[f32], sample_rate: u32) -> Option<Estimate> {
    let sr = sample_rate as f64;
    let (tau_min, tau_max) = period_range(sample_rate);
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

    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut x = 987654321u32;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32 - 0.5) * 2.0 * amp
            })
            .collect()
    }

    #[test]
    fn fft_difference_matches_direct_yin() {
        let x = tone(|t| 0.5 * (TAU * 233.0 * t).sin() + 0.2 * (TAU * 466.0 * t).sin());
        let len = track_window(SR);
        let c = SR as usize / 2;
        let p = track_f0(&x, SR, &[c])[0];
        let (tau_min, tau_max) = period_range(SR);
        let (tau, ap) = yin(&x[c - len / 2..c - len / 2 + len], tau_min, tau_max).unwrap();
        assert_eq!(p.status, TrackStatus::Voiced);
        assert!((p.hz.unwrap() - SR as f64 / tau).abs() < 0.05, "{:?} vs {}", p.hz, SR as f64 / tau);
        assert!((p.aperiodicity.unwrap() - ap).abs() < 0.02);
    }

    #[test]
    fn track_follows_glide_and_vibrato_and_marks_gaps() {
        // 0〜1 s: 200 → 400 Hz の指数グライド、1〜1.5 s: 無音、1.5〜2 s: 雑音、
        // 2〜3 s: 300 Hz ± 50 cent・5 Hz のビブラート
        let sr = SR as f64;
        let n = 3 * SR as usize;
        let mut x = vec![0f32; n];
        let mut phase = 0.0;
        let hz_at = |t: f64| -> Option<f64> {
            if t < 1.0 {
                Some(200.0 * 2f64.powf(t))
            } else if t >= 2.0 {
                Some(300.0 * 2f64.powf(50.0 / 1200.0 * (TAU * 5.0 * (t - 2.0)).sin()))
            } else {
                None
            }
        };
        for (i, v) in x.iter_mut().enumerate() {
            let t = i as f64 / sr;
            if let Some(hz) = hz_at(t) {
                phase += TAU * hz / sr;
                *v = (0.5 * phase.sin()) as f32;
            }
        }
        let nz = noise(n, 0.3);
        x[(1.5 * sr) as usize..2 * SR as usize].copy_from_slice(&nz[(1.5 * sr) as usize..2 * SR as usize]);

        let centers: Vec<usize> = (0..n).step_by(480).collect();
        let pts = track_f0(&x, SR, &centers);
        let half = track_window(SR) as f64 / 2.0 / sr + 0.002;
        let mut checked = 0;
        for p in &pts {
            let t = p.center as f64 / sr;
            let near = |a: f64| (t - a).abs() <= half;
            if t < half || t > 3.0 - half {
                assert_eq!(p.status, TrackStatus::Edge, "{t}");
                continue;
            }
            if near(1.0) || near(1.5) || near(2.0) {
                continue; // 区間の境目をまたぐ
            }
            match hz_at(t) {
                Some(hz) => {
                    assert_eq!(p.status, TrackStatus::Voiced, "{t} s");
                    let cents = 1200.0 * (p.hz.unwrap() / hz).log2();
                    // 区間の中で周波数が動くので、その分の誤差を見込む
                    assert!(cents.abs() < 20.0, "{t} s: {:?} Hz のはずが {hz} Hz（{cents:+.1} cent）", p.hz);
                    checked += 1;
                }
                None if t < 1.5 => assert_eq!(p.status, TrackStatus::Quiet, "{t} s"),
                None => assert_eq!(p.status, TrackStatus::Unvoiced, "{t} s（雑音）"),
            }
        }
        assert!(checked > 100);
        // ビブラートの山と谷が見える
        let vib: Vec<f64> = pts
            .iter()
            .filter(|p| p.center as f64 / sr > 2.1 && p.status == TrackStatus::Voiced)
            .map(|p| 1200.0 * (p.hz.unwrap() / 300.0).log2())
            .collect();
        let (lo, hi) = vib.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        assert!(hi > 30.0 && lo < -30.0, "ビブラートの幅 {lo:.1}〜{hi:.1} cent");
    }
}
