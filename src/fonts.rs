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

/// 英字の銘板・見出し用に OS のセリフ体を探す（無ければ None。フォントは同梱しない）
const SERIF_CANDIDATES: [&str; 5] = [
    "/System/Library/Fonts/Supplemental/Georgia.ttf",
    "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
    "C:\\Windows\\Fonts\\georgia.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
];

pub fn load_serif_font() -> Option<Vec<u8>> {
    SERIF_CANDIDATES.iter().find_map(|p| std::fs::read(p).ok())
}
