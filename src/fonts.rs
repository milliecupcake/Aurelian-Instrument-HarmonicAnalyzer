//! OS に入っている日本語フォントの読み込み（画面と書き出し画像で共通）。

const CJK_CANDIDATES: [&str; 6] = [
    "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "C:\\Windows\\Fonts\\YuGothM.ttc",
    "C:\\Windows\\Fonts\\meiryo.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
];

/// 最初に見つかった日本語フォントのバイト列（.ttc なら先頭のフォントを使う）
pub fn load_cjk_font() -> Option<Vec<u8>> {
    CJK_CANDIDATES.iter().find_map(|p| std::fs::read(p).ok())
}
