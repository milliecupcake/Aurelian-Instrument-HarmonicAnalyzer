//! 「Aurelian Vintage」スキン: 配色・素材（PNG）の読み込み・9-slice の描画・計器風のボタンなど。
//!
//! 色と寸法の正本は `assets/skins/aurelian-vintage/tokens.json` と `manifest.json`。
//! 素材は実行ファイルに埋め込み、起動時に一度だけデコードする。読み込めなければ無地で描く（起動は止めない）。
//! ここは描画だけを受け持ち、アプリの状態は持たない。

use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, FontFamily, FontId, Painter, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};
use std::sync::Arc;

// ---- 色（tokens.json の colors） ----
pub const CHASSIS: Color32 = Color32::from_rgb(0x0b, 0x1c, 0x1c);
pub const PANEL: Color32 = Color32::from_rgb(0x04, 0x15, 0x12);
pub const PLOT: Color32 = Color32::from_rgb(0x02, 0x14, 0x13);
pub const GRID_MAJOR: Color32 = Color32::from_rgb(0x28, 0x50, 0x4d);
pub const BRASS_LIGHT: Color32 = Color32::from_rgb(0xe6, 0xce, 0x91);
pub const BRASS: Color32 = Color32::from_rgb(0xb8, 0x93, 0x50);
pub const BRASS_SHADOW: Color32 = Color32::from_rgb(0x62, 0x49, 0x22);
pub const TEXT: Color32 = Color32::from_rgb(0xf4, 0xe8, 0xc8);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xc4, 0xb9, 0x98);
pub const TEXT_DISABLED: Color32 = Color32::from_rgb(0x7b, 0x82, 0x70);
pub const INK: Color32 = Color32::from_rgb(0x17, 0x1e, 0x16);
pub const CYAN: Color32 = Color32::from_rgb(0x33, 0xde, 0xd4);
pub const AMBER: Color32 = Color32::from_rgb(0xff, 0xbd, 0x3e);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0x80, 0xdd, 0xa0);
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x88, 0x78);
pub const SELECTED_FILL: Color32 = Color32::from_rgb(0x0c, 0x46, 0x42);
pub const FOCUS_RING: Color32 = Color32::from_rgb(0xa6, 0xf4, 0xe4);

// ---- 寸法（論理 pt） ----
/// クリック領域の最小の高さ（tokens.spacing.min_hit_target）
pub const MIN_HIT: f32 = 36.0;
/// 筐体テクスチャ 1 タイルの大きさ
const CHASSIS_TILE: f32 = 512.0;
/// 筐体テクスチャの濃さ
const CHASSIS_OPACITY: f32 = 0.65;
/// 銘板の文字の左右余白（ネジを避ける）
const PLAQUE_PAD_X: f32 = 26.0;
const PLAQUE_H: f32 = 32.0;
/// 9-slice の切り分け（左, 上, 右, 下。manifest.json の slice_ltrb）
const NAMEPLATE_SLICE: [f32; 4] = [24.0, 18.0, 24.0, 18.0];
const PLAQUE_SLICE: [f32; 4] = [24.0, 12.0, 24.0, 12.0];
const PANEL_SLICE: [f32; 4] = [12.0; 4];
const BUTTON_SLICE: [f32; 4] = [12.0; 4];
/// 部品・アイコンの PNG は 2 倍解像度
const SOURCE_SCALE: f32 = 2.0;

/// 英字の銘板・見出しに使うフォント（OS のセリフ体 → 既存の日本語フォントの順に探す）
pub fn heading_family() -> FontFamily {
    FontFamily::Name("instrument-heading".into())
}

macro_rules! asset {
    ($path:literal) => {
        include_bytes!(concat!("../assets/skins/aurelian-vintage/", $path)).as_slice()
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Stop,
    Folder,
    Lock,
    Unlock,
    Settings,
    Export,
    Help,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lamp {
    Off,
    Amber,
    Teal,
    Red,
}

/// ボタンの選択状態（点灯色）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Active {
    No,
    /// 再生中、選択中の試聴モードなど
    Amber,
    /// 固定／追従などの選択
    Teal,
}

/// ボタンの見た目の状態（manifest の button-* に対応）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonLook {
    Idle,
    Hover,
    Pressed,
    ActiveAmber,
    ActiveTeal,
    Disabled,
}

/// 描画の優先順: disabled → active → pressed → hover → idle
fn button_look(enabled: bool, active: Active, pressed: bool, hovered: bool) -> ButtonLook {
    match (enabled, active) {
        (false, _) => ButtonLook::Disabled,
        (true, Active::Amber) => ButtonLook::ActiveAmber,
        (true, Active::Teal) => ButtonLook::ActiveTeal,
        (true, Active::No) if pressed => ButtonLook::Pressed,
        (true, Active::No) if hovered => ButtonLook::Hover,
        _ => ButtonLook::Idle,
    }
}

/// デコード済みの素材（起動時に一度だけ作る）
pub struct SkinAssets {
    chassis: TextureHandle,
    nameplate: TextureHandle,
    plaque: TextureHandle,
    panel: TextureHandle,
    /// Idle, Hover, Pressed, ActiveAmber, ActiveTeal, Disabled の順
    buttons: [TextureHandle; 6],
    screw: TextureHandle,
    /// Off, Amber, Teal, Red の順
    lamps: [TextureHandle; 4],
    /// Icon の並び順
    icons: [TextureHandle; 8],
}

impl SkinAssets {
    fn load(ctx: &egui::Context) -> Result<Self, String> {
        let part = |name: &str, bytes: &[u8]| load_png(ctx, name, bytes, TextureOptions::LINEAR);
        Ok(SkinAssets {
            chassis: load_png(
                ctx,
                "skin/chassis-patina",
                asset!("textures/chassis-patina.png"),
                TextureOptions::LINEAR_MIRRORED_REPEAT,
            )?,
            nameplate: part("skin/nameplate", asset!("components/nameplate@2x.png"))?,
            plaque: part("skin/section-plaque", asset!("components/section-plaque@2x.png"))?,
            panel: part("skin/panel-frame", asset!("components/panel-frame@2x.png"))?,
            buttons: [
                part("skin/button-idle", asset!("components/button-idle@2x.png"))?,
                part("skin/button-hover", asset!("components/button-hover@2x.png"))?,
                part("skin/button-pressed", asset!("components/button-pressed@2x.png"))?,
                part("skin/button-active-amber", asset!("components/button-active-amber@2x.png"))?,
                part("skin/button-active-teal", asset!("components/button-active-teal@2x.png"))?,
                part("skin/button-disabled", asset!("components/button-disabled@2x.png"))?,
            ],
            screw: part("skin/screw", asset!("components/screw@2x.png"))?,
            lamps: [
                part("skin/lamp-off", asset!("components/lamp-off@2x.png"))?,
                part("skin/lamp-amber", asset!("components/lamp-amber@2x.png"))?,
                part("skin/lamp-teal", asset!("components/lamp-teal@2x.png"))?,
                part("skin/lamp-red", asset!("components/lamp-red@2x.png"))?,
            ],
            icons: [
                part("skin/icon-play", asset!("icons/play@2x.png"))?,
                part("skin/icon-stop", asset!("icons/stop@2x.png"))?,
                part("skin/icon-folder", asset!("icons/folder@2x.png"))?,
                part("skin/icon-lock", asset!("icons/lock@2x.png"))?,
                part("skin/icon-unlock", asset!("icons/unlock@2x.png"))?,
                part("skin/icon-settings", asset!("icons/settings@2x.png"))?,
                part("skin/icon-export", asset!("icons/export@2x.png"))?,
                part("skin/icon-help", asset!("icons/help@2x.png"))?,
            ],
        })
    }

    fn button(&self, look: ButtonLook) -> &TextureHandle {
        &self.buttons[look as usize]
    }

    fn icon(&self, icon: Icon) -> &TextureHandle {
        &self.icons[icon as usize]
    }

    fn lamp(&self, lamp: Lamp) -> &TextureHandle {
        &self.lamps[lamp as usize]
    }
}

fn skin_id() -> egui::Id {
    egui::Id::unique("aurelian-vintage-skin")
}

/// 配色を設定し、素材を読み込む。素材を読めなければ無地のまま続ける。
/// `HA_NO_SKIN=1` で素材を使わない表示（読み込み失敗時と同じ）を確かめられる。
pub fn install(ctx: &egui::Context) {
    apply_visuals(ctx);
    if std::env::var_os("HA_NO_SKIN").is_some() {
        return;
    }
    match SkinAssets::load(ctx) {
        Ok(s) => ctx.data_mut(|d| { d.insert_temp(skin_id(), Arc::new(s)); }),
        Err(e) => eprintln!("スキン素材を読み込めませんでした（無地で表示します）: {e}"),
    }
}

fn get(ctx: &egui::Context) -> Option<Arc<SkinAssets>> {
    ctx.data(|d| d.get_temp::<Arc<SkinAssets>>(skin_id()))
}

/// PNG を読み、非乗算アルファの RGBA としてテクスチャにする
fn load_png(ctx: &egui::Context, name: &str, bytes: &[u8], options: TextureOptions) -> Result<TextureHandle, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("{name}: {e}"))?;
    let len = reader.output_buffer_size().ok_or_else(|| format!("{name}: 画像が大きすぎます"))?;
    let mut buf = vec![0; len];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("{name}: {e}"))?;
    let size = [info.width as usize, info.height as usize];
    let data = &buf[..info.buffer_size()];
    let image = match info.color_type {
        png::ColorType::Rgb => ColorImage::from_rgb(size, data),
        png::ColorType::Rgba => ColorImage::from_rgba_unmultiplied(size, data),
        png::ColorType::Grayscale => {
            let rgba: Vec<u8> = data.iter().flat_map(|&v| [v, v, v, 255]).collect();
            ColorImage::from_rgba_unmultiplied(size, &rgba)
        }
        png::ColorType::GrayscaleAlpha => {
            let rgba: Vec<u8> = data.as_chunks::<2>().0.iter().flat_map(|&[v, a]| [v, v, v, a]).collect();
            ColorImage::from_rgba_unmultiplied(size, &rgba)
        }
        png::ColorType::Indexed => return Err(format!("{name}: パレット形式を展開できません")),
    };
    Ok(ctx.load_texture(name, image, options))
}

/// 9-slice で描く。`slice_ltrb` は論理 pt。四隅は拡大せず、辺は 1 軸、中央は 2 軸に伸ばす。
/// PNG のピクセルと論理 pt の換算は `SOURCE_SCALE` だけで行い、pixels_per_point は掛けない
/// （画面の解像度への換算は egui が行う）。
fn nine_slice(painter: &Painter, tex: &TextureHandle, dst: Rect, slice_ltrb: [f32; 4], tint: Color32) {
    painter.add(nine_slice_shape(tex, dst, slice_ltrb, tint));
}

fn nine_slice_shape(tex: &TextureHandle, dst: Rect, slice_ltrb: [f32; 4], tint: Color32) -> Shape {
    let [l, t, r, b] = slice_ltrb;
    let [pw, ph] = tex.size();
    let (w, h) = (pw as f32 / SOURCE_SCALE, ph as f32 / SOURCE_SCALE);
    // 四隅が入らない大きさなら、四隅を縮めて重ならないようにする
    let sx = (dst.width() / (l + r)).min(1.0);
    let sy = (dst.height() / (t + b)).min(1.0);
    let (dl, dr, dt, db) = (l * sx, r * sx, t * sy, b * sy);
    let us = [0.0, l / w, 1.0 - r / w, 1.0];
    let vs = [0.0, t / h, 1.0 - b / h, 1.0];
    let xs = [dst.left(), dst.left() + dl, dst.right() - dr, dst.right()];
    let ys = [dst.top(), dst.top() + dt, dst.bottom() - db, dst.bottom()];
    let mut mesh = egui::Mesh::with_texture(tex.id());
    for row in 0..3 {
        for col in 0..3 {
            let rect = Rect::from_min_max(pos2(xs[col], ys[row]), pos2(xs[col + 1], ys[row + 1]));
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                continue;
            }
            let uv = Rect::from_min_max(pos2(us[col], vs[row]), pos2(us[col + 1], vs[row + 1]));
            mesh.add_rect_with_uv(rect, uv, tint);
        }
    }
    Shape::mesh(mesh)
}

/// 縦横比を保って、そのままの大きさで描く（ネジ・ランプ・アイコン）
fn image(painter: &Painter, tex: &TextureHandle, rect: Rect, tint: Color32) {
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    painter.image(tex.id(), rect, uv, tint);
}

/// 画面全体の配色（素材の有無にかかわらず適用する）
fn apply_visuals(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = CHASSIS;
    v.window_fill = PANEL;
    v.extreme_bg_color = PLOT;
    v.faint_bg_color = Color32::from_rgb(0x0a, 0x22, 0x1f);
    v.code_bg_color = PLOT;
    v.weak_text_color = Some(TEXT_SECONDARY);
    v.hyperlink_color = CYAN;
    v.warn_fg_color = AMBER;
    v.error_fg_color = DANGER;
    v.window_stroke = Stroke::new(1.25, BRASS);
    v.selection.bg_fill = SELECTED_FILL;
    v.selection.stroke = Stroke::new(1.0, CYAN);
    v.text_cursor.stroke = Stroke::new(2.0, CYAN);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3a, 0x26));
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.bg_fill = Color32::from_rgb(0x10, 0x2b, 0x28);
    w.inactive.weak_bg_fill = Color32::from_rgb(0x0f, 0x24, 0x21);
    w.inactive.bg_stroke = Stroke::new(1.0, BRASS_SHADOW);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.bg_fill = Color32::from_rgb(0x17, 0x39, 0x36);
    w.hovered.weak_bg_fill = Color32::from_rgb(0x17, 0x39, 0x36);
    w.hovered.bg_stroke = Stroke::new(1.0, BRASS);
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    w.active.bg_fill = SELECTED_FILL;
    w.active.weak_bg_fill = SELECTED_FILL;
    w.active.bg_stroke = Stroke::new(1.0, BRASS_LIGHT);
    w.active.fg_stroke = Stroke::new(2.0, TEXT);
    w.open = w.hovered;
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.spacing.button_padding = vec2(8.0, 3.0);
        s.spacing.item_spacing = vec2(8.0, 5.0);
    });
}

/// 筐体の背景（暗緑の上に金属テクスチャを重ねる）
pub fn paint_chassis(painter: &Painter, rect: Rect) {
    painter.rect_filled(rect, 0.0, CHASSIS);
    let Some(s) = get(painter.ctx()) else { return };
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(rect.width() / CHASSIS_TILE, rect.height() / CHASSIS_TILE));
    painter.image(s.chassis.id(), rect, uv, Color32::from_white_alpha((CHASSIS_OPACITY * 255.0) as u8));
}

/// 外周の四隅にネジを置く
pub fn paint_corner_screws(painter: &Painter, rect: Rect) {
    let Some(s) = get(painter.ctx()) else { return };
    let size = vec2(14.0, 14.0);
    let inset = 4.0;
    for (x, y) in [
        (rect.left() + inset, rect.top() + inset),
        (rect.right() - inset - size.x, rect.top() + inset),
        (rect.left() + inset, rect.bottom() - inset - size.y),
        (rect.right() - inset - size.x, rect.bottom() - inset - size.y),
    ] {
        image(painter, &s.screw, Rect::from_min_size(pos2(x, y), size), Color32::WHITE);
    }
}

/// 真鍮の枠（panel-frame）の中に中身を置く。中央は無地なので、グラフをそのまま載せてよい。
pub fn panel<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let skin = get(ui.ctx());
    let bg = ui.painter().add(Shape::Noop);
    let r = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    let rect = r.response.rect;
    // 中身の大きさが決まってから、先に確保した位置（中身の下）へ枠を差し込む
    let shape: Shape = match skin {
        Some(s) => nine_slice_shape(&s.panel, rect, PANEL_SLICE, Color32::WHITE),
        None => egui::epaint::RectShape::new(rect, 4, PANEL, Stroke::new(1.25, BRASS), StrokeKind::Inside).into(),
    };
    ui.painter().set(bg, shape);
    egui::InnerResponse::new(r.inner, r.response)
}

/// 見出しの真鍮銘板。幅は文字の実測幅 + 左右 26 pt
pub fn plaque(ui: &mut Ui, text: &str) -> Response {
    // 英字だけの見出しはセリフ体。日本語を含むときは本文のフォント（英字と日本語の基線をそろえる）
    let family = if text.is_ascii() { heading_family() } else { FontFamily::Proportional };
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(16.0, family), INK);
    let size = vec2(galley.size().x + 2.0 * PLAQUE_PAD_X, PLAQUE_H);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        match get(ui.ctx()) {
            Some(s) => nine_slice(painter, &s.plaque, rect, PLAQUE_SLICE, Color32::WHITE),
            None => {
                painter.rect(rect.shrink(2.0), 4.0, BRASS, Stroke::new(1.0, BRASS_SHADOW), StrokeKind::Inside);
            }
        }
        let pos = rect.center() - galley.size() / 2.0;
        painter.galley(pos, galley, INK);
    }
    resp
}

/// ブランドの銘板（英字のセリフ体 + 日本語の補足）
pub fn nameplate(ui: &mut Ui, brand: &str, sub: &str, brand_size: f32) -> Response {
    let painter = ui.painter();
    let g1 = painter.layout_no_wrap(brand.to_string(), FontId::new(brand_size, heading_family()), INK);
    let g2 = painter.layout_no_wrap(sub.to_string(), FontId::proportional((brand_size * 0.5).max(11.0)), INK);
    let text_w = g1.size().x.max(g2.size().x);
    let text_h = g1.size().y + g2.size().y;
    let size = vec2(text_w + 2.0 * PLAQUE_PAD_X, (text_h + 16.0).max(48.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        match get(ui.ctx()) {
            Some(s) => nine_slice(painter, &s.nameplate, rect, NAMEPLATE_SLICE, Color32::WHITE),
            None => {
                painter.rect(rect.shrink(2.0), 6.0, BRASS, Stroke::new(1.5, BRASS_SHADOW), StrokeKind::Inside);
            }
        }
        let top = rect.center().y - text_h / 2.0;
        let x1 = rect.center().x - g1.size().x / 2.0;
        let x2 = rect.center().x - g2.size().x / 2.0;
        let h1 = g1.size().y;
        painter.galley(pos2(x1, top), g1, INK);
        painter.galley(pos2(x2, top + h1), g2, INK);
    }
    resp
}

/// 状態表示のランプ（色だけに意味を持たせないよう、呼び出し側で文字も添える）
pub fn lamp(ui: &mut Ui, lamp: Lamp) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
    match get(ui.ctx()) {
        Some(s) => image(ui.painter(), s.lamp(lamp), rect, Color32::WHITE),
        None => {
            let c = match lamp {
                Lamp::Off => TEXT_DISABLED,
                Lamp::Amber => AMBER,
                Lamp::Teal => CYAN,
                Lamp::Red => DANGER,
            };
            ui.painter().circle_filled(rect.center(), 6.0, c);
        }
    }
    resp
}

/// 計器風のボタン。実際の egui のウィジェットとしてクリック領域（高さ 36 pt 以上）を確保し、
/// 有効／無効・キーボードでの押下・読み上げ用の名前と選択状態をそのまま持つ。
pub fn button(ui: &mut Ui, icon: Option<Icon>, label: &str, active: Active) -> Response {
    let skin = get(ui.ctx());
    let font = FontId::proportional(14.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, TEXT);
    let icon_size = 18.0;
    let pad_x = 14.0;
    let gap = if icon.is_some() && !label.is_empty() { 6.0 } else { 0.0 };
    let icon_w = if icon.is_some() { icon_size } else { 0.0 };
    let content_w = icon_w + gap + galley.size().x;
    let size = vec2((content_w + 2.0 * pad_x).max(MIN_HIT), MIN_HIT);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    let selected = active != Active::No;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, enabled, selected, label));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let pressed = resp.is_pointer_button_down_on();
    let hovered = resp.hovered();
    let look = button_look(enabled, active, pressed, hovered);
    let painter = ui.painter();
    match &skin {
        Some(s) => nine_slice(painter, s.button(look), rect, BUTTON_SLICE, Color32::WHITE),
        None => {
            let fill = match look {
                ButtonLook::Idle => Color32::from_rgb(0x10, 0x16, 0x14),
                ButtonLook::Hover => Color32::from_rgb(0x1c, 0x2a, 0x24),
                ButtonLook::Pressed => Color32::from_rgb(0x0a, 0x10, 0x0e),
                ButtonLook::ActiveAmber => Color32::from_rgb(0xc8, 0x7a, 0x1c),
                ButtonLook::ActiveTeal => Color32::from_rgb(0x0e, 0x6a, 0x66),
                ButtonLook::Disabled => Color32::from_rgb(0x14, 0x18, 0x16),
            };
            painter.rect(rect, 4.0, fill, Stroke::new(1.0, BRASS_SHADOW), StrokeKind::Inside);
        }
    }
    // 選択中にカーソルが乗ったら、点灯の素材は保ったまま外周だけ明るくする
    if selected && enabled && hovered {
        painter.rect_stroke(rect.shrink(1.0), CornerRadius::same(4), Stroke::new(1.5, BRASS_LIGHT), StrokeKind::Inside);
    }
    let fg = match look {
        ButtonLook::Disabled => TEXT_DISABLED,
        ButtonLook::ActiveAmber => INK,
        _ => TEXT,
    };
    // 押している間は中身を 1 pt 下げる
    let dy = if pressed && enabled { 1.0 } else { 0.0 };
    let mut x = rect.center().x - content_w / 2.0;
    let cy = rect.center().y + dy;
    if let Some(icon) = icon {
        let r = Rect::from_center_size(pos2(x + icon_size / 2.0, cy), Vec2::splat(icon_size));
        match &skin {
            // 明るいアイコンに乗算で色を付ける
            Some(s) => image(painter, s.icon(icon), r, fg),
            None => fallback_icon(painter, icon, r, fg),
        }
        x += icon_size + gap;
    }
    painter.galley_with_override_text_color(pos2(x, cy - galley.size().y / 2.0), galley, fg);
    if resp.has_focus() {
        // キーボードフォーカスは状態の素材とは別の 2 pt の枠
        painter.rect_stroke(rect.expand(2.0), CornerRadius::same(6), Stroke::new(2.0, FOCUS_RING), StrokeKind::Outside);
    }
    resp
}

/// 素材が無いときのアイコン（簡単な図形）
fn fallback_icon(painter: &Painter, icon: Icon, r: Rect, c: Color32) {
    let r = r.shrink(3.0);
    match icon {
        Icon::Play => {
            painter.add(Shape::convex_polygon(
                vec![r.left_top(), r.left_bottom(), pos2(r.right(), r.center().y)],
                c,
                Stroke::NONE,
            ));
        }
        Icon::Stop => {
            painter.rect_filled(r.shrink(1.0), 1.0, c);
        }
        _ => {
            painter.circle_stroke(r.center(), r.width() / 2.0, Stroke::new(1.5, c));
        }
    }
}

/// グラフの上端に置く解析位置の三角マーカー（縦線と同じ x）
pub fn cursor_marker(painter: &Painter, x: f32, top: f32, color: Color32) {
    let w = 6.0;
    painter.add(Shape::convex_polygon(
        vec![pos2(x - w, top - 8.0), pos2(x + w, top - 8.0), pos2(x, top)],
        color,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_state_priority() {
        // 無効が最優先、次に点灯、押下、ホバー
        assert_eq!(button_look(false, Active::Amber, true, true), ButtonLook::Disabled);
        assert_eq!(button_look(true, Active::Teal, true, true), ButtonLook::ActiveTeal);
        assert_eq!(button_look(true, Active::Amber, false, false), ButtonLook::ActiveAmber);
        assert_eq!(button_look(true, Active::No, true, true), ButtonLook::Pressed);
        assert_eq!(button_look(true, Active::No, false, true), ButtonLook::Hover);
        assert_eq!(button_look(true, Active::No, false, false), ButtonLook::Idle);
    }

    #[test]
    fn embedded_assets_decode() {
        let ctx = egui::Context::default();
        let s = SkinAssets::load(&ctx).expect("素材を読み込めること");
        assert_eq!(s.button(ButtonLook::Idle).size(), [288, 88]);
        assert_eq!(s.plaque.size(), [560, 64]);
        assert_eq!(s.nameplate.size(), [608, 128]);
        assert_eq!(s.panel.size(), [192, 192]);
        assert_eq!(s.icon(Icon::Help).size(), [48, 48]);
        assert_eq!(s.lamp(Lamp::Red).size(), [40, 40]);
    }
}
