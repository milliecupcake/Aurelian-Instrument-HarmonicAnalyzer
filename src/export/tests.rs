//! 書き出し一式の整合性（AIEXP-07）。ZIP を作って展開し、別のパーサーで読み直して確かめる。

use super::*;
use crate::analysis::{FLOOR_DB, analyze};
use crate::audio_file::load_wav;
use crate::filter::BandMode;
use crate::test_tones::{self, SR};
use std::io::Read;

pub(crate) fn snapshot_of(clip: AudioClip, f0: f64, shown: Vec<usize>) -> Snapshot {
    let analysis = analyze(&clip.mono, clip.sample_rate, f0, WindowSize::Auto).unwrap();
    let shown: Vec<usize> = shown.into_iter().filter(|&k| k <= analysis.num_in_range).collect();
    Snapshot {
        clip,
        analysis,
        source: SourceKind::Sample { title: "テスト音".into(), about: "説明".into() },
        source_name: "サンプル: テスト 音.wav".into(),
        pitch: PitchInput { text: format!("{f0}"), a4_hz: 440.0, cents: 0.0, matches_analysis: true },
        window_mode: WindowSize::Auto,
        estimate: EstimateState::NotEstimated,
        view: ViewState {
            shown,
            focused: None,
            cursor_s: 0.5,
            display_floor_db: -100.0,
            time_range_s: Some((0.0, 2.0)),
            freq_range_hz: None,
            level_range_db: None,
        },
        listen: ListenFilter {
            enabled: false,
            kind: ListenKind::Band,
            band: Band { low_hz: 600.0, high_hz: 900.0, mode: BandMode::Keep },
            harm_width_pct: 25.0,
        },
        notes: UserNotes::default(),
        intervals: Intervals::default(),
        range: None,
        excerpt: None,
        created: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_795_000_000),
    }
}

fn mono_snapshot(samples: Vec<f32>, f0: f64) -> Snapshot {
    snapshot_of(AudioClip::from_mono(samples, SR), f0, (1..=8).collect())
}

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ha_export_test_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Unpacked {
    names: Vec<String>,
    manifest: serde_json::Value,
    csv: Vec<csv::StringRecord>,
    header: Vec<String>,
    csv_bytes: Vec<u8>,
    features: Vec<csv::StringRecord>,
    envelope: Vec<csv::StringRecord>,
    pitch: Vec<csv::StringRecord>,
    partials: Vec<csv::StringRecord>,
    wav: Vec<u8>,
    png: Vec<u8>,
    summary: String,
}

fn unpack(path: &Path) -> Unpacked {
    let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let names: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
    let mut read = |n: &str| {
        let mut v = Vec::new();
        z.by_name(n).unwrap().read_to_end(&mut v).unwrap();
        v
    };
    let manifest_bytes = read(FILE_MANIFEST);
    let csv_bytes = read(FILE_CSV);
    let wav = read(FILE_WAV);
    let png = read(FILE_OVERVIEW);
    let summary = String::from_utf8(read(FILE_SUMMARY)).unwrap();
    let records = |b: Vec<u8>| -> Vec<csv::StringRecord> {
        csv::Reader::from_reader(b.as_slice()).records().map(|r| r.unwrap()).collect()
    };
    let features = records(read(FILE_FEATURES));
    let envelope = records(read(FILE_ENVELOPE));
    let pitch = records(read(FILE_PITCH));
    let partials = records(read(FILE_PARTIALS));
    let mut rdr = csv::Reader::from_reader(csv_bytes.as_slice());
    let header = rdr.headers().unwrap().iter().map(String::from).collect();
    let csv = rdr.records().map(|r| r.unwrap()).collect();
    Unpacked {
        names,
        manifest: serde_json::from_slice(&manifest_bytes).unwrap(),
        csv,
        header,
        csv_bytes,
        features,
        envelope,
        pitch,
        partials,
        wav,
        png,
        summary,
    }
}

fn export(snap: &Snapshot, dir: &Path, name: &str) -> Unpacked {
    let dest = dir.join(name);
    export_zip(snap, &dest, None, &Progress::default()).unwrap();
    unpack(&dest)
}

fn f(r: &csv::StringRecord, i: usize) -> Option<f64> {
    let s = &r[i];
    (!s.is_empty()).then(|| s.parse().unwrap())
}

/// JSON に NaN・無限大が無いこと（serde_json は書けないが、念のため数値をすべて確かめる）
fn all_numbers_finite(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
        serde_json::Value::Array(a) => a.iter().all(all_numbers_finite),
        serde_json::Value::Object(o) => o.values().all(all_numbers_finite),
        _ => true,
    }
}

#[test]
fn zip_has_five_files_and_consistent_shapes() {
    let dir = tmp_dir("shapes");
    let snap = mono_snapshot(test_tones::sine_220(), 220.0);
    let u = export(&snap, &dir, "テスト 書き出し.zip");
    let mut names = u.names.clone();
    names.sort();
    let mut want: Vec<String> = FILES.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(names, want);

    let a = &snap.analysis;
    assert_eq!(u.header, CSV_COLUMNS);
    assert_eq!(u.csv.len(), a.num_frames * a.num_in_range);
    let m = &u.manifest;
    assert_eq!(m["schema_version"], SCHEMA_VERSION);
    assert_eq!(m["csv"]["num_rows"], u.csv.len());
    assert_eq!(m["analysis"]["harmonics_range"]["num_in_range"], a.num_in_range);
    assert_eq!(m["analysis"]["window"]["length_samples"], a.window);
    assert_eq!(m["analysis"]["hop_samples"], a.hop);
    assert_eq!(m["analysis"]["fft_size"], a.fft_size);
    assert_eq!(m["pitch"]["analysis_f0_hz"], 220.0);
    assert_eq!(m["source"]["name"], "サンプル: テスト 音.wav");
    assert_eq!(m["user_notes"]["question"], serde_json::Value::Null);
    assert_eq!(m["csv"]["columns"].as_array().unwrap().len(), CSV_COLUMNS.len());
    assert!(all_numbers_finite(m));
    assert!(u.png.starts_with(b"\x89PNG"));
    // 一時ファイルが残っていない
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left.len(), 1, "{left:?}");
}

#[test]
fn csv_rows_match_analysis_in_order() {
    let dir = tmp_dir("rows");
    let snap = mono_snapshot(test_tones::third_harmonic_decay(), 220.0);
    let u = export(&snap, &dir, "rows.zip");
    let a = &snap.analysis;
    for (i, r) in u.csv.iter().enumerate() {
        let (fr, k) = (i / a.num_in_range, i % a.num_in_range + 1);
        assert_eq!(r[0].parse::<usize>().unwrap(), fr);
        assert_eq!(r[2].parse::<usize>().unwrap(), k);
        assert_eq!(f(r, 1).unwrap(), a.frame_time(fr));
        assert_eq!(f(r, 3).unwrap(), a.harmonic_hz(k));
        let HarmonicLevel::Level { db, peak_hz, sampled_bin_hz, status } = a.level(fr, k) else { panic!() };
        // f32 の値がそのまま戻る（丸めていない）
        assert_eq!(f(r, 6).unwrap() as f32, db);
        assert_eq!(f(r, 4).map(|v| v as f32), peak_hz);
        assert_eq!(f(r, 5).map(|v| v as f32), sampled_bin_hz);
        assert_eq!(&r[7], status.as_str());
        assert_eq!(&r[8], if a.is_edge_padded(fr) { "true" } else { "false" });
        // ピークを検出したときだけ peak_hz、それ以外は sampled_bin_hz
        assert!(r[4].is_empty() != r[5].is_empty());
        assert_eq!(r[4].is_empty(), &r[7] != "detected_peak");
    }
}

#[test]
fn test_tones_give_expected_values_in_csv() {
    let dir = tmp_dir("tones");
    // 端部（edge_padded）を除いたフレームで評価する
    let inner = |u: &Unpacked, k: usize| -> Vec<(f64, f64, String)> {
        u.csv
            .iter()
            .filter(|r| &r[8] == "false" && r[2].parse::<usize>().unwrap() == k)
            .map(|r| (f(r, 1).unwrap(), f(r, 6).unwrap(), r[7].to_string()))
            .collect()
    };
    // フェードイン・アウト（10 ms）を避けて、窓が音の安定部に収まる区間
    let steady = |t: f64| (0.1..=1.9).contains(&t);

    let u = export(&mono_snapshot(test_tones::sine_220(), 220.0), &dir, "1.zip");
    for (t, db, st) in inner(&u, 1).into_iter().filter(|x| steady(x.0)) {
        assert!((db + 6.02).abs() < 0.1, "t={t}: {db}");
        assert_eq!(st, "detected_peak");
    }

    let u = export(&mono_snapshot(test_tones::sine_220_plus_half_440(), 220.0), &dir, "2.zip");
    for ((t, h1, _), (_, h2, _)) in inner(&u, 1).into_iter().zip(inner(&u, 2)).filter(|x| steady(x.0.0)) {
        assert!((h1 - h2 - 6.02).abs() < 0.1, "t={t}: {h1} {h2}");
    }

    let u = export(&mono_snapshot(test_tones::third_harmonic_decay(), 220.0), &dir, "3.zip");
    let h1: Vec<f64> = inner(&u, 1).into_iter().filter(|x| steady(x.0)).map(|x| x.1).collect();
    let spread = h1.iter().cloned().fold(f64::MIN, f64::max) - h1.iter().cloned().fold(f64::MAX, f64::min);
    assert!(spread < 0.2, "基音がほぼ一定でない: {spread}");
    let h3: Vec<(f64, f64)> = inner(&u, 3).into_iter().filter(|x| steady(x.0)).map(|x| (x.0, x.1)).collect();
    let at = |t: f64| h3.iter().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs())).unwrap().1;
    assert!((at(0.5) - at(1.5) - 20.0).abs() < 0.5, "第3倍音の減衰: {} → {}", at(0.5), at(1.5));

    // 750 Hz を基音 220 Hz の整数倍のピークとして記録しない
    let snap4 = mono_snapshot(test_tones::inharmonic_750(), 220.0);
    let u = export(&snap4, &dir, "4.zip");
    let a4 = &snap4.analysis;
    // 探索範囲（外側へのビンの丸めと補間の ±0.5 ビンを含む）
    let reach = (a4.search_half_width_bins + 1.5) * a4.bin_hz();
    for r in &u.csv {
        if let Some(p) = f(r, 4) {
            let k: f64 = r[2].parse().unwrap();
            assert!((p - k * 220.0).abs() <= reach, "第{k}倍音のピーク {p} Hz");
            assert!((p - 750.0).abs() > 10.0, "750 Hz を第{k}倍音のピークとして記録した");
        }
    }
    for (_, db, _) in inner(&u, 3).into_iter().filter(|x| steady(x.0)) {
        assert!(db < -60.0, "660 Hz の位置に 750 Hz が漏れている: {db}");
    }
}

#[test]
fn display_state_does_not_change_csv() {
    let dir = tmp_dir("display");
    let base = mono_snapshot(test_tones::sine_220_plus_half_440(), 220.0);
    let mut other = base.clone();
    other.view.shown = Vec::new();
    other.view.focused = Some(3);
    other.view.display_floor_db = -60.0;
    other.view.time_range_s = Some((0.3, 0.4));
    let u1 = export(&base, &dir, "a.zip");
    let u2 = export(&other, &dir, "b.zip");
    assert_eq!(u1.csv_bytes, u2.csv_bytes);
    assert_eq!(u1.wav, u2.wav);
    assert_eq!(u2.manifest["view"]["shown_harmonics"], serde_json::json!([]));
    assert!(u2.summary.contains("すべて OFF"));
    assert!(!u2.csv.is_empty());
}

#[test]
fn stereo_antiphase_keeps_both_channels_and_csv_is_mono_mix() {
    let dir = tmp_dir("stereo");
    let l = test_tones::sine_220();
    let interleaved: Vec<f32> = l.iter().flat_map(|&s| [s, -s]).collect();
    let mono = vec![0.0f32; l.len()];
    let clip = AudioClip { sample_rate: SR, channels: 2, interleaved: interleaved.clone(), mono, format_desc: "16 bit 整数".into() };
    let snap = snapshot_of(clip, 220.0, vec![1]);
    let dest = dir.join("stereo.zip");
    export_zip(&snap, &dest, None, &Progress::default()).unwrap();
    let u = unpack(&dest);
    let wav_path = dir.join("source.wav");
    std::fs::write(&wav_path, &u.wav).unwrap();
    let back = load_wav(&wav_path).unwrap();
    assert_eq!(back.channels, 2);
    assert_eq!(back.sample_rate, SR);
    assert_eq!(back.interleaved, interleaved, "サンプル値がそのまま戻る");
    // 平均は無音なので、CSV の値はすべて下限
    assert!(u.csv.iter().all(|r| &r[7] == "below_floor" && f(r, 6).unwrap() as f32 == FLOOR_DB && r[4].is_empty()));
    assert_eq!(u.manifest["source_wav"]["channels"], 2);
}

#[test]
fn many_channels_use_extensible_wav() {
    let dir = tmp_dir("multi");
    let frames = 4800;
    let interleaved: Vec<f32> = (0..frames * 3).map(|i| ((i % 97) as f32 / 97.0) - 0.5).collect();
    let mono: Vec<f32> = interleaved.chunks_exact(3).map(|c| c.iter().sum::<f32>() / 3.0).collect();
    let clip = AudioClip { sample_rate: SR, channels: 3, interleaved: interleaved.clone(), mono, format_desc: "32 bit 浮動小数点".into() };
    let u = export(&snapshot_of(clip, 220.0, vec![1]), &dir, "multi.zip");
    let p = dir.join("multi.wav");
    std::fs::write(&p, &u.wav).unwrap();
    let back = load_wav(&p).unwrap();
    assert_eq!(back.channels, 3);
    assert_eq!(back.interleaved, interleaved);
}

#[test]
fn reanalysing_exported_wav_reproduces_csv() {
    let dir = tmp_dir("repro");
    let snap = mono_snapshot(test_tones::third_harmonic_decay(), 220.0);
    let u = export(&snap, &dir, "r.zip");
    let p = dir.join("r.wav");
    std::fs::write(&p, &u.wav).unwrap();
    let clip = load_wav(&p).unwrap();
    let f0 = u.manifest["pitch"]["analysis_f0_hz"].as_f64().unwrap();
    let win = u.manifest["analysis"]["window"]["length_samples"].as_u64().unwrap() as usize;
    let b = analyze(&clip.mono, clip.sample_rate, f0, WindowSize::Fixed(win)).unwrap();
    for r in &u.csv {
        let (fr, k): (usize, usize) = (r[0].parse().unwrap(), r[2].parse().unwrap());
        assert!((b.level_db(fr, k).unwrap() as f64 - f(r, 6).unwrap()).abs() < 1e-3);
    }
}

#[test]
fn silence_short_and_near_nyquist() {
    let dir = tmp_dir("edge");
    // 無音: すべて下限、ピークは空欄
    let u = export(&mono_snapshot(vec![0.0; SR as usize], 220.0), &dir, "silence.zip");
    assert!(u.csv.iter().all(|r| &r[7] == "below_floor" && r[4].is_empty() && !r[5].is_empty()));

    // 短い音: 窓が収まるフレームが無い → すべて edge_padded、要約は集計対象なし
    let short = mono_snapshot(test_tones::sine_220()[..500].to_vec(), 220.0);
    let u = export(&short, &dir, "short.zip");
    assert!(u.csv.iter().all(|r| &r[8] == "true"));
    assert!(u.summary.contains("集計対象なし"));

    // ナイキスト付近の基音: 範囲内は第1倍音だけ
    let x: Vec<f32> =
        (0..SR as usize).map(|i| (0.5 * (std::f64::consts::TAU * 15_000.0 * i as f64 / SR as f64).sin()) as f32).collect();
    let snap = mono_snapshot(x, 15_000.0);
    let u = export(&snap, &dir, "nyq.zip");
    assert_eq!(u.manifest["analysis"]["harmonics_range"]["num_in_range"], 1);
    assert_eq!(u.manifest["analysis"]["harmonics_range"]["limited_by"], "nyquist");
    assert!(u.csv.iter().all(|r| &r[2] == "1"));
    assert_eq!(u.csv.len(), snap.analysis.num_frames);
}

#[test]
fn notes_and_estimate_are_recorded() {
    let dir = tmp_dir("notes");
    let mut snap = mono_snapshot(test_tones::sine_220(), 220.0);
    snap.notes = UserNotes {
        instrument: "エレピ（FM）".into(),
        technique: "".into(),
        question: "第3倍音はなぜ早く減衰する？\n2 行目".into(),
    };
    snap.estimate = EstimateState::Current(crate::pitch::Estimate { hz: 220.5, confidence: 0.9 });
    snap.listen.enabled = true;
    snap.listen.kind = ListenKind::Harmonics;
    let u = export(&snap, &dir, "notes.zip");
    let m = &u.manifest;
    assert_eq!(m["user_notes"]["instrument_or_source"], "エレピ（FM）");
    assert_eq!(m["user_notes"]["technique_or_effects"], serde_json::Value::Null);
    assert_eq!(m["pitch"]["estimate"]["hz"], 220.5);
    assert_eq!(m["pitch"]["analysis_f0_hz"], 220.0);
    assert_eq!(m["listening_filter"]["kind"], "unselected_harmonics_cut");
    assert!(u.summary.contains("エレピ（FM）"));
    assert!(u.summary.contains("奏法・エフェクト: （未入力・不明）"));
    assert!(u.summary.contains("2 行目"));

    snap.estimate = EstimateState::Unverified;
    let u = export(&snap, &dir, "unverified.zip");
    assert_eq!(u.manifest["pitch"]["estimate"]["hz"], serde_json::Value::Null);
    assert_eq!(u.manifest["pitch"]["estimate"]["status"], "unverified");
}

#[test]
fn summary_numbers_match_csv() {
    let dir = tmp_dir("summary");
    let snap = mono_snapshot(test_tones::sine_220_plus_half_440(), 220.0);
    let u = export(&snap, &dir, "s.zip");
    for k in [1, 2] {
        let s = harmonic_stats(&snap.analysis, k);
        let (db, t) = s.max.unwrap();
        // CSV の中で、edge_padded でない detected_peak の最大値と同じ（同じ値が複数あれば最初の時刻）
        let rows: Vec<(f64, f64)> = u
            .csv
            .iter()
            .filter(|r| r[2] == *k.to_string() && &r[8] == "false" && &r[7] == "detected_peak")
            .map(|r| (f(r, 6).unwrap(), f(r, 1).unwrap()))
            .collect();
        let best = rows.iter().map(|r| r.0).fold(f64::MIN, f64::max);
        assert_eq!(db, best as f32, "CSV の値（f32）と一致");
        assert_eq!(rows.iter().find(|r| r.0 == best).unwrap().1, t);
        assert_eq!(rows.len(), s.detected);
        assert!(u.summary.contains(&format!("{db:.2} dB")), "要約に {db:.2} dB が無い");
    }
}

#[test]
fn failure_and_cancel_leave_no_broken_file() {
    let dir = tmp_dir("fail");
    let snap = mono_snapshot(test_tones::sine_220(), 220.0);
    // 存在しないフォルダ → 失敗、何も残らない
    let bad = dir.join("no/such/dir/out.zip");
    assert!(matches!(export_zip(&snap, &bad, None, &Progress::default()), Err(ExportError::Failed(_))));

    // 既存のファイルはキャンセルでそのまま残る
    let dest = dir.join("keep.zip");
    std::fs::write(&dest, b"old").unwrap();
    let p = Progress::default();
    p.cancel();
    assert_eq!(export_zip(&snap, &dest, None, &p), Err(ExportError::Cancelled));
    assert_eq!(std::fs::read(&dest).unwrap(), b"old");
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left.len(), 1, "一時ファイルが残っている: {left:?}");

    // 上書きは完成してから入れ替わる
    export_zip(&snap, &dest, None, &Progress::default()).unwrap();
    assert!(unpack(&dest).names.len() == FILES.len());
}

#[test]
fn mismatched_audio_and_analysis_is_rejected() {
    let mut snap = mono_snapshot(test_tones::sine_220(), 220.0);
    snap.clip = AudioClip::from_mono(vec![0.0; 1000], SR);
    assert!(snap.check().is_err());
}

#[test]
fn utc_and_file_names() {
    assert_eq!(utc_iso(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
    assert_eq!(utc_iso(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_795_000_000)), "2026-11-18T11:06:40Z");
    assert_eq!(default_zip_name("ピアノ C4.wav"), "ピアノ C4_ai_export.zip");
    assert_eq!(default_zip_name("a/b:c.wav"), "b_c_ai_export.zip");
    assert_eq!(default_zip_name(""), "sound_ai_export.zip");
}

fn feature_row(u: &Unpacked, k: usize) -> &csv::StringRecord {
    &u.features[k - 1]
}

#[test]
fn feature_and_envelope_files_have_expected_shape() {
    let dir = tmp_dir("p2shape");
    let snap = mono_snapshot(test_tones::sine_220(), 220.0);
    let u = export(&snap, &dir, "p2.zip");
    let a = &snap.analysis;
    assert_eq!(u.features.len(), a.num_in_range);
    assert_eq!(u.envelope.len(), a.num_frames);
    assert!(u.features.iter().all(|r| r.len() == FEATURE_COLUMNS.len()));
    assert!(u.envelope.iter().all(|r| r.len() == ENVELOPE_COLUMNS.len()));
    // envelope の時刻は harmonics.csv と同じフレーム
    for (i, r) in u.envelope.iter().enumerate() {
        assert_eq!(r[0].parse::<usize>().unwrap(), i);
        assert_eq!(f(r, 1).unwrap(), a.frame_time(i));
    }
    assert_eq!(u.manifest["schema_version"], SCHEMA_VERSION);
    assert_eq!(u.manifest["features"]["sustain_interval"]["specified"], false);
    assert!(all_numbers_finite(&u.manifest));
}

#[test]
fn relative_levels_ignore_overall_volume() {
    let dir = tmp_dir("rel");
    let x = test_tones::sine_220_plus_half_440();
    let quiet: Vec<f32> = x.iter().map(|v| v * 0.1).collect();
    let mut loud = mono_snapshot(x, 220.0);
    let mut soft = mono_snapshot(quiet, 220.0);
    for s in [&mut loud, &mut soft] {
        s.intervals.sustain_s = Some((0.3, 1.7));
    }
    let ul = export(&loud, &dir, "loud.zip");
    let us = export(&soft, &dir, "soft.zip");
    let rel = |u: &Unpacked, k| f(feature_row(u, k), 10);
    assert_eq!(rel(&ul, 1), Some(0.0));
    let (a, b) = (rel(&ul, 2).unwrap(), rel(&us, 2).unwrap());
    assert!((a + 6.02).abs() < 0.1, "第2倍音の基音比 {a}");
    assert!((a - b).abs() < 0.05, "音量だけ違うのに基音比が違う: {a} / {b}");
    // 持続区間の代表レベルは音量の差（−20 dB）だけずれる
    let (sl, ss) = (f(feature_row(&ul, 1), 6).unwrap(), f(feature_row(&us, 1), 6).unwrap());
    assert!((sl - ss - 20.0).abs() < 0.05, "{sl} {ss}");
    // 成分の無い第3倍音は、相対レベルが無い（理由つき）か、サイドローブの山を拾ってもごく小さい
    let r3 = feature_row(&ul, 3);
    assert!(if r3[10].is_empty() { !r3[12].is_empty() } else { f(r3, 10).unwrap() < -80.0 }, "{r3:?}");
    // フレームごとの rel_h1_db も同じ
    let row = ul.csv.iter().find(|r| &r[2] == "2" && f(r, 1).unwrap() > 1.0).unwrap();
    assert!((f(row, 9).unwrap() - (f(row, 6).unwrap() - f(&ul.csv[row[0].parse::<usize>().unwrap() * loud.analysis.num_in_range], 6).unwrap())).abs() < 1e-4);
}

#[test]
fn rms_and_centroid_match_theory_and_silence_is_empty() {
    let dir = tmp_dir("env");
    let u = export(&mono_snapshot(test_tones::sine_220(), 220.0), &dir, "sine.zip");
    for r in u.envelope.iter().filter(|r| (0.1..=1.9).contains(&f(r, 1).unwrap())) {
        // 振幅 0.5 の正弦波: RMS = 0.5/√2 → −9.03 dB、ピーク −6.02 dB、重心 220 Hz
        assert!((f(r, 2).unwrap() + 9.03).abs() < 0.05, "RMS {}", &r[2]);
        assert!((f(r, 3).unwrap() + 6.02).abs() < 0.05, "peak {}", &r[3]);
        assert!((f(r, 4).unwrap() - 220.0).abs() < 1.0, "centroid {}", &r[4]);
    }
    let u = export(&mono_snapshot(vec![0.0; SR as usize], 220.0), &dir, "silence.zip");
    assert!(u.envelope.iter().all(|r| f(r, 2).unwrap() as f32 == FLOOR_DB && r[4].is_empty()));
    assert!(u.features.iter().all(|r| r[4].is_empty() && &r[14] == "no_peak"));
    assert_eq!(u.manifest["features"]["onset"]["method"], "unavailable");
    assert!(u.features.iter().all(|r| r[10].is_empty() && &r[12] == "h1_below_floor"));
}

#[test]
fn decay_of_third_harmonic_differs_from_fundamental() {
    let dir = tmp_dir("decay");
    let u = export(&mono_snapshot(test_tones::third_harmonic_decay(), 220.0), &dir, "decay.zip");
    let h1 = feature_row(&u, 1);
    let h3 = feature_row(&u, 3);
    // 第3倍音は 2 秒で 40 dB（20 dB/秒）→ 20 dB 下がるのに約 1 秒、40 dB は観測の終わりまでに届かない
    assert_eq!(&h3[14], "reached");
    assert!((f(h3, 13).unwrap() - 1.0).abs() < 0.05, "第3倍音の −20 dB: {}", &h3[13]);
    assert_eq!(&h3[16], "not_reached");
    assert!(h3[15].is_empty());
    // 基音は一定なので、どちらも未到達
    assert_eq!(&h1[14], "not_reached");
    assert_eq!(&h1[16], "not_reached");
    assert!(u.summary.contains("未到達"));
}

#[test]
fn manual_intervals_are_recorded_and_checked() {
    let dir = tmp_dir("intervals");
    let mut snap = mono_snapshot(test_tones::third_harmonic_decay(), 220.0);
    snap.intervals = Intervals { onset_s: Some(0.5), sustain_s: Some((0.8, 1.2)) };
    let u = export(&snap, &dir, "i.zip");
    assert_eq!(u.manifest["features"]["onset"]["method"], "manual");
    // 発音開始より前は集計しないので、第3倍音の最大は 0.5 秒付近
    let t = f(feature_row(&u, 3), 5).unwrap();
    assert!((t - 0.5).abs() < 0.02, "{t}");
    // 持続区間の第3倍音の中央値は区間の中央（1.0 秒）の値に近い
    let at1 = u.csv.iter().filter(|r| &r[2] == "3").min_by(|a, b| (f(a, 1).unwrap() - 1.0).abs().total_cmp(&(f(b, 1).unwrap() - 1.0).abs())).unwrap();
    assert!((f(feature_row(&u, 3), 6).unwrap() - f(at1, 6).unwrap()).abs() < 0.3);
    assert!(u.summary.contains("利用者の指定"));
}

#[test]
fn excerpt_is_cut_reanalysed_and_consistent() {
    let dir = tmp_dir("excerpt");
    let mut snap = mono_snapshot(test_tones::third_harmonic_decay(), 220.0);
    snap.view.cursor_s = 1.0;
    snap.range = Some(sample_range(0.5, 1.5, SR, snap.clip.mono.len()).unwrap());
    snap.intervals.sustain_s = Some((0.7, 1.3));
    let u = export(&snap, &dir, "ex.zip");
    let m = &u.manifest;
    assert_eq!(m["time_range"]["mode"], "time_range");
    assert_eq!(m["time_range"]["start_sample"], 24_000);
    assert_eq!(m["time_range"]["end_sample_exclusive"], 72_000);
    assert_eq!(m["time_range"]["offset_s"], 0.5);
    assert_eq!(m["audio"]["num_samples_per_channel"], 48_000);
    // WAV は切り出した 1 秒（元のサンプル値のまま）
    let p = dir.join("ex.wav");
    std::fs::write(&p, &u.wav).unwrap();
    let clip = load_wav(&p).unwrap();
    assert_eq!(clip.mono.len(), 48_000);
    assert_eq!(clip.mono[..], snap.clip.mono[24_000..72_000]);
    // CSV の時刻は 0 から、最後のフレームは 1 秒
    assert_eq!(f(&u.csv[0], 1).unwrap(), 0.0);
    let last = f(u.csv.last().unwrap(), 1).unwrap();
    assert!((last - 1.0).abs() < 0.01, "{last}");
    // 切り出し WAV を記録した条件で解析し直すと CSV と一致
    let win = m["analysis"]["window"]["length_samples"].as_u64().unwrap() as usize;
    let b = analyze(&clip.mono, SR, 220.0, WindowSize::Fixed(win)).unwrap();
    for r in &u.csv {
        let (fr, k): (usize, usize) = (r[0].parse().unwrap(), r[2].parse().unwrap());
        assert!((b.level_db(fr, k).unwrap() as f64 - f(r, 6).unwrap()).abs() < 1e-3);
        assert_eq!(&r[8], if b.is_edge_padded(fr) { "true" } else { "false" });
    }
    // 先頭のフレームは切り出しの端なので edge_padded を付け直している
    assert_eq!(&u.csv[0][8], "true");
    // 解析位置（元の 1.0 秒）は区間内の 0.5 秒
    assert!((m["view"]["analysis_position"]["time_s"].as_f64().unwrap() - 0.5).abs() < 0.01);
    // 持続区間は出力の時刻（0.2〜0.8 秒）になる
    assert_eq!(m["features"]["sustain_interval"]["time_s"], serde_json::json!([0.19999999999999996, 0.8]));
    assert!(u.summary.contains("0.500〜1.500 秒"));
    assert!(u.png.starts_with(b"\x89PNG"));
}

#[test]
fn cursor_outside_excerpt_is_reported() {
    let dir = tmp_dir("outside");
    let mut snap = mono_snapshot(test_tones::sine_220(), 220.0);
    snap.view.cursor_s = 1.8;
    snap.range = Some(sample_range(0.2, 0.6, SR, snap.clip.mono.len()).unwrap());
    let u = export(&snap, &dir, "o.zip");
    assert_eq!(u.manifest["view"]["analysis_position"]["frame_index"], serde_json::Value::Null);
    assert!(u.summary.contains("書き出した区間の外"));
}

#[test]
fn invalid_ranges_are_rejected_before_export() {
    let n = SR as usize * 2;
    assert!(sample_range(1.0, 1.0, SR, n).unwrap_err().contains("空"));
    assert!(sample_range(1.5, 0.5, SR, n).unwrap_err().contains("後"));
    assert!(sample_range(-0.1, 0.5, SR, n).unwrap_err().contains("外"));
    assert!(sample_range(0.5, 2.5, SR, n).unwrap_err().contains("外"));
    assert!(sample_range(f64::NAN, 0.5, SR, n).is_err());
    assert_eq!(sample_range(0.0, 2.0, SR, n).unwrap(), (0, n));
    assert_eq!(sample_range(0.5, 1.0, SR, n).unwrap(), (24_000, 48_000));
}

// ---- AIEXP-10: 基音の時間変化と倍音以外の成分 ----

/// 0〜1 s: 200 → 300 Hz のグライド（倍音つき）、1〜1.4 s: 無音、1.4〜1.8 s: 雑音、1.8〜3 s: 250 Hz ± 40 cent のビブラート
fn pitch_change_tone() -> (Vec<f32>, impl Fn(f64) -> Option<f64>) {
    use std::f64::consts::TAU;
    let sr = SR as f64;
    let hz_at = |t: f64| -> Option<f64> {
        if t < 1.0 {
            Some(200.0 * 1.5f64.powf(t))
        } else if t >= 1.8 {
            Some(250.0 * 2f64.powf(40.0 / 1200.0 * (TAU * 5.5 * (t - 1.8)).sin()))
        } else {
            None
        }
    };
    let mut phase = 0.0;
    let mut seed = 2463534242u32;
    let x = (0..3 * SR as usize)
        .map(|i| {
            let t = i as f64 / sr;
            match hz_at(t) {
                Some(hz) => {
                    phase += TAU * hz / sr;
                    (0.4 * phase.sin() + 0.2 * (2.0 * phase).sin() + 0.1 * (3.0 * phase).sin()) as f32
                }
                None if t >= 1.4 => {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed as f32 / u32::MAX as f32 - 0.5) * 0.6
                }
                None => 0.0,
            }
        })
        .collect();
    (x, hz_at)
}

#[test]
fn pitch_track_distinguishes_change_from_unestimable_sections() {
    let dir = tmp_dir("pitchtrack");
    let (x, hz_at) = pitch_change_tone();
    let snap = mono_snapshot(x, 250.0);
    let u = export(&snap, &dir, "p.zip");
    let a = &snap.analysis;
    let m = &u.manifest["pitch_track"];
    let step = m["frame_step"].as_u64().unwrap() as usize;
    assert_eq!(u.pitch.len(), m["num_rows"].as_u64().unwrap() as usize);
    assert_eq!(u.pitch.len(), (a.num_frames - 1) / step + 1);
    assert!(u.pitch.iter().all(|r| r.len() == tracks::PITCH_COLUMNS.len()));
    let half = crate::pitch::track_window(SR) as f64 / 2.0 / SR as f64 + 0.005;
    let mut seen = std::collections::BTreeSet::new();
    let mut voiced_ok = 0;
    for (i, r) in u.pitch.iter().enumerate() {
        let frame: usize = r[0].parse().unwrap();
        assert_eq!(frame, i * step);
        let t = f(r, 1).unwrap();
        assert_eq!(t, a.frame_time(frame), "harmonics.csv と同じ時刻");
        let status = &r[2];
        seen.insert(status.to_string());
        // f0 と cent は voiced のときだけ
        assert_eq!(f(r, 3).is_some(), status == "voiced");
        assert_eq!(f(r, 4).is_some(), status == "voiced");
        if let (Some(hz), Some(c)) = (f(r, 3), f(r, 4)) {
            assert!((c - 1200.0 * (hz / 250.0).log2()).abs() < 1e-6);
        }
        let near = |b: f64| (t - b).abs() <= half;
        if near(0.0) || near(3.0) || near(1.0) || near(1.4) || near(1.8) {
            continue;
        }
        match hz_at(t) {
            Some(hz) => {
                assert_eq!(status, "voiced", "{t} s");
                let cents = 1200.0 * (f(r, 3).unwrap() / hz).log2();
                assert!(cents.abs() < 15.0, "{t} s: {:?} のはずが {hz}", f(r, 3));
                voiced_ok += 1;
            }
            None if t < 1.4 => assert_eq!(status, "quiet", "{t} s"),
            None => assert_eq!(status, "unvoiced", "{t} s"),
        }
    }
    assert!(voiced_ok > 150);
    for s in ["voiced", "unvoiced", "quiet", "edge"] {
        assert!(seen.contains(s), "{s} が無い");
    }
    // グライドの幅（200 → 300 Hz）は固定基音 250 Hz から大きくずれる
    assert_eq!(m["voiced_stats"]["deviates_from_analysis_f0"], true);
    assert!(u.summary.contains("固定基音とのずれが大きい"));
    // 倍音解析は固定基音のまま
    assert_eq!(u.manifest["pitch"]["f0_mode"], "fixed");
    assert_eq!(u.manifest["pitch"]["analysis_f0_hz"], 250.0);
    assert!(u.csv.iter().all(|r| f(r, 3).unwrap() == r[2].parse::<f64>().unwrap() * 250.0));
    assert!(all_numbers_finite(&u.manifest));
}

#[test]
fn steady_tone_does_not_deviate() {
    let dir = tmp_dir("steady");
    let snap = mono_snapshot(test_tones::sine_220_plus_half_440(), 220.0);
    let u = export(&snap, &dir, "s.zip");
    let v = &u.manifest["pitch_track"]["voiced_stats"];
    assert!((v["median_f0_hz"].as_f64().unwrap() - 220.0).abs() < 0.2);
    assert!(v["cents_from_analysis_f0"]["max_abs"].as_f64().unwrap() < 3.0);
    assert_eq!(v["deviates_from_analysis_f0"], false);
    assert!(!u.summary.contains("固定基音とのずれが大きい"));
    // 整数倍だけの音には、繰り返し現れる非整数倍のピークが無い
    assert_eq!(u.manifest["partials"]["recurring_non_harmonic"]["items"], serde_json::json!([]));
    let inh = u.partials.iter().filter(|r| &r[10] != "harmonic" && f(r, 5).unwrap() > -60.0).count();
    assert_eq!(inh, 0, "強い非整数倍のピークは無いはず");
}

#[test]
fn inharmonic_750_is_recorded_apart_from_integer_harmonics() {
    let dir = tmp_dir("inh750");
    // 試験音のファイル（test_tones/4_inharmonic_750.wav と同じ内容）
    let snap = mono_snapshot(test_tones::inharmonic_750(), 220.0);
    let u = export(&snap, &dir, "i.zip");
    let a = &snap.analysis;
    assert!(u.partials.iter().all(|r| r.len() == tracks::PARTIAL_COLUMNS.len()));
    let pm = &u.manifest["partials"];
    assert_eq!(u.partials.len(), pm["num_rows"].as_u64().unwrap() as usize);
    let examined = pm["frames_examined"].as_u64().unwrap() as usize;
    assert!(examined > 100);

    // 各フレームで、750 Hz のピークが inharmonic（第3倍音 660 Hz から +90 Hz）として出る
    let mut frames_750 = std::collections::BTreeSet::new();
    for r in &u.partials {
        let frame: usize = r[0].parse().unwrap();
        assert!(!a.is_edge_padded(frame));
        let hz = f(r, 3).unwrap();
        if (hz - 750.0).abs() < 1.0 {
            assert_eq!(&r[10], "inharmonic");
            assert_eq!(&r[6], "3");
            assert!((f(r, 8).unwrap() - 90.0).abs() < 1.0);
            assert!((f(r, 4).unwrap() - 20.0 * 0.25f64.log10()).abs() < 0.2, "750 Hz は振幅 0.25");
            frames_750.insert(frame);
        }
        if (hz - 220.0).abs() < 1.0 {
            assert_eq!(&r[10], "harmonic");
            assert_eq!(&r[6], "1");
        }
    }
    assert_eq!(frames_750.len(), examined, "全フレームで 750 Hz を記録");

    // 繰り返し現れる非整数倍のピークとしてまとまる
    let items = pm["recurring_non_harmonic"]["items"].as_array().unwrap();
    let top = &items[0];
    assert!((top["median_hz"].as_f64().unwrap() - 750.0).abs() < 0.5, "{top}");
    assert_eq!(top["class"], "inharmonic");
    assert_eq!(top["nearest_harmonic"], 3);
    assert!(top["share_of_examined_frames"].as_f64().unwrap() > 0.99);
    assert!(u.summary.contains("750."));

    // harmonics.csv には 750 Hz のピークが入らない（整数倍の探索範囲の外）
    assert!(u.csv.iter().all(|r| f(r, 4).is_none_or(|p| (p - 750.0).abs() > 30.0)));

    // 検出した倍音のパワーの割合は 1 より小さい（750 Hz の分）
    let (first, last) = summary::stats_frames(a).unwrap();
    let r = a.harmonic_power_ratio((first + last) / 2).unwrap();
    let expect = 0.5f64.powi(2) / (0.5f64.powi(2) + 0.25f64.powi(2));
    assert!((r - expect).abs() < 0.01, "{r} vs {expect}");
}

#[test]
fn harmonic_power_ratio_separates_tone_from_noise() {
    for x in [test_tones::sine_220(), test_tones::sine_220_plus_half_440()] {
        let a = analyze(&x, SR, 220.0, WindowSize::Auto).unwrap();
        let r = a.harmonic_power_ratio(a.frame_at(1.0)).unwrap();
        assert!((r - 1.0).abs() < 0.01, "{r}");
    }
    let mut seed = 88172645u32;
    let noise: Vec<f32> = (0..SR as usize * 2)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32 - 0.5
        })
        .collect();
    let n = analyze(&noise, SR, 220.0, WindowSize::Auto).unwrap();
    let mut v: Vec<f64> = (10..n.num_frames - 10).filter_map(|f| n.harmonic_power_ratio(f)).collect();
    v.sort_by(f64::total_cmp);
    let med = v[v.len() / 2];
    assert!(med < 0.5, "雑音 {med}");
    let s = analyze(&vec![0.0; SR as usize], SR, 220.0, WindowSize::Auto).unwrap();
    assert!((0..s.num_frames).all(|f| s.harmonic_power_ratio(f).is_none()));
}

#[test]
fn classify_boundaries() {
    use tracks::{PartialClass::*, classify};
    assert_eq!(classify(220.0, 220.0, 22.0), (1, 0.0, Harmonic));
    assert_eq!(classify(750.0, 220.0, 22.0).2, Inharmonic);
    assert_eq!(classify(682.0, 220.0, 22.0).2, Harmonic);
    assert_eq!(classify(683.0, 220.0, 22.0).2, Inharmonic);
    assert_eq!(classify(100.0, 220.0, 22.0), (1, -120.0, BelowF0));
    assert_eq!(classify(50.0, 220.0, 22.0).0, 1, "0 番は使わない");
}
