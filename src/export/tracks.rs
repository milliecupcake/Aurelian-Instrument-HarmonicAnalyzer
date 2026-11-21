//! 基音の時間変化と、倍音以外の成分（AIEXP-10）。
//!
//! どちらも倍音解析（harmonics.csv）とは別の参考値で、倍音の値には使わない。
//! 倍音解析はいつも録音全体で 1 つの固定基音（analysis_f0_hz）を使う。
//!
//! - `pitch_track.csv` 約 10 ms ごとの基音推定（YIN）と、推定できたかどうか
//! - `partials.csv`    同じ時刻の、縮約前のスペクトルの局所ピークと、固定基音の整数倍との関係

use super::*;
use crate::analysis::{FrameSpectrum, amp_to_db, spectral_peaks};
use crate::pitch::{TrackPoint, TrackStatus, track_f0};

/// 基音推定とピーク抽出の時間間隔の目安（秒）。実際は hop の整数倍
pub const TRACK_STEP_S: f64 = 0.01;
/// ピークを拾う範囲（そのフレームの最大から、dB）。窓のサイドローブ（約 −92 dB）を拾わないように
pub const PEAK_RANGE_DB: f64 = 80.0;
/// 1 フレームで拾うピークの最大数（大きい順）
pub const MAX_PEAKS_PER_FRAME: usize = 40;
/// 「繰り返し現れる」とみなす、調べたフレームに占める割合
pub const RECURRING_MIN_SHARE: f64 = 0.2;
/// 要約に載せる繰り返し現れる非整数倍ピークの最大数
pub const RECURRING_MAX: usize = 10;

/// 何フレームおきに調べるか（hop × これ ≈ TRACK_STEP_S）
pub fn grid_step(a: &Analysis) -> usize {
    ((TRACK_STEP_S * a.sample_rate as f64 / a.hop as f64).round() as usize).max(1)
}

/// 調べるフレーム（0 から grid_step おき）
pub fn grid(a: &Analysis) -> Vec<usize> {
    (0..a.num_frames).step_by(grid_step(a)).collect()
}

/// 値の列の分位点（線形補間）。空なら None
fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let x = q * (sorted.len() - 1) as f64;
    let (i, t) = (x.floor() as usize, x.fract());
    let j = (i + 1).min(sorted.len() - 1);
    Some(sorted[i] + (sorted[j] - sorted[i]) * t)
}

fn cents(hz: f64, reference: f64) -> f64 {
    1200.0 * (hz / reference).log2()
}

/// 基音の時間変化の集計
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PitchStats {
    pub voiced: usize,
    pub unvoiced: usize,
    pub quiet: usize,
    pub edge: usize,
    pub median_hz: Option<f64>,
    /// voiced のフレームの、固定基音からのずれ（cent）の 5%・50%・95% 点
    pub cents_p05: Option<f64>,
    pub cents_median: Option<f64>,
    pub cents_p95: Option<f64>,
    /// 固定基音からのずれの絶対値の最大（cent）
    pub max_abs_cents: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PitchTrack {
    /// (フレーム番号, 推定結果)
    pub rows: Vec<(usize, TrackPoint)>,
    pub stats: PitchStats,
}

impl PitchTrack {
    /// 固定基音とのずれが大きいか（中央値が ±50 cent を超える、または 5〜95% の幅が 50 cent を超える）
    pub fn deviates(&self) -> bool {
        let s = &self.stats;
        s.cents_median.is_some_and(|m| m.abs() > 50.0)
            || matches!((s.cents_p05, s.cents_p95), (Some(a), Some(b)) if b - a > 50.0)
    }
}

pub fn pitch_track(snap: &Snapshot) -> PitchTrack {
    let a = &snap.analysis;
    let frames = grid(a);
    let centers: Vec<usize> = frames.iter().map(|&f| f * a.hop).collect();
    let points = track_f0(&snap.clip.mono, a.sample_rate, &centers);
    let mut stats = PitchStats::default();
    let mut dev = Vec::new();
    let mut hz = Vec::new();
    for p in &points {
        match p.status {
            TrackStatus::Voiced => stats.voiced += 1,
            TrackStatus::Unvoiced => stats.unvoiced += 1,
            TrackStatus::Quiet => stats.quiet += 1,
            TrackStatus::Edge => stats.edge += 1,
        }
        if let Some(h) = p.hz {
            hz.push(h);
            dev.push(cents(h, a.f0));
        }
    }
    hz.sort_by(f64::total_cmp);
    dev.sort_by(f64::total_cmp);
    stats.median_hz = quantile(&hz, 0.5);
    stats.cents_p05 = quantile(&dev, 0.05);
    stats.cents_median = quantile(&dev, 0.5);
    stats.cents_p95 = quantile(&dev, 0.95);
    stats.max_abs_cents = dev.iter().map(|d| d.abs()).fold(None, |m, d| Some(m.map_or(d, |m: f64| m.max(d))));
    PitchTrack { rows: frames.into_iter().zip(points).collect(), stats }
}

/// ピークと固定基音の整数倍との関係
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartialClass {
    /// 最も近い整数倍から、倍音の探索範囲（±half_width_hz）以内
    Harmonic,
    /// 探索範囲の外（整数倍の間）
    Inharmonic,
    /// 基音の探索範囲より低い
    BelowF0,
}

impl PartialClass {
    pub fn as_str(self) -> &'static str {
        match self {
            PartialClass::Harmonic => "harmonic",
            PartialClass::Inharmonic => "inharmonic",
            PartialClass::BelowF0 => "below_f0",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Partial {
    pub frame: usize,
    /// そのフレームで振幅の大きい順（1 始まり）
    pub rank: usize,
    pub hz: f64,
    pub db: f32,
    /// そのフレームの最大のピークに対する dB
    pub rel_max_db: f32,
    /// 最も近い整数倍の番号（1 以上）
    pub nearest: usize,
    /// hz − nearest × f0
    pub dev_hz: f64,
    pub class: PartialClass,
}

/// 繰り返し現れる非整数倍のピーク（周波数の近いものをまとめたもの）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recurring {
    pub median_hz: f64,
    pub min_hz: f64,
    pub max_hz: f64,
    pub median_db: f64,
    /// 現れたフレーム数
    pub frames: usize,
    /// 調べたフレームに占める割合
    pub share: f64,
    pub nearest: usize,
    pub dev_hz: f64,
    pub class: PartialClass,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Partials {
    /// ピークを調べたフレーム数（グリッドのうち edge_padded でないもの）
    pub frames_examined: usize,
    pub rows: Vec<Partial>,
    pub recurring: Vec<Recurring>,
    /// 倍音とみなす幅（片側、Hz）= 倍音の探索範囲
    pub half_width_hz: f64,
    /// まとめるときの周波数の幅（Hz）
    pub cluster_hz: f64,
}

pub fn classify(hz: f64, f0: f64, half_width_hz: f64) -> (usize, f64, PartialClass) {
    let nearest = ((hz / f0).round() as usize).max(1);
    let dev = hz - nearest as f64 * f0;
    let class = if hz < f0 - half_width_hz {
        PartialClass::BelowF0
    } else if dev.abs() <= half_width_hz {
        PartialClass::Harmonic
    } else {
        PartialClass::Inharmonic
    };
    (nearest, dev, class)
}

pub fn partials(snap: &Snapshot, progress: &Progress) -> Result<Partials, ExportError> {
    let a = &snap.analysis;
    let bin_hz = a.bin_hz();
    let half_width_hz = a.search_half_width_bins * bin_hz;
    let mut reader = FrameSpectrum::for_analysis(a);
    let mut rows = Vec::new();
    let mut examined = 0usize;
    for (i, f) in grid(a).into_iter().enumerate() {
        if i % 64 == 0 && progress.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        if a.is_edge_padded(f) {
            continue;
        }
        examined += 1;
        let mag = reader.compute(&snap.clip.mono, f).map_err(ExportError::Failed)?;
        let peaks = spectral_peaks(mag, bin_hz, PEAK_RANGE_DB, MAX_PEAKS_PER_FRAME);
        let top = peaks.first().map(|p| amp_to_db(p.amp));
        for (r, p) in peaks.iter().enumerate() {
            let (nearest, dev_hz, class) = classify(p.hz, a.f0, half_width_hz);
            let db = amp_to_db(p.amp);
            rows.push(Partial {
                frame: f,
                rank: r + 1,
                hz: p.hz,
                db,
                rel_max_db: db - top.unwrap_or(db),
                nearest,
                dev_hz,
                class,
            });
        }
    }
    let cluster_hz = 2.0 * bin_hz;
    let recurring = recurring(&rows, examined, cluster_hz, a.f0, half_width_hz);
    Ok(Partials { frames_examined: examined, rows, recurring, half_width_hz, cluster_hz })
}

/// 非整数倍・基音より下のピークを周波数の幅 `cell_hz` の升に分け、隣の升も合わせて多くのフレームに現れる周波数を探す
fn recurring(rows: &[Partial], examined: usize, cell_hz: f64, f0: f64, half_width_hz: f64) -> Vec<Recurring> {
    use std::collections::{BTreeMap, BTreeSet};
    if examined == 0 {
        return Vec::new();
    }
    let others: Vec<&Partial> = rows.iter().filter(|p| p.class != PartialClass::Harmonic).collect();
    let cell = |hz: f64| (hz / cell_hz).floor() as i64;
    let mut frames_in: BTreeMap<i64, BTreeSet<usize>> = BTreeMap::new();
    for p in &others {
        frames_in.entry(cell(p.hz)).or_default().insert(p.frame);
    }
    // 升 c の前後 1 升に現れたフレーム数
    let coverage = |c: i64| -> usize {
        let mut s: BTreeSet<usize> = BTreeSet::new();
        for d in -1..=1 {
            if let Some(v) = frames_in.get(&(c + d)) {
                s.extend(v);
            }
        }
        s.len()
    };
    let mut cand: Vec<(i64, usize)> = frames_in.keys().map(|&c| (c, coverage(c))).collect();
    cand.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    let need = ((RECURRING_MIN_SHARE * examined as f64).ceil() as usize).max(1);
    let mut taken: Vec<i64> = Vec::new();
    let mut out = Vec::new();
    for (c, n) in cand {
        if n < need || out.len() >= RECURRING_MAX {
            break;
        }
        if taken.iter().any(|&t| (t - c).abs() <= 2) {
            continue;
        }
        taken.push(c);
        // 同じフレームに複数あれば強いほうだけ使う
        let mut best: BTreeMap<usize, &Partial> = BTreeMap::new();
        for p in others.iter().filter(|p| (cell(p.hz) - c).abs() <= 1) {
            let e = best.entry(p.frame).or_insert(p);
            if p.db > e.db {
                *e = p;
            }
        }
        let mut hz: Vec<f64> = best.values().map(|p| p.hz).collect();
        let mut db: Vec<f64> = best.values().map(|p| p.db as f64).collect();
        hz.sort_by(f64::total_cmp);
        db.sort_by(f64::total_cmp);
        let median_hz = quantile(&hz, 0.5).unwrap_or(0.0);
        let (nearest, dev_hz, class) = classify(median_hz, f0, half_width_hz);
        out.push(Recurring {
            median_hz,
            min_hz: hz[0],
            max_hz: hz[hz.len() - 1],
            median_db: quantile(&db, 0.5).unwrap_or(0.0),
            frames: best.len(),
            share: best.len() as f64 / examined as f64,
            nearest,
            dev_hz,
            class,
        });
    }
    out.sort_by(|x, y| y.median_db.total_cmp(&x.median_db));
    out
}

/// pitch_track.csv と partials.csv の内容
#[derive(Clone, Debug, PartialEq)]
pub struct Tracks {
    pub step: usize,
    pub pitch: PitchTrack,
    pub partials: Partials,
}

pub fn compute(snap: &Snapshot, progress: &Progress) -> Result<Tracks, ExportError> {
    let pitch = pitch_track(snap);
    if progress.is_cancelled() {
        return Err(ExportError::Cancelled);
    }
    let partials = partials(snap, progress)?;
    Ok(Tracks { step: grid_step(&snap.analysis), pitch, partials })
}

/// pitch_track.csv の列（この順）
pub const PITCH_COLUMNS: [&str; 7] =
    ["frame_index", "time_s", "status", "f0_hz", "cents_from_analysis_f0", "aperiodicity", "rms_db"];

/// partials.csv の列（この順）
pub const PARTIAL_COLUMNS: [&str; 11] = [
    "frame_index",
    "time_s",
    "rank",
    "peak_hz",
    "level_db",
    "rel_frame_max_db",
    "nearest_harmonic",
    "nearest_harmonic_hz",
    "deviation_hz",
    "deviation_cents",
    "class",
];

pub fn write_pitch_csv(snap: &Snapshot, t: &Tracks, w: &mut impl Write) -> Result<(), ExportError> {
    use std::fmt::Write as _;
    let a = &snap.analysis;
    let mut o = PITCH_COLUMNS.join(",");
    o.push('\n');
    for (f, p) in &t.pitch.rows {
        let _ = write!(o, "{f},");
        num(&mut o, a.frame_time(*f));
        let _ = write!(o, ",{},", p.status.as_str());
        opt(&mut o, p.hz);
        o.push(',');
        opt(&mut o, p.hz.map(|h| cents(h, a.f0)));
        o.push(',');
        opt(&mut o, p.aperiodicity);
        o.push(',');
        opt(&mut o, p.rms.map(|r| amp_to_db(r) as f64));
        o.push('\n');
    }
    w.write_all(o.as_bytes()).map_err(fail(FILE_PITCH))
}

pub fn write_partials_csv(snap: &Snapshot, t: &Tracks, w: &mut impl Write) -> Result<(), ExportError> {
    use std::fmt::Write as _;
    let a = &snap.analysis;
    let mut w = BufWriter::with_capacity(1 << 16, w);
    let io = fail(FILE_PARTIALS);
    let mut o = PARTIAL_COLUMNS.join(",");
    o.push('\n');
    for p in &t.partials.rows {
        let _ = write!(o, "{},", p.frame);
        num(&mut o, a.frame_time(p.frame));
        let _ = write!(o, ",{},", p.rank);
        num(&mut o, p.hz);
        o.push(',');
        num32(&mut o, p.db);
        o.push(',');
        num32(&mut o, p.rel_max_db);
        let _ = write!(o, ",{},", p.nearest);
        let target = a.harmonic_hz(p.nearest);
        num(&mut o, target);
        o.push(',');
        num(&mut o, p.dev_hz);
        o.push(',');
        num(&mut o, cents(p.hz, target));
        let _ = writeln!(o, ",{}", p.class.as_str());
        if o.len() > 1 << 15 {
            w.write_all(o.as_bytes()).map_err(&io)?;
            o.clear();
        }
    }
    w.write_all(o.as_bytes()).map_err(&io)?;
    w.flush().map_err(&io)
}
