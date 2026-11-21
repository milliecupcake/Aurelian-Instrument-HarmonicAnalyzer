//! 画面と書き出し画像（overview.png）で共通の色。

/// 第k倍音の色（k = 1 が先頭。10 色を繰り返す）。
/// 値は assets/skins/aurelian-vintage/tokens.json の colors.harmonics
pub const PALETTE: [[u8; 3]; 10] = [
    [0xa6, 0x99, 0xed],
    [0xf1, 0x80, 0x68],
    [0x33, 0xde, 0xd4],
    [0xc9, 0x86, 0xe8],
    [0xf0, 0xc8, 0x4b],
    [0x65, 0xb9, 0xf2],
    [0xef, 0x91, 0xc9],
    [0xb0, 0xbb, 0xf0],
    [0xcb, 0xa9, 0x6f],
    [0x91, 0xc7, 0xa5],
];

pub fn harmonic_rgb(k: usize) -> [u8; 3] {
    PALETTE[(k.max(1) - 1) % PALETTE.len()]
}

/// 解析位置（琥珀色。tokens.json の colors.amber）
pub const CURSOR_RGB: [u8; 3] = [0xff, 0xbd, 0x3e];

/// スペクトログラムの色。0..1 を 暗緑 → 暗紫 → 赤 → 橙 → 黄白 に対応させる
/// （tokens.json の spectrogram_stops。0 はグラフ面の色と同じ）
pub fn colormap(x: f32) -> [u8; 3] {
    const STOPS: [(f32, [u8; 3]); 6] = [
        (0.0, [0x02, 0x14, 0x13]),
        (0.25, [0x28, 0x10, 0x20]),
        (0.45, [0x80, 0x24, 0x36]),
        (0.65, [0xd3, 0x43, 0x22]),
        (0.85, [0xff, 0x9c, 0x2e]),
        (1.0, [0xff, 0xf0, 0xb0]),
    ];
    let x = if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 };
    let i = STOPS.iter().position(|s| s.0 >= x).unwrap_or(STOPS.len() - 1).max(1);
    let (x0, c0) = STOPS[i - 1];
    let (x1, c1) = STOPS[i];
    let t = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
    let ch = |j: usize| (c0[j] as f32 + (c1[j] as f32 - c0[j] as f32) * t).round() as u8;
    [ch(0), ch(1), ch(2)]
}
