//! 画面と書き出し画像（overview.png）で共通の色。

/// 第k倍音の色（k = 1 が先頭。10 色を繰り返す）
pub const PALETTE: [[u8; 3]; 10] = [
    [86, 156, 255],
    [255, 99, 99],
    [120, 210, 90],
    [220, 120, 255],
    [255, 200, 60],
    [60, 210, 210],
    [255, 140, 190],
    [170, 170, 255],
    [200, 160, 100],
    [160, 230, 180],
];

pub fn harmonic_rgb(k: usize) -> [u8; 3] {
    PALETTE[(k.max(1) - 1) % PALETTE.len()]
}

/// 解析位置（橙）
pub const CURSOR_RGB: [u8; 3] = [255, 160, 40];

/// スペクトログラムの色。0..1 を 黒 → 紫 → 橙 → 黄白（magma 風）に対応させる
pub fn colormap(x: f32) -> [u8; 3] {
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [0.0, 0.0, 0.02]),
        (0.3, [0.23, 0.06, 0.44]),
        (0.55, [0.71, 0.21, 0.47]),
        (0.8, [0.98, 0.53, 0.28]),
        (1.0, [0.99, 0.99, 0.75]),
    ];
    let x = if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 };
    let i = STOPS.iter().position(|s| s.0 >= x).unwrap_or(4).max(1);
    let (x0, c0) = STOPS[i - 1];
    let (x1, c1) = STOPS[i];
    let t = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
    let ch = |j: usize| ((c0[j] + (c1[j] - c0[j]) * t) * 255.0) as u8;
    [ch(0), ch(1), ch(2)]
}
