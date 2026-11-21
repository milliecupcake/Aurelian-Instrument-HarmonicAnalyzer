//! manifest.json（解析条件・音源情報・表示と試聴の状態・データ形式の説明）。
//!
//! キーと列挙値は英語、説明文は日本語。不明な値は null（0 とは区別する）。NaN・無限大は書かない。

use super::*;
use crate::analysis::{FLOOR_DB, MAX_HARMONICS, SEARCH_TOLERANCE, auto_window};
use crate::filter::BandMode;
use serde_json::{Value, json};

/// 有限なら数値、そうでなければ null
pub(super) fn n(v: f64) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
}

fn n32(v: f32) -> Value {
    n(v as f64)
}

fn text_or_null(s: &str) -> Value {
    let t = s.trim();
    if t.is_empty() { Value::Null } else { json!(t) }
}

fn range(r: Option<(f64, f64)>) -> Value {
    match r {
        Some((a, b)) if a.is_finite() && b.is_finite() => json!([a, b]),
        _ => Value::Null,
    }
}

pub(super) fn build(snap: &Snapshot, feats: &features::Features, tr: &tracks::Tracks) -> Value {
    let a = &snap.analysis;
    let c = &snap.clip;
    let sr = a.sample_rate as f64;
    let cursor_frame = snap.cursor_frame();

    let source = match &snap.source {
        SourceKind::File { path } => json!({
            "kind": "file",
            "name": snap.source_name,
            "path": path.to_string_lossy(),
            "input_format": format!("WAV（{}、{} ch）", c.format_desc, c.channels),
            "channels_note": "元のチャンネル数・順序のまま読み込んだ",
        }),
        SourceKind::Sample { title, about } => json!({
            "kind": "sample",
            "name": snap.source_name,
            "path": null,
            "sample_title": title,
            "sample_description": about,
            "input_format": "アプリの中で生成した試験音（32-bit float、モノラル）",
            "channels_note": "モノラルで生成した",
        }),
        SourceKind::LiveRecording { input_name, input_channels, saved_path } => json!({
            "kind": "live_recording",
            "name": snap.source_name,
            "path": saved_path.as_ref().map(|p| p.to_string_lossy()),
            "path_note": "path はライブ停止後に「WAV に保存」した先（保存していなければ null）",
            "live_input_name": input_name,
            "live_input_channels": input_channels,
            "input_format": "ライブ入力（取り込み時に全チャンネルの平均をとったモノラル、32-bit float）",
            "channels_note": "入力の元の複数チャンネルは保持していない。source.wav は保持していたモノラル音声",
        }),
    };

    let estimate = match snap.estimate {
        EstimateState::NotEstimated => json!({
            "status": "not_estimated",
            "hz": null,
            "confidence": null,
        }),
        EstimateState::Unverified => json!({
            "status": "unverified",
            "hz": null,
            "confidence": null,
            "note": "推定はしたが、書き出す音声で推定したものか保証できないため記録しない",
        }),
        EstimateState::Current(e) => json!({
            "status": "estimated_from_this_audio",
            "hz": n(e.hz),
            "confidence": n(e.confidence),
            "confidence_definition": "調べた区間のうち、周期をはっきり検出できた区間の割合（0〜1）。正解である確率ではない",
            "difference_from_analysis_f0_cents": n(1200.0 * (a.f0 / e.hz).log2()),
            "method": {
                "name": "YIN",
                "input": "source.wav をチャンネル平均したモノラル音声（解析と同じ）",
                "search_range_hz": [crate::pitch::MIN_HZ, crate::pitch::MAX_HZ.min(sr / 4.0)],
                "frames": "音量が最大の区間から 30 dB 以内の区間を最大 40 か所、等間隔に選ぶ",
                "threshold": 0.15,
                "aggregation": "各区間の周期の中央値",
            },
        }),
    };

    let window_mode = match snap.window_mode {
        WindowSize::Auto => json!({
            "mode": "auto",
            "rule": "基音の 8 周期以上を含む 2 のべき乗（512〜32768）",
            "auto_value_for_analysis_f0": auto_window(a.sample_rate, a.f0),
        }),
        WindowSize::Fixed(n) => json!({ "mode": "fixed", "requested_samples": n }),
    };

    let bin_hz = a.bin_hz();
    let max_k = a.num_in_range;
    let limited_by_nyquist = max_k < MAX_HARMONICS;
    let csv_columns = json!([
        { "name": "frame_index", "type": "integer", "unit": null, "missing": "なし",
          "description": "解析フレームの番号（0 始まり）" },
        { "name": "time_s", "type": "number", "unit": "s", "missing": "なし",
          "description": "窓の中心の時刻 = frame_index × hop_samples / sample_rate_hz" },
        { "name": "harmonic", "type": "integer", "unit": null, "missing": "なし",
          "description": "倍音番号（1 始まり。1 = 基音）" },
        { "name": "target_hz", "type": "number", "unit": "Hz", "missing": "なし",
          "description": "harmonic × analysis_f0_hz" },
        { "name": "peak_hz", "type": "number", "unit": "Hz", "missing": "空欄 = 局所ピークを検出していない",
          "description": "探索範囲内で検出した局所ピークの周波数（放物線補間）。status が detected_peak のときだけ値がある" },
        { "name": "sampled_bin_hz", "type": "number", "unit": "Hz", "missing": "空欄 = ピークを検出した",
          "description": "ピークを検出できず、レベルを読んだ FFT ビンの周波数（target_hz に最も近いビン）" },
        { "name": "level_db", "type": "number", "unit": "dB", "missing": "なし",
          "description": "振幅レベル（振幅 1.0 の正弦波 = 0 dB）。下限 floor_db でクランプ済み。帯域のエネルギーではない" },
        { "name": "status", "type": "string", "unit": null, "missing": "なし",
          "description": "値の得かた: detected_peak / sampled_target_bin / below_floor" },
        { "name": "edge_padded", "type": "boolean", "unit": null, "missing": "なし",
          "description": "窓が音声の先頭より前か末尾より後にかかり、その部分をゼロで埋めたフレームなら true（値が小さく出やすい）" },
        { "name": "rel_h1_db", "type": "number", "unit": "dB", "missing": "空欄 = rel_h1_missing の理由で未取得",
          "description": "基音に対する相対レベル level_db(harmonic) − level_db(1)。そのフレームで基音とこの倍音の両方が detected_peak のときだけ" },
        { "name": "rel_h1_missing", "type": "string", "unit": null, "missing": "空欄 = 値がある",
          "description": "rel_h1_db が無い理由: h1_below_floor / h1_not_detected / harmonic_below_floor / harmonic_not_detected" },
    ]);

    let listen = &snap.listen;
    let listen_value = json!({
        "enabled": listen.enabled,
        "kind": match listen.kind { ListenKind::Band => "band", ListenKind::Harmonics => "unselected_harmonics_cut" },
        "band": {
            "low_hz": n(listen.band.low_hz),
            "high_hz": n(listen.band.high_hz),
            "mode": match listen.band.mode { BandMode::Keep => "keep_band", BandMode::Remove => "remove_band" },
            "used_when": "kind が band のとき",
        },
        "unselected_harmonics_cut": {
            "kept_harmonics": snap.view.shown,
            "half_width_percent_of_f0": n(listen.harm_width_pct),
            "half_width_hz": n(a.f0 * listen.harm_width_pct / 100.0),
            "used_when": "kind が unselected_harmonics_cut のとき。kept_harmonics は表示 ON の倍音",
        },
        "applies_to": "再生音だけ。source.wav・harmonics.csv・overview.png・summary.txt はすべてフィルタ適用前の音の内容",
    });

    let excerpt = match snap.excerpt {
        None => json!({ "mode": "whole_audio", "offset_s": 0.0 }),
        Some(x) => json!({
            "mode": "time_range",
            "start_sample": x.start_sample,
            "end_sample_exclusive": x.end_sample,
            "num_samples": x.end_sample - x.start_sample,
            "offset_s": n(snap.offset_s()),
            "end_s": n(x.end_sample as f64 / sr),
            "source_sample_rate_hz": a.sample_rate,
            "source_num_samples": x.source_num_samples,
            "source_duration_s": n(x.source_num_samples as f64 / sr),
            "time_conversion": "元の音源の時刻（秒）= このファイル一式の時刻 + offset_s。サンプル位置 = 時刻 × sample_rate_hz + start_sample",
            "reanalysis": "切り出した音声を、同じ基音・同じ窓長（サンプル数）で解析し直した。窓が切り出しの端にかかるフレームは edge_padded を付け直している（元の音源の解析とは端部の値が変わる）",
        }),
    };

    let onset = match &feats.onset {
        features::Onset::Manual { t, frame } => json!({
            "method": "manual", "time_s": n(*t), "frame_index": frame,
            "note": "利用者が指定した時刻（窓が音声に収まる最も近いフレーム）",
        }),
        features::Onset::Auto { t, frame, max_rms_db } => json!({
            "method": "auto_rms",
            "time_s": n(*t),
            "frame_index": frame,
            "rule": format!("窓が音声に収まるフレームのうち、RMS が最大値（{max_rms_db:.2} dB）から {} dB 以内に初めて入るフレーム", features::ONSET_WITHIN_DB),
            "note": "推定。窓の半分（edge_padded の区間）より前の立ち上がりは見えない",
        }),
        features::Onset::Unavailable(r) => json!({ "method": "unavailable", "time_s": null, "frame_index": null, "reason": r }),
    };
    let sustain = match (snap.intervals.sustain_s, feats.sustain_frames) {
        (None, _) => json!({ "specified": false, "note": "持続区間は指定されていない。代表レベルは未取得（推測で補わない）" }),
        (Some((s0, s1)), fr) => json!({
            "specified": true,
            "source_time_s": [n(s0), n(s1)],
            "time_s": [n(snap.out_time(s0)), n(snap.out_time(s1))],
            "frames": fr.map(|(x, y)| json!([x, y])),
            "missing": feats.sustain_missing,
            "note": "利用者が指定した区間（窓が音声に収まるフレームだけを使う）",
        }),
    };
    let feature_columns = json!([
        { "name": "harmonic", "unit": null, "description": "倍音番号（1 = 基音）" },
        { "name": "target_hz", "unit": "Hz", "description": "harmonic × analysis_f0_hz" },
        { "name": "frames", "unit": null, "description": "集計したフレーム数（発音開始から、窓が音声に収まる最後のフレームまで）" },
        { "name": "detected_frames", "unit": null, "description": "そのうち status が detected_peak のフレーム数" },
        { "name": "max_level_db", "unit": "dB", "description": "集計フレームのうち detected_peak の level_db の最大値（空欄 = 検出なし）。持続音の代表値ではない" },
        { "name": "max_time_s", "unit": "s", "description": "max_level_db のフレームの時刻（同じ値が複数あれば最初）" },
        { "name": "sustain_median_db", "unit": "dB", "description": "持続区間で detected_peak だったフレームの level_db の中央値（空欄 = sustain_missing の理由で未取得）" },
        { "name": "sustain_detected_frames", "unit": null, "description": "中央値に使ったフレーム数" },
        { "name": "sustain_frames", "unit": null, "description": "持続区間のフレーム数" },
        { "name": "sustain_missing", "unit": null, "description": "not_specified / no_valid_frames / no_valid_frames_in_interval / no_detected_peak_in_interval（空欄 = 値がある）" },
        { "name": "rel_h1_median_db", "unit": "dB", "description": "rel_h1_db（基音に対する相対レベル）の中央値。持続区間があればその中、無ければ集計フレーム全体で、値のあるフレームだけ" },
        { "name": "rel_h1_frames", "unit": null, "description": "中央値に使ったフレーム数" },
        { "name": "rel_h1_missing", "unit": null, "description": "値が無いとき、いちばん多かった理由（harmonics.csv の rel_h1_missing と同じ値）" },
        { "name": "decay_20db_s", "unit": "s", "description": "max_time_s から、level_db が max_level_db − 20 dB 以下になるまでの秒数（フレーム間は直線補間）" },
        { "name": "decay_20db_status", "unit": null, "description": "reached / not_reached（観測の終わりまで下回らない。観測外は推測しない）/ threshold_below_floor（閾値が −120 dB 以下で判定不能）/ no_peak" },
        { "name": "decay_40db_s", "unit": "s", "description": "同じく − 40 dB" },
        { "name": "decay_40db_status", "unit": null, "description": "decay_20db_status と同じ" },
        { "name": "decay_reference", "unit": null, "description": "減衰の起点（いまは常に max_level = その倍音の最大の時刻）" },
    ]);
    let envelope_columns = json!([
        { "name": "frame_index", "unit": null, "description": "harmonics.csv と同じフレーム" },
        { "name": "time_s", "unit": "s", "description": "窓の中心の時刻（harmonics.csv と同じ）" },
        { "name": "rms_db", "unit": "dB", "description": "解析と同じ中心・長さの矩形窓での RMS（解析用モノラル）。振幅 1.0 = 0 dB（振幅 1.0 の正弦波の RMS は −3.01 dB）。下限 −120 dB" },
        { "name": "peak_db", "unit": "dB", "description": "同じ窓での最大の絶対値（サンプルピーク）。振幅 1.0 = 0 dB。下限 −120 dB" },
        { "name": "spectral_centroid_hz", "unit": "Hz", "description": "縮約前の線形振幅スペクトル（窓掛け・2 倍ゼロ詰め、DC を除く全ビン）を振幅で重み付けした平均周波数。無音（全ビンが下限以下）は空欄。dB 値や縮約済みのセルは使わない" },
        { "name": "harmonic_power_ratio", "unit": null, "description": "検出した倍音のパワーの割合 = Σ（その フレームで status が detected_peak の倍音の振幅² / 2）÷ フレームのパワー（窓を掛けた信号の平均 2 乗、DC を除く）。倍音だけの定常音でほぼ 1、非整数倍の成分・雑音・固定基音からのピッチのずれがあると小さくなる。無音は空欄" },
        { "name": "edge_padded", "unit": null, "description": "harmonics.csv と同じ" },
        { "name": "samples_used", "unit": "samples", "description": "RMS・ピークの計算に使ったサンプル数（端のフレームでは窓の一部が音声の外なので少ない。ゼロ埋めは数えない）" },
    ]);

    let spec = &a.spectrogram;
    let (ov_lo, ov_hi) = overview::freq_range(snap);

    let step = tr.step;
    let ps = &tr.pitch.stats;
    let (tau_min, tau_max) = crate::pitch::period_range(a.sample_rate);
    let track_len = crate::pitch::track_window(a.sample_rate);
    let pitch_track = json!({
        "file": FILE_PITCH,
        "role": "時刻ごとの基音の参考値。harmonics.csv・features.csv・partials.csv の計算には使っていない",
        "row": "1 行 = 1 時刻（解析フレームを frame_step おきに選んだもの）",
        "frame_step": step,
        "step_s": n(step as f64 * a.hop as f64 / sr),
        "frame_index_note": "harmonics.csv の frame_index と同じ番号・同じ時刻",
        "num_rows": tr.pitch.rows.len(),
        "method": {
            "name": "YIN（差分関数を FFT で計算）",
            "input": "解析と同じモノラル音声。フィルタ・正規化はしない",
            "window_samples": track_len,
            "window_s": n(track_len as f64 / sr),
            "window_rule": "最長の周期の 2 倍 + 2 サンプルの矩形窓。周期 τ の比較に使う w + τ サンプルの中心がその時刻に来るよう、1 回目の推定の周期で窓をずらして求め直す",
            "search_range_hz": [n(crate::pitch::MIN_HZ), n(crate::pitch::max_hz(a.sample_rate))],
            "period_range_samples": [tau_min, tau_max],
            "threshold": crate::pitch::TRACK_THRESHOLD,
            "dip_rule": "累積平均で正規化した差分 d'(τ) が threshold を下回る最初の谷（下回らなければ最小の谷）を放物線補間する",
            "quiet_rule": format!("窓の RMS が全時刻の最大から {} dB より小さい、または −100 dB 未満なら推定しない", crate::pitch::TRACK_QUIET_DB),
            "independent_of_analysis_f0": "推定は analysis_f0_hz を使わない（cents_from_analysis_f0 の計算にだけ使う）",
        },
        "status_values": {
            "voiced": "周期を検出した（aperiodicity < threshold）。f0_hz に値がある",
            "unvoiced": "音はあるが周期がはっきりしない（雑音・複数の音・急な変化・非整数倍の成分など）。f0_hz は空欄",
            "quiet": "音が小さすぎるので推定しない。f0_hz・aperiodicity は空欄",
            "edge": "推定の窓が音声の端をはみ出すので推定しない（窓の長さの半分より端に近い時刻）",
        },
        "columns": [
            { "name": "frame_index", "unit": null, "description": "解析フレームの番号（harmonics.csv と同じ）" },
            { "name": "time_s", "unit": "s", "description": "時刻（harmonics.csv と同じ。推定の窓の中心）" },
            { "name": "status", "unit": null, "description": "voiced / unvoiced / quiet / edge" },
            { "name": "f0_hz", "unit": "Hz", "description": "推定した基音。voiced のときだけ" },
            { "name": "cents_from_analysis_f0", "unit": "cent", "description": "1200 × log2(f0_hz / analysis_f0_hz)。voiced のときだけ" },
            { "name": "aperiodicity", "unit": null, "description": "選んだ谷の d'（0 に近いほど周期的）。voiced・unvoiced のとき" },
            { "name": "rms_db", "unit": "dB", "description": "推定の窓（window_samples の矩形窓）の RMS。振幅 1.0 = 0 dB。edge は空欄。envelope.csv の rms_db とは窓が違う" },
        ],
        "counts": { "voiced": ps.voiced, "unvoiced": ps.unvoiced, "quiet": ps.quiet, "edge": ps.edge },
        "voiced_stats": {
            "median_f0_hz": ps.median_hz.map(n),
            "cents_from_analysis_f0": {
                "p05": ps.cents_p05.map(n),
                "median": ps.cents_median.map(n),
                "p95": ps.cents_p95.map(n),
                "max_abs": ps.max_abs_cents.map(n),
            },
            "deviates_from_analysis_f0": tr.pitch.deviates(),
            "deviates_rule": "中央値が ±50 cent を超える、または 5〜95% の幅が 50 cent を超える。true なら固定基音による倍音の値（とくに高次）が外れている可能性がある",
        },
        "limitations": [
            "オクターブの取り違え（倍・半分）が起こりうる。前後の時刻とのつながりは補正していない",
            "窓（window_s）より速い変化は平均される",
            "非整数倍の成分を含む音では、両方の公約数に近い低い周期を返すことがある",
            "voiced は周期的という意味で、聴感上の音高や楽器の基音と一致する保証はない",
        ],
    });

    let pt = &tr.partials;
    let recurring: Vec<Value> = pt
        .recurring
        .iter()
        .map(|r| {
            json!({
                "median_hz": n(r.median_hz),
                "range_hz": [n(r.min_hz), n(r.max_hz)],
                "median_level_db": n(r.median_db),
                "frames": r.frames,
                "share_of_examined_frames": n(r.share),
                "nearest_harmonic": r.nearest,
                "deviation_hz": n(r.dev_hz),
                "class": r.class.as_str(),
            })
        })
        .collect();
    let partials = json!({
        "file": FILE_PARTIALS,
        "role": "縮約前のスペクトルの局所ピーク。harmonics.csv に入らない非整数倍の成分・基音より低い成分も含む",
        "row": "1 行 = 1 フレームの 1 ピーク",
        "order": "frame_index の昇順、同じフレームの中は rank（振幅の大きい順）",
        "frames": format!("pitch_track.csv と同じ時刻のうち、edge_padded でないフレーム（{} 個）", pt.frames_examined),
        "frames_examined": pt.frames_examined,
        "num_rows": pt.rows.len(),
        "spectrum": {
            "same_as": "harmonics.csv と同じ窓・ゼロ詰め・振幅の正規化（analysis.window・analysis.amplitude）",
            "value": "線形振幅（振幅 1.0 の正弦波 = 1.0）を 20 × log10 で dB にしたもの。パワーやエネルギーではない",
            "frequency_axis": "FFT ビン（bin_hz 間隔）を対数振幅の放物線補間で求めた周波数",
            "reduction": "なし（縮約前のスペクトル。スペクトログラム画像のセルは使わない）",
        },
        "peak_rule": {
            "local_max": "左隣より大きく、右隣以上のビン",
            "range_db": n(tracks::PEAK_RANGE_DB),
            "range_rule": "そのフレームの最大ピークから range_db 以内、かつ floor_db より大きいもの",
            "max_peaks_per_frame": tracks::MAX_PEAKS_PER_FRAME,
            "selection": "条件に合うピークのうち振幅の大きい順に max_peaks_per_frame 個まで",
        },
        "classification": {
            "reference_f0": "analysis_f0_hz（固定。pitch_track.csv の値は使わない）",
            "nearest_harmonic": "max(1, round(peak_hz / analysis_f0_hz))",
            "harmonic_tolerance_hz": n(pt.half_width_hz),
            "harmonic_tolerance_rule": "倍音のピーク探索と同じ幅（peak_search.half_width_hz）",
            "values": {
                "harmonic": "|peak_hz − nearest_harmonic × analysis_f0_hz| ≤ harmonic_tolerance_hz",
                "inharmonic": "整数倍から harmonic_tolerance_hz より離れている（整数倍の間）",
                "below_f0": "peak_hz < analysis_f0_hz − harmonic_tolerance_hz",
            },
            "nyquist": "nearest_harmonic が解析範囲（num_in_range）を超えることがある（ナイキスト近くのピーク）",
        },
        "columns": [
            { "name": "frame_index", "unit": null, "description": "解析フレームの番号（harmonics.csv と同じ）" },
            { "name": "time_s", "unit": "s", "description": "窓の中心の時刻" },
            { "name": "rank", "unit": null, "description": "そのフレームでの振幅の順位（1 = 最大）" },
            { "name": "peak_hz", "unit": "Hz", "description": "ピークの周波数（放物線補間）" },
            { "name": "level_db", "unit": "dB", "description": "ピークの振幅レベル（振幅 1.0 の正弦波 = 0 dB）" },
            { "name": "rel_frame_max_db", "unit": "dB", "description": "そのフレームの最大ピーク（rank 1）に対する差（0 以下）" },
            { "name": "nearest_harmonic", "unit": null, "description": "最も近い整数倍の番号" },
            { "name": "nearest_harmonic_hz", "unit": "Hz", "description": "nearest_harmonic × analysis_f0_hz" },
            { "name": "deviation_hz", "unit": "Hz", "description": "peak_hz − nearest_harmonic_hz" },
            { "name": "deviation_cents", "unit": "cent", "description": "1200 × log2(peak_hz / nearest_harmonic_hz)" },
            { "name": "class", "unit": null, "description": "harmonic / inharmonic / below_f0" },
        ],
        "recurring_non_harmonic": {
            "description": "class が inharmonic・below_f0 のピークのうち、多くのフレームに繰り返し現れる周波数",
            "rule": format!(
                "周波数を {:.4} Hz（2 ビン）の升に分け、前後 1 升を合わせて、調べたフレームの {:.0}% 以上に現れる升を多い順に選ぶ（近い升は 1 つにまとめる。最大 {} 個）。値は同じフレームでいちばん強いピークのもの",
                pt.cluster_hz,
                tracks::RECURRING_MIN_SHARE * 100.0,
                tracks::RECURRING_MAX
            ),
            "order": "median_level_db の大きい順",
            "items": recurring,
        },
        "limitations": [
            "局所ピークは楽器由来の成分とは限らない（雑音の山・窓のサイドローブ・隣の成分の裾）。雑音が多いと max_peaks_per_frame 個まで雑音の山で埋まる",
            "harmonic は固定基音の整数倍に近いという意味で、ピッチが動く音では本来の倍音でも inharmonic になりうる（pitch_track.csv を併せて見る）",
            "周波数分解能は窓長で決まる。main lobe（約 ±4 ビン）より近い 2 つの成分は 1 つのピークになる",
            "harmonic_tolerance_hz が analysis_f0_hz の 1/2 に近いと、ほとんどのピークが harmonic になる",
        ],
    });

    json!({
        "schema": "harmonic_analyzer.ai_export",
        "schema_version": SCHEMA_VERSION,
        "app": { "name": "倍音解析エディタ (harmonic_analyzer)", "version": APP_VERSION },
        "created_at_utc": utc_iso(snap.created),
        "conventions": {
            "text_encoding": "UTF-8",
            "units": { "time": "s", "frequency": "Hz", "level": "dB", "length": "samples" },
            "missing_values": { "json": "null", "csv": "空欄" },
            "non_finite_numbers": "NaN・Infinity は出力しない",
            "harmonic_numbering": "1 始まり。第1倍音 = 基音",
            "frame_numbering": "0 始まり",
            "number_format": "CSV の数値は表示用に丸めず、保持している値（f32 または f64）を復元できる 10 進表記",
        },
        "files": [
            { "name": FILE_MANIFEST, "role": "このファイル。解析条件・音源情報・状態・データ形式" },
            { "name": FILE_SUMMARY, "role": "主要な数値の要約、解析上の制約、利用者の入力した背景と質問（日本語）" },
            { "name": FILE_CSV, "role": "解析範囲内の全倍音 × 全フレームの数値（基音に対する相対レベルつき）" },
            { "name": FILE_FEATURES, "role": "倍音ごとの特徴量（最大・持続区間の代表値・相対レベル・減衰時間）" },
            { "name": FILE_ENVELOPE, "role": "フレームごとの RMS・ピーク・スペクトル重心・倍音帯域のパワーの割合" },
            { "name": FILE_PITCH, "role": "約 10 ms ごとの基音推定（参考値。倍音解析には使っていない）と推定できたかどうか" },
            { "name": FILE_PARTIALS, "role": "スペクトルの局所ピーク（非整数倍の成分を含む）と、固定基音の整数倍との関係" },
            { "name": FILE_WAV, "role": "試聴フィルタ適用前の音声" },
            { "name": FILE_OVERVIEW, "role": "波形・スペクトログラム・表示 ON の倍音の時間変化をまとめた画像" },
        ],
        "source": source,
        "time_range": excerpt,
        "audio": {
            "sample_rate_hz": c.sample_rate,
            "channels": c.channels,
            "num_samples_per_channel": c.mono.len(),
            "duration_s": n(c.duration()),
        },
        "source_wav": {
            "file": FILE_WAV,
            "format": "RIFF WAVE、32-bit IEEE float、リトルエンディアン",
            "channels": c.channels,
            "sample_rate_hz": c.sample_rate,
            "num_samples_per_channel": c.mono.len(),
            "content": "読み込み済み（デコード済み）のサンプルそのもの。入力ファイルのバイト列のコピーではない。音量の正規化・チャンネルの平均・試聴フィルタは掛けていない",
        },
        "pitch": {
            "analysis_f0_hz": n(a.f0),
            "analysis_f0_note": "倍音解析に実際に使った基音。録音全体で 1 つの固定値",
            "f0_mode": "fixed",
            "f0_mode_note": "harmonics.csv・features.csv・partials.csv の分類はすべてこの固定基音による。時間変化する基音は pitch_track.csv に別に出すだけで、倍音解析には使わない（方式を混在させない）",
            "input": if snap.pitch.matches_analysis {
                json!({
                    "text": snap.pitch.text,
                    "a4_hz": n(snap.pitch.a4_hz),
                    "cents_offset": n(snap.pitch.cents),
                    "resolution": "音名（A4 基準）または Hz を、セント補正して analysis_f0_hz にした",
                })
            } else {
                Value::Null
            },
            "input_note": if snap.pitch.matches_analysis {
                Value::Null
            } else {
                json!("入力欄の値は解析に使われていない（読み取りエラーなど）。analysis_f0_hz が実際の値")
            },
            "estimate": estimate,
        },
        "analysis": {
            "preprocessing": "全チャンネルの算術平均（モノラル）を解析する。それ以外の前処理（正規化・フィルタ）はしない",
            "window": {
                "selection": window_mode,
                "length_samples": a.window,
                "length_s": n(a.window as f64 / sr),
                "function": "4 項 Blackman-Harris（周期形、サイドローブ約 −92 dB）",
            },
            "hop_samples": a.hop,
            "hop_s": n(a.hop as f64 / sr),
            "hop_rule": "窓長の 1/8（64〜512 サンプル）",
            "fft_size": a.fft_size,
            "zero_padding": "窓長の 2 倍にゼロ詰めして FFT",
            "bin_hz": n(bin_hz),
            "num_frames": a.num_frames,
            "frame_time": "フレーム f の時刻は窓の中心 = f × hop_samples / sample_rate_hz（秒）",
            "edge_handling": "窓が音声の外にかかる部分はゼロで埋める。そのフレームは harmonics.csv の edge_padded が true",
            "amplitude": {
                "normalization": "窓の和で正規化し、振幅 1.0 の正弦波が 0 dB になる。録音全体・全倍音で共通",
                "db_formula": "20 × log10(振幅)",
                "floor_db": n32(FLOOR_DB),
                "floor_meaning": "これ以下は floor_db にクランプする（status が below_floor）。真のレベルは分からない",
                "measure": "各倍音の値は周辺ピークの振幅で、帯域を積分したエネルギーではない",
            },
            "peak_search": {
                "half_width_rule": format!("片側 max({SEARCH_TOLERANCE} × analysis_f0_hz / bin_hz, 1) ビン"),
                "half_width_bins": n(a.search_half_width_bins),
                "half_width_hz": n(a.search_half_width_bins * bin_hz),
                "bin_rounding": "目標ビン t = target_hz / bin_hz に対し、floor(t − 半幅) 〜 ceil(t + 半幅) のビンを調べる（1〜fft_size/2 − 1 に制限）",
                "peak_rule": "範囲内で振幅最大のビンが両隣以上で、下限より大きければ局所ピークとする",
                "interpolation": "対数振幅の 3 点放物線補間（補間による増加は約 0.5 dB まで）",
                "fallback": "局所ピークが無ければ、target_hz に最も近いビンの振幅を使う",
                "status_values": {
                    "detected_peak": "探索範囲内の局所ピークを検出した。その楽器由来の倍音であることは保証しない（窓のサイドローブや雑音の山でもなる）",
                    "sampled_target_bin": "ピークを検出できず、目標に最も近い FFT ビンの値を読んだ",
                    "below_floor": "値が floor_db 以下。真のレベルは確定できない",
                },
            },
            "harmonics_range": {
                "num_in_range": max_k,
                "max_harmonic": max_k,
                "nyquist_hz": n(a.nyquist()),
                "implementation_limit": MAX_HARMONICS,
                "limited_by": if limited_by_nyquist { "nyquist" } else { "implementation_limit" },
                "rule": "harmonic × analysis_f0_hz < nyquist_hz の倍音だけを解析する（最大 implementation_limit 本）",
                "out_of_range_rows": "範囲外の倍音の行は harmonics.csv に出さない（値を持たないため。0 dB や無音ではない）",
            },
        },
        "csv": {
            "file": FILE_CSV,
            "row": "1 行 = 1 フレームの 1 倍音",
            "order": "frame_index の昇順、同じフレームの中は harmonic の昇順",
            "header": true,
            "num_rows": snap.csv_rows(),
            "num_rows_rule": "num_frames × num_in_range（見出し行を除く）",
            "columns": csv_columns,
            "includes": "表示 OFF の倍音・画面の表示範囲外の時間・表示下限より弱い成分もすべて含む",
        },
        "view": {
            "note": "画面の表示状態。harmonics.csv の値や行数には影響しない。ON／OFF は表示の選択で、音の成分の有無ではない",
            "shown_harmonics": snap.view.shown,
            "focused_harmonic": snap.view.focused,
            "analysis_position": match cursor_frame {
                Some(f) => json!({ "time_s": n(a.frame_time(f)), "frame_index": f, "source_time_s": n(snap.view.cursor_s) }),
                None => json!({
                    "time_s": null,
                    "frame_index": null,
                    "source_time_s": n(snap.view.cursor_s),
                    "note": "解析位置は書き出した区間の外",
                }),
            },
            "display_floor_db": n32(snap.view.display_floor_db),
            "time_range_s": range(snap.view.time_range_s),
            "frequency_range_hz": range(snap.view.freq_range_hz),
            "level_range_db": range(snap.view.level_range_db),
            "ranges_note": "画面のグラフの表示範囲（書き出し前に最後に描いたもの、元の音源の時刻）。不明なら null",
        },
        "listening_filter": listen_value,
        "overview_png": {
            "file": FILE_OVERVIEW,
            "panels": ["波形（解析に使ったモノラル）", "スペクトログラム", "表示 ON の倍音の時間変化"],
            "audio": "フィルタ適用前の音",
            "harmonics_drawn": snap.view.shown,
            "harmonics_drawn_note": "画像には表示 ON の倍音だけを描く。全倍音は harmonics.csv にある",
            "time_range_s": [0.0, n(c.duration())],
            "spectrogram": {
                "frequency_range_hz": [n(ov_lo), n(ov_hi)],
                "color_range_db": [n32(snap.view.display_floor_db), 0.0],
                "reduction": "各セルは、まとめた FFT ビン・フレームの振幅の最大値（最大値縮約）。エネルギー等の計算には使わない",
                "cell_time_s": n(spec.col_dt),
                "cell_frequency_hz": n(spec.row_hz),
                "rows": spec.rows,
                "cols": spec.cols,
            },
            "level_range_db": [n32(snap.view.display_floor_db), 3.0],
        },
        "features": {
            "file": FILE_FEATURES,
            "row": "1 行 = 1 倍音（第1〜num_in_range）",
            "frames_used": "窓が音声に収まるフレーム（edge_padded でないもの）のうち、発音開始以降",
            "valid_frames": feats.valid.map(|(x, y)| json!({ "first": x, "last": y, "first_time_s": n(a.frame_time(x)), "last_time_s": n(a.frame_time(y)) })),
            "onset": onset,
            "sustain_interval": sustain,
            "decay": {
                "drops_db": features::DECAY_DROPS_DB,
                "start": "各倍音の最大レベル（detected_peak）の時刻",
                "threshold": "最大レベル − drop dB",
                "method": "起点より後で、level_db（status を問わない）が閾値以下になる最初のフレーム。直前のフレームとの間を直線補間",
                "end_of_observation": "窓が音声に収まる最後のフレーム。そこまでに下回らなければ not_reached（観測外を推測しない）",
            },
            "release": "キーを離した時刻は記録されていないため、リリース区間は扱わない",
            "columns": feature_columns,
        },
        "envelope": {
            "file": FILE_ENVELOPE,
            "row": "1 行 = 1 フレーム（harmonics.csv と同じフレーム）",
            "window": "解析と同じ中心・長さ（window.length_samples）の矩形窓",
            "target": "解析用モノラル（全チャンネルの算術平均）",
            "harmonic_power_ratio_limits": [
                "雑音だけでも、各倍音の探索範囲で雑音の山を detected_peak として拾うので 0 にはならない（倍音の数が多いほど大きい）",
                "雑音や非整数倍の成分が倍音の探索範囲に入ると、倍音のパワーに数えられる",
                "振幅の補間と音の変化のため、1 をわずかに超えることがある",
                "雑音の種類（スペクトルの形）は評価しない",
            ],
            "columns": envelope_columns,
        },
        "pitch_track": pitch_track,
        "partials": partials,
        "not_exported": {
            "full_spectrum": "スペクトル全体（全ビン × 全フレーム）は出力しない。ピークは partials.csv、全体の様子は overview.png のスペクトログラム（縮約済み）",
            "phase": "位相は出力しない。倍音の振幅（と周波数）だけから元の波形や発音方式（FM の設定など）を一意に復元することはできない。再合成を目的にするなら、位相の保存を別に設計する必要がある",
        },
        "user_notes": {
            "instrument_or_source": text_or_null(&snap.notes.instrument),
            "technique_or_effects": text_or_null(&snap.notes.technique),
            "question": text_or_null(&snap.notes.question),
            "note": "利用者の任意入力。null は未入力（不明）",
        },
        "limitations": [
            "倍音解析は録音全体で 1 つの固定基音を使う。ピッチの揺れ（ビブラートなど）は倍音の値に反映しない。時刻ごとの基音は pitch_track.csv に参考値として別に出す",
            "解析は全チャンネルの平均（モノラル）に対して行う。チャンネル間の差や逆相成分は解析値に現れない",
            "各倍音の値は周辺ピークの振幅で、帯域のエネルギーではない。位相は出力しない",
            "detected_peak は楽器由来の倍音であることを保証しない",
            "floor_db 以下の値は真のレベルが分からない",
            "edge_padded のフレームは窓の一部がゼロなので値が小さく出やすい",
            "倍音以外の成分（非整数倍のピーク）は harmonics.csv に含まれない。partials.csv と envelope.csv の harmonic_power_ratio を見る",
            "雑音の量は harmonic_power_ratio とピークの数でしか表していない。雑音の種類やスペクトルの形は評価していない",
        ],
    })
}
