//! AI や外部の解析プログラム向けの書き出し（AI_ANALYSIS_EXPORT_TASKS の P1〜P3）。
//!
//! 書き出し開始時点の音声・解析結果・設定・画面の状態を `Snapshot` に固定し、
//! そこから次のファイルを 1 つの ZIP にまとめる。
//!
//! - `source.wav`     試聴フィルタ適用前の音声（32-bit float、元のチャンネルのまま）
//! - `manifest.json`  解析条件・音源情報・表示と試聴の状態・データ形式の説明
//! - `harmonics.csv`  解析範囲内の全倍音 × 全フレームの数値（基音に対する相対レベルつき）
//! - `features.csv`   倍音ごとの特徴量（最大・持続区間の代表値・相対レベル・減衰時間）
//! - `envelope.csv`   フレームごとの RMS・ピーク・スペクトル重心・倍音のパワーの割合
//! - `pitch_track.csv` 約 10 ms ごとの基音推定（参考値。倍音解析は固定基音のまま）
//! - `partials.csv`   同じ時刻の、スペクトルの局所ピークと整数倍との関係（非整数倍の成分を含む）
//! - `overview.png`   波形・スペクトログラム・倍音の時間変化
//! - `summary.txt`    主要な数値・制約・利用者の入力した背景と質問
//!
//! 時間区間を指定したときは、その区間を切り出して同じ条件で解析し直し、切り出した音の一式を書く
//! （時刻 0 = 区間の先頭。元の音源の時刻 = 時刻 + offset）。
//!
//! ZIP は保存先と同じフォルダの一時ファイルに書き、すべて書けてから保存先へ置き換える。
//! 途中で失敗・キャンセルしたら一時ファイルを消し、既存のファイルはそのまま残す。

pub mod features;
mod manifest;
mod overview;
mod summary;
pub mod tracks;

use crate::analysis::{Analysis, HarmonicLevel, WindowSize, analyze};
use crate::audio_file::AudioClip;
use crate::filter::Band;
use crate::pitch::Estimate;
use std::borrow::Cow;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub use overview::render_overview;
pub use summary::{HarmonicStats, harmonic_stats};

/// 出力形式の版。列や項目の意味を変えたら上げる
pub const SCHEMA_VERSION: &str = "1.2.0";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const FILE_WAV: &str = "source.wav";
pub const FILE_MANIFEST: &str = "manifest.json";
pub const FILE_CSV: &str = "harmonics.csv";
pub const FILE_OVERVIEW: &str = "overview.png";
pub const FILE_SUMMARY: &str = "summary.txt";
pub const FILE_FEATURES: &str = "features.csv";
pub const FILE_ENVELOPE: &str = "envelope.csv";
pub const FILE_PITCH: &str = "pitch_track.csv";
pub const FILE_PARTIALS: &str = "partials.csv";
/// ZIP に入るファイル（この順で書く）
pub const FILES: [&str; 9] = [
    FILE_MANIFEST,
    FILE_SUMMARY,
    FILE_FEATURES,
    FILE_ENVELOPE,
    FILE_PITCH,
    FILE_PARTIALS,
    FILE_CSV,
    FILE_WAV,
    FILE_OVERVIEW,
];

/// harmonics.csv の列（この順）
pub const CSV_COLUMNS: [&str; 11] = [
    "frame_index",
    "time_s",
    "harmonic",
    "target_hz",
    "peak_hz",
    "sampled_bin_hz",
    "level_db",
    "status",
    "edge_padded",
    "rel_h1_db",
    "rel_h1_missing",
];

/// features.csv の列（この順）
pub const FEATURE_COLUMNS: [&str; 18] = [
    "harmonic",
    "target_hz",
    "frames",
    "detected_frames",
    "max_level_db",
    "max_time_s",
    "sustain_median_db",
    "sustain_detected_frames",
    "sustain_frames",
    "sustain_missing",
    "rel_h1_median_db",
    "rel_h1_frames",
    "rel_h1_missing",
    "decay_20db_s",
    "decay_20db_status",
    "decay_40db_s",
    "decay_40db_status",
    "decay_reference",
];

/// envelope.csv の列（この順）
pub const ENVELOPE_COLUMNS: [&str; 8] = [
    "frame_index",
    "time_s",
    "rms_db",
    "peak_db",
    "spectral_centroid_hz",
    "harmonic_power_ratio",
    "edge_padded",
    "samples_used",
];

/// 音源の種類
#[derive(Clone, Debug, PartialEq)]
pub enum SourceKind {
    /// WAV ファイル
    File { path: PathBuf },
    /// アプリの中で作るサンプル
    Sample { title: String, about: String },
    /// ライブ入力を止めて残った音（`saved_path` は「WAV に保存」した先）
    LiveRecording { input_name: String, input_channels: usize, saved_path: Option<PathBuf> },
}

/// 基音の指定。`matches_analysis` が false なら、入力欄の値は解析に使われていない（読み取りエラーなど）
#[derive(Clone, Debug, PartialEq)]
pub struct PitchInput {
    pub text: String,
    pub a4_hz: f64,
    pub cents: f64,
    pub matches_analysis: bool,
}

/// 基音推定の状態
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EstimateState {
    /// 推定していない（または基音を手で変えて消えた）
    NotEstimated,
    /// 推定したが、いまの音源で推定したものか保証できない
    Unverified,
    /// いまの音源（書き出す音声そのもの）で推定した
    Current(Estimate),
}

/// 試聴フィルタの種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListenKind {
    Band,
    Harmonics,
}

/// 試聴フィルタの設定（再生音だけに掛かる。書き出す音声・数値・画像には掛からない）
#[derive(Clone, Debug, PartialEq)]
pub struct ListenFilter {
    pub enabled: bool,
    pub kind: ListenKind,
    pub band: Band,
    /// 「選択していない倍音をカット」で各倍音の周りに残す幅（片側、基音に対する %）
    pub harm_width_pct: f64,
}

/// 画面の状態（数値には影響しない）
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    /// 時間変化のグラフに表示する倍音（解析範囲内のもの、昇順）
    pub shown: Vec<usize>,
    pub focused: Option<usize>,
    /// 解析位置（秒、フレームの中心）
    pub cursor_s: f64,
    pub display_floor_db: f32,
    /// 画面のグラフの表示範囲（最後に描いたときのもの）
    pub time_range_s: Option<(f64, f64)>,
    pub freq_range_hz: Option<(f64, f64)>,
    pub level_range_db: Option<(f64, f64)>,
}

/// 特徴量の区間（元の音源の時刻、秒）。持続区間は指定したときだけ使う
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Intervals {
    /// 発音開始（None なら RMS から推定）
    pub onset_s: Option<f64>,
    /// 持続区間（開始, 終了）
    pub sustain_s: Option<(f64, f64)>,
}

/// 切り出した区間（元の音源のサンプル位置）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Excerpt {
    pub start_sample: usize,
    /// 含まない
    pub end_sample: usize,
    pub source_num_samples: usize,
}

/// 利用者が書き出し画面で入力した背景と質問（空なら不明として扱う）
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserNotes {
    pub instrument: String,
    pub technique: String,
    pub question: String,
}

/// 書き出す対象の一式。書き出し開始時点で固定し、以後は画面の状態が変わっても影響しない
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub clip: AudioClip,
    pub analysis: Analysis,
    pub source: SourceKind,
    /// 画面に出していた音源の名前
    pub source_name: String,
    pub pitch: PitchInput,
    pub window_mode: WindowSize,
    pub estimate: EstimateState,
    pub view: ViewState,
    pub listen: ListenFilter,
    pub notes: UserNotes,
    pub intervals: Intervals,
    /// 書き出す区間（元の音源のサンプル位置 [開始, 終了)）。None なら音声全体
    pub range: Option<(usize, usize)>,
    /// 切り出し済みなら、その区間（`prepare` が設定する）
    pub excerpt: Option<Excerpt>,
    pub created: SystemTime,
}

/// 秒で指定した区間を、元の音源のサンプル位置 [開始, 終了) にする。空・逆転・音源外ならエラー
pub fn sample_range(start_s: f64, end_s: f64, sample_rate: u32, num_samples: usize) -> Result<(usize, usize), String> {
    let sr = sample_rate as f64;
    let dur = num_samples as f64 / sr;
    if !(start_s.is_finite() && end_s.is_finite()) {
        return Err("区間の時刻が数値ではありません".into());
    }
    if start_s > end_s {
        return Err("区間の開始が終了より後です".into());
    }
    if start_s < 0.0 || end_s > dur + 0.5 / sr {
        return Err(format!("区間が音源の範囲（0〜{dur:.3} 秒）の外です"));
    }
    let s = (start_s * sr).round() as usize;
    let e = ((end_s * sr).round() as usize).min(num_samples);
    if e <= s {
        return Err("区間が空です".into());
    }
    Ok((s, e))
}

/// 必要なら区間を切り出して解析し直した一式（区間が無ければそのまま）
pub fn prepare(snap: &Snapshot) -> Result<Cow<'_, Snapshot>, String> {
    snap.check()?;
    let Some((s, e)) = snap.range else { return Ok(Cow::Borrowed(snap)) };
    let c = &snap.clip;
    if e > c.mono.len() || s >= e {
        return Err("区間が音源の範囲の外です".into());
    }
    let clip = AudioClip {
        sample_rate: c.sample_rate,
        channels: c.channels,
        interleaved: c.interleaved[s * c.channels..e * c.channels].to_vec(),
        mono: c.mono[s..e].to_vec(),
        format_desc: c.format_desc.clone(),
    };
    // 同じ基音・同じ窓長（サンプル数）で解析し直す
    let a = &snap.analysis;
    let analysis = analyze(&clip.mono, clip.sample_rate, a.f0, WindowSize::Fixed(a.window))?;
    let mut out = snap.clone();
    out.view.shown.retain(|&k| k <= analysis.num_in_range);
    out.clip = clip;
    out.analysis = analysis;
    out.range = None;
    out.excerpt = Some(Excerpt { start_sample: s, end_sample: e, source_num_samples: c.mono.len() });
    out.check()?;
    Ok(Cow::Owned(out))
}

impl Snapshot {
    /// 音声と解析結果が同じものを表しているか（サンプル数・サンプリング周波数）
    pub fn check(&self) -> Result<(), String> {
        let a = &self.analysis;
        let c = &self.clip;
        if a.sample_rate != c.sample_rate || a.num_samples != c.mono.len() {
            return Err("音声と解析結果が一致しません。解析し直してから書き出してください".into());
        }
        if c.channels == 0 || c.interleaved.len() != c.mono.len() * c.channels {
            return Err("音声のチャンネル数とサンプル数が合いません".into());
        }
        if a.num_frames == 0 || a.num_in_range == 0 {
            return Err("解析範囲内の倍音がありません".into());
        }
        Ok(())
    }

    /// 書き出す音の時刻 0 が、元の音源の何秒か
    pub fn offset_s(&self) -> f64 {
        self.excerpt.map_or(0.0, |x| x.start_sample as f64 / self.clip.sample_rate as f64)
    }

    /// 元の音源の時刻 → 書き出す音の時刻
    pub fn out_time(&self, source_t: f64) -> f64 {
        source_t - self.offset_s()
    }

    /// 解析位置のフレーム（書き出す区間の外なら None）
    pub fn cursor_frame(&self) -> Option<usize> {
        let t = self.out_time(self.view.cursor_s);
        let dur = self.clip.duration();
        (t >= -1e-9 && t <= dur + 1e-9).then(|| self.analysis.frame_at(t))
    }

    /// harmonics.csv のデータ行数
    pub fn csv_rows(&self) -> usize {
        self.analysis.num_frames * self.analysis.num_in_range
    }
}

/// 書き出しの進み具合とキャンセル（別スレッドと共有する）
#[derive(Debug, Default)]
pub struct Progress {
    cancel: AtomicBool,
    /// 0..=1000
    permille: AtomicU32,
}

impl Progress {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn fraction(&self) -> f32 {
        self.permille.load(Ordering::Relaxed) as f32 / 1000.0
    }

    fn set(&self, f: f64) {
        self.permille.store((f.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    }
}

#[derive(Debug, PartialEq)]
pub enum ExportError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Cancelled => write!(f, "キャンセルしました"),
            ExportError::Failed(e) => write!(f, "{e}"),
        }
    }
}

fn fail(what: &str) -> impl Fn(std::io::Error) -> ExportError + '_ {
    move |e| ExportError::Failed(format!("{what}: {e}"))
}

fn zip_fail(e: zip::result::ZipError) -> ExportError {
    ExportError::Failed(format!("ZIP の書き込みに失敗しました: {e}"))
}

/// 一式を ZIP にして `dest` に保存する。
/// `cjk_font` は画像に日本語（音源名など）を描くためのフォント（無ければ英数字だけで描く）。
pub fn export_zip(snap: &Snapshot, dest: &Path, cjk_font: Option<&[u8]>, progress: &Progress) -> Result<(), ExportError> {
    let prepared = prepare(snap).map_err(ExportError::Failed)?;
    let snap = prepared.as_ref();
    let dir = dest.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = dest.file_name().ok_or_else(|| ExportError::Failed("保存先のファイル名がありません".into()))?;
    let tmp = dir.join(format!(".{}.partial-{}", name.to_string_lossy(), std::process::id()));
    let result = write_zip_file(snap, &tmp, cjk_font, progress).and_then(|()| {
        if progress.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        // 同じフォルダ内の置き換えなので、既存のファイルは完成品と一度に入れ替わる
        std::fs::rename(&tmp, dest).map_err(fail("保存先に置けませんでした"))
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn write_zip_file(snap: &Snapshot, path: &Path, cjk_font: Option<&[u8]>, progress: &Progress) -> Result<(), ExportError> {
    let file = std::fs::File::create(path).map_err(fail("保存先に書き込めません"))?;
    let mut zip = zip::ZipWriter::new(BufWriter::new(file));
    let time = zip_time(snap.created);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(time);
    let stored = deflated.compression_method(zip::CompressionMethod::Stored);
    progress.set(0.0);

    let feats = features::compute(snap);
    let tracks = tracks::compute(snap, progress)?;
    progress.set(0.08);
    zip.start_file(FILE_MANIFEST, deflated).map_err(zip_fail)?;
    let json = serde_json::to_string_pretty(&manifest::build(snap, &feats, &tracks))
        .map_err(|e| ExportError::Failed(e.to_string()))?;
    zip.write_all(json.as_bytes()).map_err(fail("manifest.json"))?;

    zip.start_file(FILE_SUMMARY, deflated).map_err(zip_fail)?;
    zip.write_all(summary::build(snap, &feats, &tracks).as_bytes()).map_err(fail("summary.txt"))?;

    zip.start_file(FILE_FEATURES, deflated).map_err(zip_fail)?;
    write_features_csv(snap, &feats, &mut zip)?;
    zip.start_file(FILE_ENVELOPE, deflated).map_err(zip_fail)?;
    write_envelope_csv(snap, &feats, &mut zip)?;
    zip.start_file(FILE_PITCH, deflated).map_err(zip_fail)?;
    tracks::write_pitch_csv(snap, &tracks, &mut zip)?;
    let big_partials = tracks.partials.rows.len() as u64 * 160 > u32::MAX as u64;
    zip.start_file(FILE_PARTIALS, deflated.large_file(big_partials)).map_err(zip_fail)?;
    tracks::write_partials_csv(snap, &tracks, &mut zip)?;
    progress.set(0.12);

    // CSV は 1 行 100 バイト前後。4 GiB を超えそうなら ZIP64 にする
    let big = snap.csv_rows() as u64 * 120 > u32::MAX as u64;
    zip.start_file(FILE_CSV, deflated.large_file(big)).map_err(zip_fail)?;
    write_csv(snap, &mut zip, |f| progress.set(0.12 + 0.58 * f), progress)?;

    zip.start_file(FILE_WAV, stored.large_file(snap.clip.interleaved.len() as u64 * 4 > u32::MAX as u64 / 2))
        .map_err(zip_fail)?;
    write_wav_f32(&mut zip, &snap.clip.interleaved, snap.clip.channels, snap.clip.sample_rate, progress)?;
    progress.set(0.85);

    let png = render_overview(snap, &feats, cjk_font).map_err(ExportError::Failed)?;
    if progress.is_cancelled() {
        return Err(ExportError::Cancelled);
    }
    zip.start_file(FILE_OVERVIEW, stored).map_err(zip_fail)?;
    zip.write_all(&png).map_err(fail("overview.png"))?;

    let mut out = zip.finish().map_err(zip_fail)?;
    out.flush().map_err(fail("保存先に書き込めません"))?;
    let file = out.into_inner().map_err(|e| ExportError::Failed(format!("保存先に書き込めません: {}", e.error())))?;
    file.sync_all().map_err(fail("保存先に書き込めません"))?;
    progress.set(1.0);
    Ok(())
}

/// CSV の数値。表示用に丸めず、値を復元できる最短の 10 進表記で書く（NaN・無限大は空欄）
fn num(out: &mut String, v: f64) {
    use std::fmt::Write as _;
    if v.is_finite() {
        let _ = write!(out, "{v}");
    }
}

fn num32(out: &mut String, v: f32) {
    use std::fmt::Write as _;
    if v.is_finite() {
        let _ = write!(out, "{v}");
    }
}

/// harmonics.csv を書く（1 行 = 1 フレームの 1 倍音、フレーム → 倍音番号の順）
pub fn write_csv(
    snap: &Snapshot,
    w: &mut impl Write,
    mut on_progress: impl FnMut(f64),
    progress: &Progress,
) -> Result<(), ExportError> {
    let a = &snap.analysis;
    let mut w = BufWriter::with_capacity(1 << 16, w);
    let io = fail("harmonics.csv");
    w.write_all(CSV_COLUMNS.join(",").as_bytes()).map_err(&io)?;
    w.write_all(b"\n").map_err(&io)?;
    let mut line = String::with_capacity(128);
    for f in 0..a.num_frames {
        if f % 256 == 0 {
            if progress.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            on_progress(f as f64 / a.num_frames as f64);
        }
        let t = a.frame_time(f);
        let edge = a.is_edge_padded(f);
        for k in 1..=a.num_in_range {
            let HarmonicLevel::Level { db, peak_hz, sampled_bin_hz, status } = a.level(f, k) else {
                continue; // 範囲内だけを回しているので来ない
            };
            line.clear();
            use std::fmt::Write as _;
            let _ = write!(line, "{f},");
            num(&mut line, t);
            let _ = write!(line, ",{k},");
            num(&mut line, a.harmonic_hz(k));
            line.push(',');
            if let Some(p) = peak_hz {
                num32(&mut line, p);
            }
            line.push(',');
            if let Some(b) = sampled_bin_hz {
                num32(&mut line, b);
            }
            line.push(',');
            num32(&mut line, db);
            let _ = write!(line, ",{},{},", status.as_str(), edge);
            match features::relative_to_h1(a, f, k) {
                Ok(v) => {
                    num32(&mut line, v);
                    line.push_str(",\n");
                }
                Err(r) => {
                    let _ = writeln!(line, ",{}", r.as_str());
                }
            }
            w.write_all(line.as_bytes()).map_err(&io)?;
        }
    }
    w.flush().map_err(&io)?;
    on_progress(1.0);
    Ok(())
}

fn opt(out: &mut String, v: Option<f64>) {
    if let Some(v) = v {
        num(out, v);
    }
}

/// features.csv（1 行 = 1 倍音）
fn write_features_csv(snap: &Snapshot, feats: &features::Features, w: &mut impl Write) -> Result<(), ExportError> {
    use std::fmt::Write as _;
    let a = &snap.analysis;
    let mut o = FEATURE_COLUMNS.join(",");
    o.push('\n');
    for h in &feats.harmonics {
        let _ = write!(o, "{},", h.harmonic);
        num(&mut o, a.harmonic_hz(h.harmonic));
        let _ = write!(o, ",{},{},", h.frames, h.detected);
        opt(&mut o, h.max.map(|m| m.0 as f64));
        o.push(',');
        opt(&mut o, h.max.map(|m| m.1));
        o.push(',');
        opt(&mut o, h.sustain.median_db);
        let _ = write!(o, ",{},{},{},", h.sustain.detected, h.sustain.frames, h.sustain.missing.unwrap_or(""));
        opt(&mut o, h.rel_h1_median_db);
        let _ = write!(o, ",{},{},", h.rel_h1_frames, h.rel_h1_missing.unwrap_or(""));
        for d in h.decay {
            opt(&mut o, d.seconds());
            let _ = write!(o, ",{},", d.status());
        }
        o.push_str("max_level\n");
    }
    w.write_all(o.as_bytes()).map_err(fail("features.csv"))
}

/// envelope.csv（1 行 = 1 フレーム）
fn write_envelope_csv(snap: &Snapshot, feats: &features::Features, w: &mut impl Write) -> Result<(), ExportError> {
    use std::fmt::Write as _;
    let a = &snap.analysis;
    let e = &feats.envelope;
    let mut o = ENVELOPE_COLUMNS.join(",");
    o.push('\n');
    for f in 0..a.num_frames {
        let _ = write!(o, "{f},");
        num(&mut o, a.frame_time(f));
        o.push(',');
        num32(&mut o, e.rms_db[f]);
        o.push(',');
        num32(&mut o, e.peak_db[f]);
        o.push(',');
        opt(&mut o, a.spectral_centroid(f));
        o.push(',');
        opt(&mut o, a.harmonic_power_ratio(f));
        let _ = writeln!(o, ",{},{}", a.is_edge_padded(f), e.samples_used[f]);
    }
    w.write_all(o.as_bytes()).map_err(fail("envelope.csv"))
}

/// 32-bit float の WAV を逐次書く（ヘッダーの大きさは先に決まるので、シークは要らない）。
/// 3 チャンネル以上は WAVE_FORMAT_EXTENSIBLE（スピーカー配置は指定しない）。
pub fn write_wav_f32(
    w: &mut impl Write,
    interleaved: &[f32],
    channels: usize,
    sample_rate: u32,
    progress: &Progress,
) -> Result<(), ExportError> {
    let io = fail("source.wav");
    let data_len = interleaved.len() as u64 * 4;
    let frames = interleaved.len() / channels.max(1);
    let extensible = channels > 2;
    let fmt_len: u32 = if extensible { 40 } else { 18 };
    let riff_len = 4 + (8 + fmt_len as u64) + (8 + 4) + (8 + data_len);
    if riff_len > u32::MAX as u64 {
        return Err(ExportError::Failed("音声が長すぎて WAV（4 GiB まで）に入りません".into()));
    }
    let block = (channels * 4) as u16;
    let mut h = Vec::with_capacity(80);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(riff_len as u32).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&fmt_len.to_le_bytes());
    h.extend_from_slice(&(if extensible { 0xFFFEu16 } else { 3u16 }).to_le_bytes());
    h.extend_from_slice(&(channels as u16).to_le_bytes());
    h.extend_from_slice(&sample_rate.to_le_bytes());
    h.extend_from_slice(&(sample_rate * block as u32).to_le_bytes());
    h.extend_from_slice(&block.to_le_bytes());
    h.extend_from_slice(&32u16.to_le_bytes());
    if extensible {
        h.extend_from_slice(&22u16.to_le_bytes()); // 拡張部の長さ
        h.extend_from_slice(&32u16.to_le_bytes()); // 有効ビット数
        h.extend_from_slice(&0u32.to_le_bytes()); // スピーカー配置なし
        // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        h.extend_from_slice(&[0x03, 0, 0, 0, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
    } else {
        h.extend_from_slice(&0u16.to_le_bytes());
    }
    h.extend_from_slice(b"fact");
    h.extend_from_slice(&4u32.to_le_bytes());
    h.extend_from_slice(&(frames as u32).to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(data_len as u32).to_le_bytes());
    w.write_all(&h).map_err(&io)?;
    let mut buf = Vec::with_capacity(1 << 16);
    for chunk in interleaved.chunks(1 << 14) {
        if progress.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        buf.clear();
        chunk.iter().for_each(|s| buf.extend_from_slice(&s.to_le_bytes()));
        w.write_all(&buf).map_err(&io)?;
    }
    Ok(())
}

/// UTC の日時（年, 月, 日, 時, 分, 秒）
pub fn utc_parts(t: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // 暦日への変換（Howard Hinnant の civil_from_days）
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32, (rem % 60) as u32)
}

/// ISO 8601（UTC）
pub fn utc_iso(t: SystemTime) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(t);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// ローカル時刻の（年, 月, 日, 時, 分, 秒）。ZIP の日時はローカル時刻で記録するのが慣例のため
fn local_parts(t: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    #[cfg(target_os = "macos")]
    {
        let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as libc::time_t).unwrap_or(0);
        // SAFETY: localtime_r は渡した tm にだけ書き込む
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
            return (
                tm.tm_year as i64 + 1900,
                tm.tm_mon as u32 + 1,
                tm.tm_mday as u32,
                tm.tm_hour as u32,
                tm.tm_min as u32,
                tm.tm_sec as u32,
            );
        }
    }
    utc_parts(t)
}

fn zip_time(t: SystemTime) -> zip::DateTime {
    let (y, mo, d, h, mi, s) = local_parts(t);
    zip::DateTime::from_date_and_time(y.clamp(1980, 2107) as u16, mo as u8, d as u8, h as u8, mi as u8, s as u8)
        .unwrap_or_default()
}

/// 音源名から既定の ZIP ファイル名を作る（拡張子を除き、ファイル名に使えない文字を置き換える）
pub fn default_zip_name(source_name: &str) -> String {
    let stem = Path::new(source_name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let clean: String = stem
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let clean = clean.trim().trim_start_matches('.');
    format!("{}_ai_export.zip", if clean.is_empty() { "sound" } else { clean })
}

#[cfg(test)]
mod tests;
