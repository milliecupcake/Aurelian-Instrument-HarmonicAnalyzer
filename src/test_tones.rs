//! 完成確認用の試験音（要件定義の「完成を確認する音例」）。

use std::f64::consts::TAU;

pub const SR: u32 = 48_000;
pub const DURATION: f64 = 2.0;

fn render(f: impl Fn(f64) -> f64) -> Vec<f32> {
    let n = (DURATION * SR as f64) as usize;
    // 両端 10 ms のフェードでクリックを防ぐ
    let fade = 0.01;
    (0..n)
        .map(|i| {
            let t = i as f64 / SR as f64;
            let g = (t / fade).min((DURATION - t) / fade).clamp(0.0, 1.0);
            (f(t) * g) as f32
        })
        .collect()
}

/// 1. 220 Hz 正弦波（振幅 0.5）
pub fn sine_220() -> Vec<f32> {
    render(|t| 0.5 * (TAU * 220.0 * t).sin())
}

/// 2. 220 Hz（振幅 0.5）+ 440 Hz（振幅 0.25）→ 第2倍音が約 -6.02 dB
pub fn sine_220_plus_half_440() -> Vec<f32> {
    render(|t| 0.5 * (TAU * 220.0 * t).sin() + 0.25 * (TAU * 440.0 * t).sin())
}

/// 3. 基音一定、第3倍音だけ 0 dB → -40 dB へ指数減衰（dB で直線）
pub fn third_harmonic_decay() -> Vec<f32> {
    render(|t| {
        let h3 = 0.4 * 10f64.powf(-40.0 * (t / DURATION) / 20.0);
        0.4 * (TAU * 220.0 * t).sin() + h3 * (TAU * 660.0 * t).sin()
    })
}

/// 4. 220 Hz + 非整数倍の 750 Hz
pub fn inharmonic_750() -> Vec<f32> {
    render(|t| 0.5 * (TAU * 220.0 * t).sin() + 0.25 * (TAU * 750.0 * t).sin())
}

pub fn all() -> Vec<(&'static str, Vec<f32>)> {
    vec![
        ("1_sine_220.wav", sine_220()),
        ("2_220_plus_half_440.wav", sine_220_plus_half_440()),
        ("3_third_harmonic_decay.wav", third_harmonic_decay()),
        ("4_inharmonic_750.wav", inharmonic_750()),
    ]
}
