//! 比較用の特徴量（AIEXP-08）。
//!
//! - 基音に対する相対レベル（フレームごと・倍音ごとの中央値）
//! - 元波形の RMS・ピークと、スペクトル重心の時間変化（envelope.csv）
//! - 倍音ごとの最大レベルと時刻、持続区間の代表レベル、減衰に要する時間（features.csv）
//!
//! 集計に使うのは窓が音声に収まるフレーム（edge_padded でないもの）だけ。
//! 持続区間は利用者の指定だけで扱い、推測で補わない。発音開始は指定か、根拠を記録した推定。
//! キーを離した時刻（リリース）は扱わない。

use super::*;
use crate::analysis::{FLOOR_DB, PeakStatus, amp_to_db};

/// 発音開始の自動推定: RMS が最大値からこの dB 以内に初めて入るフレーム
pub const ONSET_WITHIN_DB: f64 = 20.0;
/// 減衰の閾値（各倍音の最大レベルからの低下量）
pub const DECAY_DROPS_DB: [f64; 2] = [20.0, 40.0];

/// 相対レベルを出せない理由
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelMissing {
    H1BelowFloor,
    H1NotDetected,
    HarmonicBelowFloor,
    HarmonicNotDetected,
}

impl RelMissing {
    pub fn as_str(self) -> &'static str {
        match self {
            RelMissing::H1BelowFloor => "h1_below_floor",
            RelMissing::H1NotDetected => "h1_not_detected",
            RelMissing::HarmonicBelowFloor => "harmonic_below_floor",
            RelMissing::HarmonicNotDetected => "harmonic_not_detected",
        }
    }
}

/// 第k倍音の基音に対する相対レベル level_db(k) − level_db(1)。両方でピークを検出したときだけ
pub fn relative_to_h1(a: &Analysis, frame: usize, k: usize) -> Result<f32, RelMissing> {
    let st = |k| match a.level(frame, k) {
        HarmonicLevel::Level { db, status, .. } => (db, status),
        HarmonicLevel::OutOfRange => (FLOOR_DB, PeakStatus::BelowFloor),
    };
    let (h1, s1) = st(1);
    match s1 {
        PeakStatus::BelowFloor => return Err(RelMissing::H1BelowFloor),
        PeakStatus::SampledTargetBin => return Err(RelMissing::H1NotDetected),
        PeakStatus::DetectedPeak => {}
    }
    let (hk, sk) = st(k);
    match sk {
        PeakStatus::BelowFloor => Err(RelMissing::HarmonicBelowFloor),
        PeakStatus::SampledTargetBin => Err(RelMissing::HarmonicNotDetected),
        PeakStatus::DetectedPeak => Ok(hk - h1),
    }
}

/// フレームごとの RMS とピーク（dB、振幅 1.0 = 0 dB、下限 −120 dB）。
/// 窓の範囲（解析と同じ中心・長さの矩形窓）のうち、音声の中にあるサンプルだけで計算する
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub rms_db: Vec<f32>,
    pub peak_db: Vec<f32>,
    /// 計算に使ったサンプル数（端のフレームでは窓長より少ない）
    pub samples_used: Vec<usize>,
}

pub fn envelope(snap: &Snapshot) -> Envelope {
    let a = &snap.analysis;
    let x = &snap.clip.mono;
    let mut env = Envelope { rms_db: Vec::new(), peak_db: Vec::new(), samples_used: Vec::new() };
    for f in 0..a.num_frames {
        let start = (f * a.hop) as isize - (a.window / 2) as isize;
        let lo = start.max(0) as usize;
        let hi = ((start + a.window as isize).max(0) as usize).min(x.len());
        let seg = if lo < hi { &x[lo..hi] } else { &x[0..0] };
        let n = seg.len();
        let (sum, peak) = seg.iter().fold((0.0f64, 0.0f64), |(s, p), &v| (s + (v as f64).powi(2), p.max((v as f64).abs())));
        let rms = if n > 0 { (sum / n as f64).sqrt() } else { 0.0 };
        env.rms_db.push(amp_to_db(rms));
        env.peak_db.push(amp_to_db(peak));
        env.samples_used.push(n);
    }
    env
}

/// 発音開始
#[derive(Clone, Debug, PartialEq)]
pub enum Onset {
    /// 利用者が指定した（書き出す音の時刻、秒）
    Manual { t: f64, frame: usize },
    /// RMS から推定した
    Auto { t: f64, frame: usize, max_rms_db: f32 },
    /// 決められない（理由）
    Unavailable(String),
}

impl Onset {
    pub fn frame(&self) -> Option<usize> {
        match self {
            Onset::Manual { frame, .. } | Onset::Auto { frame, .. } => Some(*frame),
            Onset::Unavailable(_) => None,
        }
    }
}

/// 減衰の結果
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Decay {
    /// 最大の時刻から閾値を下回るまでの秒数
    Reached(f64),
    /// 観測の終わり（集計区間の最後のフレーム）まで閾値を下回らなかった
    NotReached,
    /// 閾値が下限（−120 dB）以下で判定できない
    ThresholdBelowFloor,
    /// ピークを検出したフレームが無い
    NoPeak,
}

impl Decay {
    pub fn status(self) -> &'static str {
        match self {
            Decay::Reached(_) => "reached",
            Decay::NotReached => "not_reached",
            Decay::ThresholdBelowFloor => "threshold_below_floor",
            Decay::NoPeak => "no_peak",
        }
    }

    pub fn seconds(self) -> Option<f64> {
        if let Decay::Reached(s) = self { Some(s) } else { None }
    }
}

/// 中央値（偶数個なら中央 2 つの平均）
fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    Some(if v.len() % 2 == 1 { v[m] } else { (v[m - 1] + v[m]) / 2.0 })
}

/// 持続区間の代表レベル（区間内でピークを検出したフレームの level_db の中央値）
#[derive(Clone, Debug, PartialEq)]
pub struct Sustain {
    pub median_db: Option<f64>,
    pub detected: usize,
    pub frames: usize,
    pub missing: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HarmonicFeatures {
    pub harmonic: usize,
    /// 集計に使ったフレーム数（発音開始から最後の有効フレームまで）
    pub frames: usize,
    pub detected: usize,
    /// 最大レベル（dB）とその時刻（秒）。ピークを検出したフレームの中で
    pub max: Option<(f32, f64)>,
    pub sustain: Sustain,
    /// 基音に対する相対レベルの中央値（dB）と使ったフレーム数
    pub rel_h1_median_db: Option<f64>,
    pub rel_h1_frames: usize,
    pub rel_h1_missing: Option<&'static str>,
    pub decay: [Decay; 2],
}

/// 特徴量の計算に使う区間（書き出す音の時刻）
#[derive(Clone, Debug, PartialEq)]
pub struct Features {
    pub envelope: Envelope,
    /// 窓が音声に収まるフレームの範囲
    pub valid: Option<(usize, usize)>,
    pub onset: Onset,
    /// 持続区間（指定されていて、有効なフレームがあるとき）のフレーム範囲
    pub sustain_frames: Option<(usize, usize)>,
    /// 持続区間を使えない理由（未指定・区間外など）
    pub sustain_missing: Option<&'static str>,
    pub harmonics: Vec<HarmonicFeatures>,
}

pub fn compute(snap: &Snapshot) -> Features {
    let a = &snap.analysis;
    let envelope = envelope(snap);
    let valid = summary::stats_frames(a);
    let dur = snap.clip.duration();

    // 発音開始
    let onset = match (valid, snap.intervals.onset_s.map(|t| snap.out_time(t))) {
        (None, _) => Onset::Unavailable("窓が音声に収まるフレームが無い".into()),
        (Some(_), Some(t)) if !(0.0..=dur).contains(&t) => {
            Onset::Unavailable("指定した発音開始が書き出す区間の外".into())
        }
        (Some((first, last)), Some(t)) => {
            let frame = a.frame_at(t).clamp(first, last);
            Onset::Manual { t, frame }
        }
        (Some((first, last)), None) => {
            let max_rms = (first..=last).map(|f| envelope.rms_db[f]).fold(FLOOR_DB, f32::max);
            if max_rms <= FLOOR_DB {
                Onset::Unavailable("無音（RMS が下限以下）".into())
            } else {
                let thr = max_rms as f64 - ONSET_WITHIN_DB;
                let frame = (first..=last).find(|&f| envelope.rms_db[f] as f64 >= thr).unwrap_or(first);
                Onset::Auto { t: a.frame_time(frame), frame, max_rms_db: max_rms }
            }
        }
    };

    // 持続区間（指定だけ）
    let (sustain_frames, sustain_missing) = match (valid, snap.intervals.sustain_s) {
        (_, None) => (None, Some("not_specified")),
        (None, Some(_)) => (None, Some("no_valid_frames")),
        (Some((first, last)), Some((s, e))) => {
            let (s, e) = (snap.out_time(s), snap.out_time(e));
            let fr: Vec<usize> = (first..=last).filter(|&f| (s..=e).contains(&a.frame_time(f))).collect();
            match (fr.first(), fr.last()) {
                (Some(&x), Some(&y)) => (Some((x, y)), None),
                _ => (None, Some("no_valid_frames_in_interval")),
            }
        }
    };

    let mut harmonics = Vec::with_capacity(a.num_in_range);
    for k in 1..=a.num_in_range {
        let mut h = HarmonicFeatures {
            harmonic: k,
            frames: 0,
            detected: 0,
            max: None,
            sustain: Sustain { median_db: None, detected: 0, frames: 0, missing: sustain_missing },
            rel_h1_median_db: None,
            rel_h1_frames: 0,
            rel_h1_missing: None,
            decay: [Decay::NoPeak; 2],
        };
        let Some((first, last)) = valid else {
            h.rel_h1_missing = Some("no_valid_frames");
            harmonics.push(h);
            continue;
        };
        let from = onset.frame().unwrap_or(first).max(first);
        let mut max_frame = None;
        for f in from..=last {
            h.frames += 1;
            if let HarmonicLevel::Level { db, status: PeakStatus::DetectedPeak, .. } = a.level(f, k) {
                h.detected += 1;
                if h.max.is_none_or(|(m, _)| db > m) {
                    h.max = Some((db, a.frame_time(f)));
                    max_frame = Some(f);
                }
            }
        }

        // 持続区間の代表レベル
        if let Some((s, e)) = sustain_frames {
            let mut vals = Vec::new();
            for f in s..=e {
                h.sustain.frames += 1;
                if let HarmonicLevel::Level { db, status: PeakStatus::DetectedPeak, .. } = a.level(f, k) {
                    vals.push(db as f64);
                }
            }
            h.sustain.detected = vals.len();
            h.sustain.median_db = median(vals);
            if h.sustain.median_db.is_none() {
                h.sustain.missing = Some("no_detected_peak_in_interval");
            }
        }

        // 相対レベル: 持続区間があればその中、無ければ発音開始から最後まで
        let (rs, re) = sustain_frames.unwrap_or((from, last));
        let mut rels = Vec::new();
        let mut reasons = Vec::new();
        for f in rs..=re {
            match relative_to_h1(a, f, k) {
                Ok(v) => rels.push(v as f64),
                Err(r) => reasons.push(r),
            }
        }
        h.rel_h1_frames = rels.len();
        h.rel_h1_median_db = median(rels);
        if h.rel_h1_median_db.is_none() {
            // いちばん多かった理由
            let mut best: Option<(RelMissing, usize)> = None;
            for r in [RelMissing::H1BelowFloor, RelMissing::H1NotDetected, RelMissing::HarmonicBelowFloor, RelMissing::HarmonicNotDetected] {
                let n = reasons.iter().filter(|&&x| x == r).count();
                if n > 0 && best.is_none_or(|(_, m)| n > m) {
                    best = Some((r, n));
                }
            }
            h.rel_h1_missing = Some(best.map(|b| b.0.as_str()).unwrap_or("no_frames"));
        }

        // 減衰: 最大の時刻から、最大 − X dB を下回るまで（status を問わず level_db で判定、フレーム間は直線補間）
        if let (Some((max_db, t_max)), Some(mf)) = (h.max, max_frame) {
            for (i, drop) in DECAY_DROPS_DB.iter().enumerate() {
                let thr = max_db as f64 - drop;
                h.decay[i] = if thr <= FLOOR_DB as f64 {
                    Decay::ThresholdBelowFloor
                } else {
                    let lv = |f| a.level_db(f, k).unwrap_or(FLOOR_DB) as f64;
                    match (mf + 1..=last).find(|&f| lv(f) <= thr) {
                        Some(f) => {
                            let (y0, y1) = (lv(f - 1), lv(f));
                            let frac = if y0 > y1 { ((y0 - thr) / (y0 - y1)).clamp(0.0, 1.0) } else { 1.0 };
                            let t = a.frame_time(f - 1) + frac * (a.frame_time(f) - a.frame_time(f - 1));
                            Decay::Reached(t - t_max)
                        }
                        None => Decay::NotReached,
                    }
                };
            }
        }
        harmonics.push(h);
    }

    Features { envelope, valid, onset, sustain_frames, sustain_missing, harmonics }
}
