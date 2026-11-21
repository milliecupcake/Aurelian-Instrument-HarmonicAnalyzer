//! summary.txt（主要な数値・解析上の制約・利用者の入力した背景と質問）。
//!
//! 数値はすべて harmonics.csv と manifest.json から確かめられるものだけを書く。
//! 音源の種類や音の原因は推測しない。

use super::*;
use crate::analysis::{FLOOR_DB, PeakStatus};
use std::fmt::Write as _;

/// 要約に載せる倍音の最大数（表示 ON の倍音はこれより先でも載せる）
const LIST_FIRST: usize = 16;

/// 1 本の倍音の集計。集計区間は窓が音声に収まるフレーム（edge_padded でないもの）、
/// 有効な検出は status が detected_peak の値
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HarmonicStats {
    pub harmonic: usize,
    /// 集計区間のフレーム数
    pub frames: usize,
    /// そのうちピークを検出したフレーム数
    pub detected: usize,
    /// 検出したフレームでの最大レベル（dB）とその時刻（秒）
    pub max: Option<(f32, f64)>,
}

/// 集計区間（edge_padded でないフレームの範囲）。無ければ None
pub fn stats_frames(a: &Analysis) -> Option<(usize, usize)> {
    let first = (0..a.num_frames).find(|&f| !a.is_edge_padded(f))?;
    let last = (0..a.num_frames).rev().find(|&f| !a.is_edge_padded(f))?;
    Some((first, last))
}

pub fn harmonic_stats(a: &Analysis, k: usize) -> HarmonicStats {
    let mut s = HarmonicStats { harmonic: k, frames: 0, detected: 0, max: None };
    let Some((first, last)) = stats_frames(a) else { return s };
    for f in first..=last {
        s.frames += 1;
        if let HarmonicLevel::Level { db, status: PeakStatus::DetectedPeak, .. } = a.level(f, k) {
            s.detected += 1;
            if s.max.is_none_or(|(m, _)| db > m) {
                s.max = Some((db, a.frame_time(f)));
            }
        }
    }
    s
}

fn or_unknown(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() { "（未入力・不明）".into() } else { t.replace('\n', "\n    ") }
}

fn db_text(db: f32) -> String {
    if db <= FLOOR_DB { format!("≤ {FLOOR_DB:.0} dB（下限）") } else { format!("{db:.2} dB") }
}

/// 要約に載せる倍音（第1〜第16倍音と、表示 ON の倍音）
pub fn listed_harmonics(snap: &Snapshot) -> Vec<usize> {
    let n = snap.analysis.num_in_range;
    let mut ks: Vec<usize> = (1..=n.min(LIST_FIRST)).collect();
    ks.extend(snap.view.shown.iter().copied().filter(|&k| k > LIST_FIRST && k <= n));
    ks
}

fn opt_db(v: Option<f64>) -> String {
    v.map(|d| format!("{d:.2} dB")).unwrap_or_else(|| "—".into())
}

fn decay_text(d: features::Decay) -> String {
    match d {
        features::Decay::Reached(s) => format!("{s:.3} s"),
        features::Decay::NotReached => "未到達".into(),
        features::Decay::ThresholdBelowFloor => "判定不能".into(),
        features::Decay::NoPeak => "—".into(),
    }
}

pub(super) fn build(snap: &Snapshot, feats: &features::Features, tr: &tracks::Tracks) -> String {
    let a = &snap.analysis;
    let c = &snap.clip;
    let sr = a.sample_rate as f64;
    let mut o = String::new();

    let _ = writeln!(o, "倍音解析エディタ 書き出しの要約");
    let _ = writeln!(o, "（{}、schema_version {}、アプリ {}）", utc_iso(snap.created), SCHEMA_VERSION, APP_VERSION);
    o.push('\n');

    let _ = writeln!(o, "■ 音源");
    let kind = match &snap.source {
        SourceKind::File { .. } => "WAV ファイル".to_string(),
        SourceKind::Sample { .. } => "アプリ内のサンプル（試験音）".to_string(),
        SourceKind::LiveRecording { input_name, .. } => format!("ライブ入力の録音（入力元: {input_name}）"),
    };
    let _ = writeln!(o, "  名前: {}", snap.source_name);
    let _ = writeln!(o, "  種類: {kind}");
    let _ = writeln!(
        o,
        "  長さ: {:.3} 秒（{} サンプル/チャンネル）、{} Hz、{} ch",
        c.duration(),
        c.mono.len(),
        c.sample_rate,
        c.channels
    );
    match snap.excerpt {
        None => {
            let _ = writeln!(o, "  書き出した範囲: 音声全体");
        }
        Some(x) => {
            let off = snap.offset_s();
            let _ = writeln!(
                o,
                "  書き出した範囲: 元の音源の {:.3}〜{:.3} 秒（サンプル {}〜{}、終了は含まない）。元の長さ {:.3} 秒",
                off,
                x.end_sample as f64 / sr,
                x.start_sample,
                x.end_sample,
                x.source_num_samples as f64 / sr
            );
            let _ = writeln!(o, "    このファイル一式の時刻 0 = 区間の先頭。元の音源の時刻 = 時刻 + {off:.6} 秒");
            let _ = writeln!(o, "    切り出した音を同じ基音・窓長で解析し直した（端のフレームは edge_padded を付け直している）");
        }
    }
    o.push('\n');

    let _ = writeln!(o, "■ 利用者の入力（任意）");
    let _ = writeln!(o, "  音源・楽器: {}", or_unknown(&snap.notes.instrument));
    let _ = writeln!(o, "  奏法・エフェクト: {}", or_unknown(&snap.notes.technique));
    let _ = writeln!(o, "  調べたいこと: {}", or_unknown(&snap.notes.question));
    o.push('\n');

    let _ = writeln!(o, "■ 解析条件の要点（詳細は manifest.json）");
    let _ = writeln!(o, "  解析に使った基音: {:.4} Hz（録音全体で固定）", a.f0);
    if snap.pitch.matches_analysis {
        let _ = writeln!(
            o,
            "    入力: 「{}」、A4 = {} Hz、補正 {:+.1} cent",
            snap.pitch.text, snap.pitch.a4_hz, snap.pitch.cents
        );
    } else {
        let _ = writeln!(o, "    入力欄の値は解析に使われていない（読み取りエラーなど）");
    }
    match snap.estimate {
        EstimateState::Current(e) => {
            let _ = writeln!(
                o,
                "  基音の推定（YIN）: {:.3} Hz、確からしさ {:.2}（周期を検出できた区間の割合。正解の確率ではない）",
                e.hz, e.confidence
            );
        }
        EstimateState::Unverified => {
            let _ = writeln!(o, "  基音の推定: この音声で推定したものか保証できないため記録しない");
        }
        EstimateState::NotEstimated => {
            let _ = writeln!(o, "  基音の推定: していない");
        }
    }
    let _ = writeln!(
        o,
        "  窓: 4 項 Blackman-Harris、{} サンプル（{:.1} ms、{}）、hop {} サンプル（{:.2} ms）、FFT {}（2 倍ゼロ詰め、1 ビン {:.3} Hz）",
        a.window,
        a.window as f64 / sr * 1000.0,
        match snap.window_mode {
            WindowSize::Auto => "自動",
            WindowSize::Fixed(_) => "固定",
        },
        a.hop,
        a.hop as f64 / sr * 1000.0,
        a.fft_size,
        a.bin_hz()
    );
    let _ = writeln!(o, "  時刻: 窓の中心。フレーム数 {}", a.num_frames);
    let _ = writeln!(o, "  レベル: 振幅 1.0 の正弦波 = 0 dB（20 log10）、下限 {FLOOR_DB:.0} dB");
    let _ = writeln!(
        o,
        "  解析範囲: 第1〜第{}倍音（{}、ナイキスト {:.1} Hz）",
        a.num_in_range,
        if a.num_in_range < crate::analysis::MAX_HARMONICS { "これより上はナイキスト以上" } else { "実装上限" },
        a.nyquist()
    );
    o.push('\n');

    let _ = writeln!(o, "■ ファイルの読み方");
    let _ = writeln!(o, "  harmonics.csv  1 行 = 1 フレームの 1 倍音（{} 行）。表示 OFF の倍音も含む全倍音", snap.csv_rows());
    let _ = writeln!(o, "                 rel_h1_db = 基音に対する相対レベル（両方 detected_peak のときだけ。無ければ rel_h1_missing に理由）");
    let _ = writeln!(o, "                 status: detected_peak（局所ピークを検出）／sampled_target_bin（検出できず目標に近いビンの値）／below_floor（下限以下）");
    let _ = writeln!(o, "                 peak_hz は detected_peak のときだけ。edge_padded = true は窓が音声の端をまたぐフレーム");
    let _ = writeln!(o, "  source.wav     試聴フィルタ適用前の音声（32-bit float、{} ch）", c.channels);
    let _ = writeln!(o, "  overview.png   波形・スペクトログラム・表示 ON の倍音の時間変化（フィルタ適用前の音）");
    let _ = writeln!(o, "  features.csv   倍音ごとの最大・持続区間の代表値・相対レベル・減衰時間");
    let _ = writeln!(o, "  envelope.csv   フレームごとの RMS・ピーク・スペクトル重心・倍音のパワーの割合");
    let _ = writeln!(o, "  pitch_track.csv 約 10 ms ごとの基音推定（参考値。倍音の値には使っていない）");
    let _ = writeln!(
        o,
        "  partials.csv   同じ時刻のスペクトルの局所ピーク。class = harmonic / inharmonic（整数倍の間）/ below_f0"
    );
    let _ = writeln!(o, "  manifest.json  解析条件・列の定義・表示と試聴の状態");
    o.push('\n');

    let _ = writeln!(o, "■ 倍音ごとの最大レベル");
    match stats_frames(a) {
        None => {
            let _ = writeln!(o, "  集計対象なし: 窓（{:.1} ms）が音声に収まるフレームが無い（音が短すぎる）", a.window as f64 / sr * 1000.0);
        }
        Some((first, last)) => {
            let _ = writeln!(
                o,
                "  集計区間: {:.3}〜{:.3} 秒（edge_padded でないフレーム {} 個）。有効な検出 = status が detected_peak の値",
                a.frame_time(first),
                a.frame_time(last),
                last - first + 1
            );
            let _ = writeln!(o, "  全期間の最大値で、持続音の代表値ではない");
            let _ = writeln!(o, "  倍音   対象周波数      最大レベル    その時刻    検出フレーム");
            for k in listed_harmonics(snap) {
                let s = harmonic_stats(a, k);
                let (max, at) = match s.max {
                    Some((db, t)) => (db_text(db), format!("{t:.3} s")),
                    None => ("検出なし".into(), "—".into()),
                };
                let _ = writeln!(
                    o,
                    "  第{k:<3}  {:>10.2} Hz  {:>12}  {:>9}   {}/{}",
                    a.harmonic_hz(k),
                    max,
                    at,
                    s.detected,
                    s.frames
                );
            }
        }
    }
    o.push('\n');

    let _ = writeln!(o, "■ 特徴量（features.csv・envelope.csv）");
    match &feats.onset {
        features::Onset::Manual { t, .. } => {
            let _ = writeln!(o, "  発音開始: {t:.3} 秒（利用者の指定）");
        }
        features::Onset::Auto { t, max_rms_db, .. } => {
            let _ = writeln!(
                o,
                "  発音開始: {t:.3} 秒（推定: RMS が最大 {max_rms_db:.2} dB から {} dB 以内に初めて入るフレーム）",
                features::ONSET_WITHIN_DB
            );
        }
        features::Onset::Unavailable(r) => {
            let _ = writeln!(o, "  発音開始: 決められない（{r}）");
        }
    }
    match (snap.intervals.sustain_s, feats.sustain_frames) {
        (None, _) => {
            let _ = writeln!(o, "  持続区間: 未指定（代表レベルは出していない）");
        }
        (Some((s0, s1)), Some((f0, f1))) => {
            let _ = writeln!(
                o,
                "  持続区間: {:.3}〜{:.3} 秒（利用者の指定。フレーム {f0}〜{f1}）",
                snap.out_time(s0),
                snap.out_time(s1)
            );
        }
        (Some((s0, s1)), None) => {
            let _ = writeln!(
                o,
                "  持続区間: {:.3}〜{:.3} 秒を指定したが、窓が音声に収まるフレームが無い",
                snap.out_time(s0),
                snap.out_time(s1)
            );
        }
    }
    let env = &feats.envelope;
    if let Some((first, last)) = feats.valid {
        let (fm, rm) = (first..=last).map(|f| (f, env.rms_db[f])).fold((first, f32::MIN), |b, x| if x.1 > b.1 { x } else { b });
        let _ = writeln!(o, "  RMS の最大: {} （{:.3} 秒）", db_text(rm), a.frame_time(fm));
        let cents: Vec<f64> = (first..=last).filter_map(|f| a.spectral_centroid(f)).collect();
        if let Some(m) = {
            let mut c = cents.clone();
            c.sort_by(f64::total_cmp);
            c.get(c.len() / 2).copied()
        } {
            let _ = writeln!(o, "  スペクトル重心の中央値: {m:.1} Hz（{} フレーム）", cents.len());
        }
    }
    let rel_scope = if feats.sustain_frames.is_some() { "持続区間" } else { "発音開始以降" };
    let _ = writeln!(
        o,
        "  倍音   持続区間の中央値   基音比の中央値（{rel_scope}）   −20 dB まで   −40 dB まで"
    );
    for k in listed_harmonics(snap) {
        let Some(h) = feats.harmonics.get(k - 1) else { continue };
        let rel = match h.rel_h1_median_db {
            Some(v) => format!("{v:+.2} dB"),
            None => format!("—（{}）", h.rel_h1_missing.unwrap_or("")),
        };
        let _ = writeln!(
            o,
            "  第{k:<3}  {:>14}   {:>22}   {:>10}   {:>10}",
            opt_db(h.sustain.median_db),
            rel,
            decay_text(h.decay[0]),
            decay_text(h.decay[1])
        );
    }
    let _ = writeln!(o, "  減衰は各倍音の最大の時刻からの秒数。「未到達」は観測の終わりまで下回らなかったもの（観測外は推測しない）");
    o.push('\n');

    let ps = &tr.pitch.stats;
    let total = tr.pitch.rows.len();
    let _ = writeln!(o, "■ 基音の時間変化（pitch_track.csv、参考値）");
    let _ = writeln!(
        o,
        "  倍音解析は固定基音 {:.4} Hz のまま。下の推定値は harmonics.csv・features.csv・partials.csv には使っていない",
        a.f0
    );
    let _ = writeln!(
        o,
        "  {} 時刻（{:.1} ms おき）: 周期を検出 {}、周期がはっきりしない {}、音が小さい {}、端で推定しない {}",
        total,
        tr.step as f64 * a.hop as f64 / sr * 1000.0,
        ps.voiced,
        ps.unvoiced,
        ps.quiet,
        ps.edge
    );
    match (ps.median_hz, ps.cents_p05, ps.cents_median, ps.cents_p95, ps.max_abs_cents) {
        (Some(hz), Some(lo), Some(mid), Some(hi), Some(mx)) => {
            let _ = writeln!(
                o,
                "  検出した基音の中央値 {hz:.3} Hz。固定基音からのずれ: 中央値 {mid:+.1} cent、5〜95% 点 {lo:+.1}〜{hi:+.1} cent、最大 {mx:.1} cent"
            );
            if tr.pitch.deviates() {
                let _ = writeln!(
                    o,
                    "  ※ 固定基音とのずれが大きい。高次の倍音ほど探索範囲から外れ、値が実際より小さく出ている可能性がある"
                );
                let _ = writeln!(
                    o,
                    "    ただし推定値が固定基音の整数倍・整数分の 1 に近いときは、オクターブの取り違えや、非整数倍の成分による低い共通周期のこともある"
                );
            }
        }
        _ => {
            let _ = writeln!(o, "  周期を検出できた時刻が無い");
        }
    }
    let _ = writeln!(
        o,
        "  オクターブの取り違えは補正していない。窓（{:.1} ms）より速い変化は平均される",
        crate::pitch::track_window(a.sample_rate) as f64 / sr * 1000.0
    );
    o.push('\n');

    let pt = &tr.partials;
    let _ = writeln!(o, "■ 倍音以外の成分（partials.csv）");
    let _ = writeln!(
        o,
        "  {} フレームで、最大から {} dB 以内の局所ピークを最大 {} 個ずつ拾った（{} 行）",
        pt.frames_examined,
        tracks::PEAK_RANGE_DB,
        tracks::MAX_PEAKS_PER_FRAME,
        pt.rows.len()
    );
    let _ = writeln!(
        o,
        "  整数倍 × {:.4} Hz から ±{:.2} Hz 以内を harmonic、それ以外を inharmonic（基音より下は below_f0）とした",
        a.f0, pt.half_width_hz
    );
    if pt.recurring.is_empty() {
        let _ = writeln!(
            o,
            "  調べたフレームの {:.0}% 以上に繰り返し現れる非整数倍のピーク: なし",
            tracks::RECURRING_MIN_SHARE * 100.0
        );
    } else {
        let _ = writeln!(
            o,
            "  調べたフレームの {:.0}% 以上に繰り返し現れる非整数倍のピーク（強い順）:",
            tracks::RECURRING_MIN_SHARE * 100.0
        );
        for r in &pt.recurring {
            let _ = writeln!(
                o,
                "    {:>10.2} Hz（{:.2}〜{:.2}）  中央値 {:>8.2} dB  {} フレーム（{:.0}%）  第{}倍音 {:.2} Hz から {:+.2} Hz  {}",
                r.median_hz,
                r.min_hz,
                r.max_hz,
                r.median_db,
                r.frames,
                r.share * 100.0,
                r.nearest,
                a.harmonic_hz(r.nearest),
                r.dev_hz,
                r.class.as_str()
            );
        }
    }
    if let Some((first, last)) = feats.valid {
        let mut ratios: Vec<f64> = (first..=last).filter_map(|f| a.harmonic_power_ratio(f)).collect();
        ratios.sort_by(f64::total_cmp);
        if let Some(m) = ratios.get(ratios.len() / 2) {
            let _ = writeln!(
                o,
                "  検出した倍音のパワーの割合（envelope.csv の harmonic_power_ratio、中央値）: {m:.3}（倍音だけの定常音で約 1。雑音だけでも 0 にはならない）"
            );
        }
    }
    let _ =
        writeln!(o, "  局所ピークは楽器由来とは限らない（雑音の山・隣の成分の裾）。音源の種類や原因は判断していない");
    o.push('\n');

    match snap.cursor_frame() {
        Some(f) => {
            let _ = writeln!(
                o,
                "■ 解析位置 {:.3} 秒（フレーム {f}{}）の値",
                a.frame_time(f),
                if a.is_edge_padded(f) { "、edge_padded" } else { "" }
            );
            for k in listed_harmonics(snap) {
                if let HarmonicLevel::Level { db, peak_hz, status, .. } = a.level(f, k) {
                    let peak = peak_hz.map(|p| format!("ピーク {p:.2} Hz")).unwrap_or_else(|| "ピーク未検出".into());
                    let _ = writeln!(o, "  第{k:<3}  {:>12}  {}  {}", db_text(db), status.as_str(), peak);
                }
            }
        }
        None => {
            let _ = writeln!(o, "■ 解析位置（元の音源の {:.3} 秒）は書き出した区間の外", snap.view.cursor_s);
        }
    }
    o.push('\n');

    let _ = writeln!(o, "■ 表示と試聴の状態（数値には影響しない）");
    let shown = &snap.view.shown;
    if shown.is_empty() {
        let _ = writeln!(o, "  表示 ON の倍音: なし（すべて OFF。数値は harmonics.csv に全倍音ある）");
    } else {
        let list: Vec<String> = shown.iter().map(|k| k.to_string()).collect();
        let _ = writeln!(o, "  表示 ON の倍音: {}", list.join(", "));
    }
    let _ = writeln!(
        o,
        "  注目中の倍音: {}",
        snap.view.focused.map(|k| format!("第{k}倍音")).unwrap_or_else(|| "なし".into())
    );
    let l = &snap.listen;
    let filter = if !l.enabled {
        "オフ".to_string()
    } else {
        match l.kind {
            ListenKind::Band => format!(
                "{:.1}–{:.1} Hz {}",
                l.band.low_hz,
                l.band.high_hz,
                match l.band.mode {
                    crate::filter::BandMode::Keep => "だけ聴く",
                    crate::filter::BandMode::Remove => "を除く",
                }
            ),
            ListenKind::Harmonics => {
                format!("表示 ON の倍音の周り（± 基音の {:.0}%）だけ聴く", l.harm_width_pct)
            }
        }
    };
    let _ = writeln!(o, "  試聴フィルタ: {filter}（再生音だけ。書き出したデータはすべてフィルタ適用前）");
    o.push('\n');

    let _ = writeln!(o, "■ 解釈するときの制約");
    let _ = writeln!(
        o,
        "  ・倍音解析の基音は録音全体で固定。時刻ごとの基音は pitch_track.csv に参考値として別に出しただけ"
    );
    let _ = writeln!(o, "  ・全チャンネルの平均（モノラル）を解析。チャンネル間の差や逆相成分は数値に現れない");
    let _ = writeln!(o, "  ・値は周辺ピークの振幅で、帯域のエネルギーではない。位相は出力していない");
    let _ = writeln!(o, "  ・detected_peak でも楽器由来の倍音とは限らない。below_floor の真のレベルは不明");
    let _ = writeln!(
        o,
        "  ・非整数倍のピークは partials.csv にある（harmonics.csv には無い）。雑音は帯域のパワーの割合でしか表していない"
    );
    let _ = writeln!(
        o,
        "  ・位相とスペクトル全体は出力していない。倍音の振幅だけから元の波形や発音方式は一意に復元できない"
    );
    let _ = writeln!(o, "  ・音源の種類や音の原因は、利用者の入力以外からは判断していない");
    o
}
