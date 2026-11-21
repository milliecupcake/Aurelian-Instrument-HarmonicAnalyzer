//! 要件定義「完成を確認する音例」の自動確認。

use harmonic_analyzer::analysis::{FLOOR_DB, WindowSize, analyze};
use harmonic_analyzer::test_tones::{self, SR};

fn db(a: &harmonic_analyzer::analysis::Analysis, t: f64, k: usize) -> f32 {
    a.level_db(a.frame_at(t), k).unwrap()
}

#[test]
fn case1_sine_220_fundamental_dominates() {
    let a = analyze(&test_tones::sine_220(), SR, 220.0, WindowSize::Auto).unwrap();
    let h1 = db(&a, 1.0, 1);
    assert!((h1 - (-6.02)).abs() < 0.1, "振幅0.5 → -6.02 dB のはず: {h1}");
    for k in 2..=a.num_in_range {
        assert!(db(&a, 1.0, k) < h1 - 60.0, "第{k}倍音が大きすぎる");
    }
}

#[test]
fn case2_second_harmonic_6db_below_in_bar_and_track() {
    let a = analyze(&test_tones::sine_220_plus_half_440(), SR, 220.0, WindowSize::Auto).unwrap();
    for t in [0.3, 1.0, 1.7] {
        let f = a.frame_at(t);
        let diff = a.level_db(f, 1).unwrap() - a.level_db(f, 2).unwrap();
        assert!((diff - 6.02).abs() < 0.1, "t={t}: 差 {diff} dB");
        // 棒（level_db）と折れ線（track）が同じ時刻で同じ値
        let t1 = a.track(1).unwrap()[f];
        let t2 = a.track(2).unwrap()[f];
        assert_eq!(t1[0], a.frame_time(f));
        assert_eq!(t1[1] as f32, a.level_db(f, 1).unwrap());
        assert_eq!(t2[1] as f32, a.level_db(f, 2).unwrap());
    }
}

#[test]
fn case3_third_harmonic_decays_fundamental_flat() {
    let a = analyze(&test_tones::third_harmonic_decay(), SR, 220.0, WindowSize::Auto).unwrap();
    let ts: Vec<f64> = (0..=16).map(|i| 0.1 + i as f64 * 0.1).collect();
    let h1: Vec<f32> = ts.iter().map(|&t| db(&a, t, 1)).collect();
    let spread = h1.iter().cloned().fold(f32::MIN, f32::max) - h1.iter().cloned().fold(f32::MAX, f32::min);
    assert!(spread < 0.2, "基音がほぼ水平でない: {spread} dB");
    let h3: Vec<f32> = ts.iter().map(|&t| db(&a, t, 3)).collect();
    assert!(h3.windows(2).all(|w| w[1] < w[0]), "第3倍音が単調に下がらない: {h3:?}");
    // 期待値: -40 dB / 2 s → 0.5 s と 1.5 s で 20 dB 差
    let d = db(&a, 0.5, 3) - db(&a, 1.5, 3);
    assert!((d - 20.0).abs() < 0.5, "前半と後半の差 {d} dB");
}

#[test]
fn case4_inharmonic_750_visible_apart_from_660() {
    let a = analyze(&test_tones::inharmonic_750(), SR, 220.0, WindowSize::Auto).unwrap();
    let s = &a.spectrogram;
    let at = |hz: f64| s.value_at(1.0, hz).unwrap();
    // 750 Hz 付近が強く、660 Hz（第3倍音の位置）は弱い
    assert!(at(750.0) > -15.0, "750 Hz: {}", at(750.0));
    assert!(at(660.0) < at(750.0) - 40.0, "660 Hz: {}", at(660.0));
    // 第3倍音として 750 Hz を拾っていない
    assert!(db(&a, 1.0, 3) < db(&a, 1.0, 1) - 40.0);
}

#[test]
fn silence_and_out_of_range() {
    let silence = vec![0.0f32; SR as usize];
    let a = analyze(&silence, SR, 220.0, WindowSize::Auto).unwrap();
    let v = a.level_db(a.frame_at(0.5), 1).unwrap();
    assert!(v.is_finite() && v == FLOOR_DB);
    assert!(a.spectrogram.db.iter().all(|v| v.is_finite()));
    // 220 Hz × 109 = 23980 < 24000、× 110 = 24200 ≥ 24000
    assert_eq!(a.num_in_range, 109);
    assert!(a.level_db(0, 110).is_none());
}

#[test]
fn between_bins_frequency() {
    // FFT のビンの間に来る周波数でも振幅が正しく読める
    for f0 in [217.3, 233.7, 251.9] {
        let x: Vec<f32> = (0..SR as usize)
            .map(|i| (0.5 * (std::f64::consts::TAU * f0 * i as f64 / SR as f64).sin()) as f32)
            .collect();
        let a = analyze(&x, SR, f0, WindowSize::Auto).unwrap();
        let v = a.level_db(a.frame_at(0.5), 1).unwrap();
        assert!((v - (-6.02)).abs() < 0.1, "f0={f0}: {v}");
    }
}
