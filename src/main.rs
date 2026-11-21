//! 倍音解析エディタ（初版）

use eframe::egui::{self, Color32, RichText, Stroke};
use egui_plot::{
    Bar, BarChart, FilledArea, HLine, HoverPosition, Legend, Line, LineStyle, Plot, PlotImage, PlotPoint,
    Points, Span, Text, VLine,
};
use harmonic_analyzer::analysis::{Analysis, FLOOR_DB, HarmonicLevel, WindowSize, analyze, auto_window};
use harmonic_analyzer::audio_file::{AudioClip, load_wav};
use harmonic_analyzer::note::{nearest_note, parse_pitch};
use harmonic_analyzer::filter::{self, Band, BandMode};
use harmonic_analyzer::playback::Player;
use std::collections::BTreeSet;
use std::path::PathBuf;

const CURSOR_COLOR: Color32 = Color32::from_rgb(255, 160, 40);
const PLAYHEAD_COLOR: Color32 = Color32::from_rgb(80, 220, 120);
const WINDOW_CHOICES: [usize; 6] = [512, 1024, 2048, 4096, 8192, 16384];
const PALETTE: [Color32; 10] = [
    Color32::from_rgb(86, 156, 255),
    Color32::from_rgb(255, 99, 99),
    Color32::from_rgb(120, 210, 90),
    Color32::from_rgb(220, 120, 255),
    Color32::from_rgb(255, 200, 60),
    Color32::from_rgb(60, 210, 210),
    Color32::from_rgb(255, 140, 190),
    Color32::from_rgb(170, 170, 255),
    Color32::from_rgb(200, 160, 100),
    Color32::from_rgb(160, 230, 180),
];

fn harmonic_color(k: usize) -> Color32 {
    PALETTE[(k - 1) % PALETTE.len()]
}

fn main() -> eframe::Result {
    let initial = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1440.0, 920.0]).with_title("倍音解析エディタ"),
        ..Default::default()
    };
    eframe::run_native(
        "倍音解析エディタ",
        options,
        Box::new(move |cc| {
            setup_fonts(&cc.egui_ctx);
            let mut app = App::default();
            if let Some(p) = initial {
                app.open(p);
            }
            Ok(Box::new(app))
        }),
    )
}

fn setup_fonts(ctx: &egui::Context) {
    let candidates = [
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "C:\\Windows\\Fonts\\YuGothM.ttc",
        "C:\\Windows\\Fonts\\meiryo.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ];
    let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else { return };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("jp".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        // 英数字と日本語のベースラインをそろえるため、日本語フォントを最優先にする
        fonts.families.entry(fam).or_default().insert(0, "jp".into());
    }
    ctx.set_fonts(fonts);
}

struct App {
    player: Player,
    path: Option<PathBuf>,
    clip: Option<AudioClip>,
    analysis: Option<Analysis>,
    error: Option<String>,

    f0_text: String,
    a4_hz: f64,
    cents: f64,
    window: WindowSize,

    /// 解析位置（秒）。常に解析フレームの中心時刻にそろえる。
    cursor: f64,
    selected: BTreeSet<usize>,
    focused: Option<usize>,
    bar_count: usize,
    display_floor: f32,
    show_harmonic_guides: bool,

    /// 試聴フィルタ（再生音だけに掛ける）
    filter_on: bool,
    band: Band,
    /// いまプレーヤーに入っている音に掛かっているフィルタ（None = 元の音）
    filter_applied: Option<Band>,
    /// スペクトログラム上で Shift+ドラッグ中の開始周波数
    band_drag_start: Option<f64>,

    spec_texture: Option<egui::TextureHandle>,
    spec_dirty: bool,
    /// ファイルを開くたびに増やし、プロットの表示範囲をリセットする
    generation: u64,
    /// 開発用: 環境変数 HA_SCREENSHOT=path.png で数フレーム後に画面を保存して終了する
    frames: u64,
}

impl Default for App {
    fn default() -> Self {
        App {
            player: Player::new(),
            path: None,
            clip: None,
            analysis: None,
            error: None,
            f0_text: "A3".into(),
            a4_hz: 440.0,
            cents: 0.0,
            window: WindowSize::Auto,
            cursor: 0.0,
            selected: [1, 2, 3].into_iter().collect(),
            focused: Some(1),
            bar_count: 32,
            display_floor: -100.0,
            show_harmonic_guides: true,
            filter_on: false,
            band: Band { low_hz: 600.0, high_hz: 900.0, mode: BandMode::Keep },
            filter_applied: None,
            band_drag_start: None,
            spec_texture: None,
            spec_dirty: true,
            generation: 0,
            frames: 0,
        }
    }
}

impl App {
    fn open(&mut self, path: PathBuf) {
        match load_wav(&path) {
            Ok(clip) => {
                self.player.set_clip(&clip.interleaved, clip.channels, clip.sample_rate);
                self.filter_applied = None; // 新しい音は元のまま入る。必要なら次のフレームで掛け直す
                self.cursor = (clip.duration() * 0.25).min(0.5);
                self.clip = Some(clip);
                self.path = Some(path);
                self.generation += 1;
                self.reanalyze();
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// フィルタ設定が変わっていて、マウスを離していれば、再生する音を作り直す
    fn sync_filter(&mut self, ctx: &egui::Context) {
        let Some(clip) = &self.clip else { return };
        let wanted = self.filter_on.then_some(self.band);
        if wanted == self.filter_applied || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let data = match &wanted {
            Some(b) => filter::apply(&clip.interleaved, clip.channels, clip.sample_rate, b),
            None => clip.interleaved.clone(),
        };
        self.player.replace_audio(data, clip.channels, clip.sample_rate);
        self.filter_applied = wanted;
    }

    /// 基音の指定（音名/Hz + チューニング補正）から実際に使う周波数
    fn resolved_f0(&self) -> Result<f64, String> {
        let p = parse_pitch(&self.f0_text, self.a4_hz)?;
        Ok(p.hz * 2f64.powf(self.cents / 1200.0))
    }

    fn reanalyze(&mut self) {
        let Some(clip) = &self.clip else { return };
        let f0 = match self.resolved_f0() {
            Ok(f) => f,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        match analyze(&clip.mono, clip.sample_rate, f0, self.window) {
            Ok(a) => {
                self.bar_count = self.bar_count.clamp(1, 128);
                self.selected.retain(|&k| a.is_in_range(k));
                self.cursor = a.frame_time(a.frame_at(self.cursor));
                self.analysis = Some(a);
                self.error = None;
                self.spec_dirty = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn set_cursor(&mut self, t: f64) {
        if let Some(a) = &self.analysis {
            self.cursor = a.frame_time(a.frame_at(t));
        } else {
            self.cursor = t.max(0.0);
        }
        if self.player.is_playing() {
            self.player.play_from(self.cursor);
        }
    }

    fn toggle_play(&mut self) {
        if self.player.is_playing() {
            self.player.stop();
        } else if self.clip.is_some() {
            self.player.play_from(self.cursor);
        }
    }

    fn rebuild_spec_texture(&mut self, ctx: &egui::Context) {
        let Some(a) = &self.analysis else { return };
        let s = &a.spectrogram;
        let lo = self.display_floor;
        let mut pixels = Vec::with_capacity(s.cols * s.rows);
        for r in (0..s.rows).rev() {
            for c in 0..s.cols {
                let v = s.db[r * s.cols + c];
                pixels.push(colormap(((v - lo) / (0.0 - lo)).clamp(0.0, 1.0)));
            }
        }
        let img = egui::ColorImage::new([s.cols, s.rows], pixels);
        self.spec_texture = Some(ctx.load_texture("spectrogram", img, egui::TextureOptions::LINEAR));
        self.spec_dirty = false;
    }
}

/// 黒 → 紫 → 橙 → 黄白（magma 風）
fn colormap(x: f32) -> Color32 {
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [0.0, 0.0, 0.02]),
        (0.3, [0.23, 0.06, 0.44]),
        (0.55, [0.71, 0.21, 0.47]),
        (0.8, [0.98, 0.53, 0.28]),
        (1.0, [0.99, 0.99, 0.75]),
    ];
    let i = STOPS.iter().position(|s| s.0 >= x).unwrap_or(4).max(1);
    let (x0, c0) = STOPS[i - 1];
    let (x1, c1) = STOPS[i];
    let t = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
    let ch = |j: usize| ((c0[j] + (c1[j] - c0[j]) * t) * 255.0) as u8;
    Color32::from_rgb(ch(0), ch(1), ch(2))
}

fn save_png(path: &str, img: &egui::ColorImage) {
    let Ok(file) = std::fs::File::create(path) else { return };
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.size[0] as u32, img.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    if let Ok(mut w) = enc.write_header() {
        let data: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
        let _ = w.write_image_data(&data);
    }
}

fn hover_pos(p: &HoverPosition<'_>) -> PlotPoint {
    match p {
        HoverPosition::NearDataPoint { position, .. } | HoverPosition::Elsewhere { position } => *position,
    }
}

fn fmt_db(v: f32) -> String {
    if v <= FLOOR_DB { format!("≤ {FLOOR_DB:.0} dB（下限）") } else { format!("{v:.1} dB") }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // ドラッグ&ドロップで開く
        if let Some(p) = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf())) {
            self.open(p);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) && !ctx.egui_wants_keyboard_input() {
            self.toggle_play();
        }
        if self.player.is_playing() {
            ctx.request_repaint();
        }
        if self.spec_dirty {
            self.rebuild_spec_texture(&ctx);
        }
        self.sync_filter(&ctx);
        self.frames += 1;
        if let Ok(path) = std::env::var("HA_SCREENSHOT") {
            if self.frames == 1 {
                if let Ok(t) = std::env::var("HA_CURSOR").map(|v| v.parse::<f64>().unwrap_or(0.0)) {
                    self.set_cursor(t);
                }
                // 例: HA_FILTER=600-900 または 600-900:cut
                if let Ok(f) = std::env::var("HA_FILTER") {
                    let (range, mode) = f.split_once(':').unwrap_or((&f, "keep"));
                    if let Some((lo, hi)) = range.split_once('-') {
                        self.band.low_hz = lo.parse().unwrap_or(0.0);
                        self.band.high_hz = hi.parse().unwrap_or(0.0);
                        self.band.mode = if mode == "cut" { BandMode::Remove } else { BandMode::Keep };
                        self.filter_on = true;
                    }
                }
                if let Ok(list) = std::env::var("HA_SELECT") {
                    self.selected = list.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                }
            }
            if self.frames == 15 {
                ctx.request_screenshot(move |img| {
                    save_png(&path, &img);
                    std::process::exit(0);
                });
            }
            ctx.request_repaint();
        }

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::right("harmonics").resizable(true).default_size(470.0).show(ui, |ui| self.harmonic_panel(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| self.time_views(ui));
    }
}

impl App {
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("WAV を開く…").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("WAV", &["wav", "wave"]).pick_file()
            {
                self.open(p);
            }
            let playing = self.player.is_playing();
            let label = if playing { "■ 停止" } else { "▶ 再生" };
            if ui.add_enabled(self.clip.is_some(), egui::Button::new(label)).on_hover_text("Space").clicked() {
                self.toggle_play();
            }
            if let (Some(p), Some(c)) = (&self.path, &self.clip) {
                ui.label(RichText::new(p.file_name().unwrap_or_default().to_string_lossy()).strong());
                ui.label(format!(
                    "{} Hz / {} ch / {} / {:.3} 秒",
                    c.sample_rate,
                    c.channels,
                    c.format_desc,
                    c.duration()
                ));
            } else {
                ui.label("WAV ファイルを開くか、ウィンドウへドロップしてください");
            }
            ui.separator();
            ui.label(RichText::new(format!("解析位置 {:.3} s", self.cursor)).color(CURSOR_COLOR));
            ui.label(RichText::new(format!("再生位置 {:.3} s", self.player.position())).color(PLAYHEAD_COLOR));
            if let Some(e) = &self.player.error {
                ui.colored_label(Color32::LIGHT_RED, format!("再生不可: {e}"));
            }
        });

        ui.horizontal(|ui| {
            let mut changed = false;
            ui.label("基音");
            let r = ui.add(egui::TextEdit::singleline(&mut self.f0_text).desired_width(80.0).hint_text("A3 / 220"));
            if r.lost_focus() || (r.changed() && parse_pitch(&self.f0_text, self.a4_hz).is_ok()) {
                changed = true;
            }
            ui.label("A4 =");
            changed |= ui
                .add(egui::DragValue::new(&mut self.a4_hz).range(400.0..=480.0).speed(0.1).suffix(" Hz"))
                .changed();
            ui.label("チューニング補正");
            changed |= ui
                .add(egui::Slider::new(&mut self.cents, -100.0..=100.0).suffix(" cent").fixed_decimals(1))
                .changed();
            if ui.small_button("0").clicked() {
                self.cents = 0.0;
                changed = true;
            }

            match self.resolved_f0() {
                Ok(f0) => {
                    let (name, dev) = nearest_note(f0, self.a4_hz);
                    ui.label(RichText::new(format!("使用周波数 {f0:.2} Hz")).strong())
                        .on_hover_text(format!("最も近い音名: {name} {dev:+.1} cent"));
                    ui.label(format!("（{name} {dev:+.1} cent）"));
                }
                Err(e) => {
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
            }

            ui.separator();
            ui.label("窓長");
            let sr = self.clip.as_ref().map(|c| c.sample_rate).unwrap_or(48000);
            let auto_n = self.resolved_f0().map(|f| auto_window(sr, f)).unwrap_or(2048);
            let text = |w: WindowSize| match w {
                WindowSize::Auto => format!("自動 ({auto_n})"),
                WindowSize::Fixed(n) => n.to_string(),
            };
            egui::ComboBox::from_id_salt("window").selected_text(text(self.window)).show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut self.window, WindowSize::Auto, text(WindowSize::Auto)).changed();
                for n in WINDOW_CHOICES {
                    changed |= ui.selectable_value(&mut self.window, WindowSize::Fixed(n), n.to_string()).changed();
                }
            });
            if let Some(a) = &self.analysis {
                let ms = |n: usize| n as f64 * 1000.0 / a.sample_rate as f64;
                ui.label(format!("窓 {:.1} ms / 間隔 {:.1} ms", ms(a.window), ms(a.hop)))
                    .on_hover_text("窓が長いほど周波数の分離が良く、短いほど立ち上がりを細かく追える");
            }
            if changed {
                self.reanalyze();
            }
        });

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "レベル基準: 0 dB = 振幅 1.0（フルスケール）の正弦波。録音全体・全倍音で共通。下限 {FLOOR_DB:.0} dB"
                ))
                .weak(),
            );
            ui.separator();
            ui.label("表示下限");
            if ui
                .add(egui::Slider::new(&mut self.display_floor, -120.0..=-30.0).suffix(" dB").integer())
                .changed()
            {
                self.spec_dirty = true;
            }
            ui.checkbox(&mut self.show_harmonic_guides, "スペクトログラムに倍音位置を表示");
            if let Some(e) = &self.error {
                ui.colored_label(Color32::LIGHT_RED, e);
            }
        });

        ui.horizontal(|ui| self.filter_controls(ui));
        ui.add_space(4.0);
    }

    fn filter_controls(&mut self, ui: &mut egui::Ui) {
        let nyq = self.clip.as_ref().map(|c| c.sample_rate as f64 / 2.0).unwrap_or(24000.0);
        ui.checkbox(&mut self.filter_on, RichText::new("試聴フィルタ").strong())
            .on_hover_text("再生する音だけに掛かる。解析・グラフは元の音のまま");
        ui.add_enabled_ui(self.filter_on, |ui| {
            egui::ComboBox::from_id_salt("band-mode")
                .selected_text(match self.band.mode {
                    BandMode::Keep => "範囲だけ聴く",
                    BandMode::Remove => "範囲をカットして聴く",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.band.mode, BandMode::Keep, "範囲だけ聴く");
                    ui.selectable_value(&mut self.band.mode, BandMode::Remove, "範囲をカットして聴く");
                });
            let hi = self.band.high_hz;
            ui.add(egui::DragValue::new(&mut self.band.low_hz).range(0.0..=hi).speed(1.0).suffix(" Hz").max_decimals(1));
            ui.label("〜");
            let lo = self.band.low_hz;
            ui.add(egui::DragValue::new(&mut self.band.high_hz).range(lo..=nyq).speed(1.0).suffix(" Hz").max_decimals(1));
            if let (Some(k), Some(a)) = (self.focused, &self.analysis)
                && a.is_in_range(k)
                && ui
                    .button(format!("第{k}倍音の周り"))
                    .on_hover_text("注目中の倍音 ± 基音の半分（隣の倍音の手前まで）")
                    .clicked()
            {
                self.band.low_hz = (k as f64 - 0.5) * a.f0;
                self.band.high_hz = ((k as f64 + 0.5) * a.f0).min(nyq);
            }
        });
        ui.label(
            RichText::new(format!(
                "スペクトログラム上で Shift+ドラッグでも範囲を指定できる。境界はなだらか（幅 {:.0}〜{:.0} Hz）",
                filter::transition_width(self.band.low_hz),
                filter::transition_width(self.band.high_hz)
            ))
            .weak(),
        );
        if self.filter_on && Some(self.band) != self.filter_applied && self.clip.is_some() {
            ui.spinner();
        }
    }

    fn harmonic_panel(&mut self, ui: &mut egui::Ui) {
        let Some(a) = &self.analysis else {
            ui.label("解析結果なし");
            return;
        };
        let frame = a.frame_at(self.cursor);
        let t = a.frame_time(frame);
        ui.heading(format!("倍音分布  t = {t:.3} s"));
        ui.horizontal(|ui| {
            ui.label("表示する倍音数");
            ui.add(egui::Slider::new(&mut self.bar_count, 1..=128));
        });
        ui.label(
            RichText::new(format!(
                "範囲内: 第1〜第{}倍音（{:.0} Hz 未満）。棒をクリックで選択／解除",
                a.num_in_range,
                a.nyquist()
            ))
            .weak(),
        );

        let n = self.bar_count;
        let floor = self.display_floor as f64;
        let bars: Vec<Bar> = (1..=n.min(a.num_in_range))
            .map(|k| {
                let db = a.level_db(frame, k).unwrap() as f64;
                let sel = self.selected.contains(&k);
                let fill = if sel { harmonic_color(k) } else { Color32::from_gray(140) };
                let mut b = Bar::new(k as f64, db - FLOOR_DB as f64)
                    .base_offset(FLOOR_DB as f64)
                    .width(0.7)
                    .fill(fill)
                    .name(format!("第{k}倍音"));
                if self.focused == Some(k) {
                    b = b.stroke(Stroke::new(2.0, Color32::WHITE));
                }
                b
            })
            .collect();

        let a_ref = a;
        let plot = Plot::new(("bars", self.generation))
            .height(ui.available_height() * 0.55)
            .x_axis_label("倍音番号")
            .y_axis_label("レベル (dB)")
            .default_x_bounds(0.3, n as f64 + 0.7)
            .default_y_bounds(floor, 3.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .allow_double_click_reset(false)
            .reset()
            .label_formatter(move |p| {
                let pos = hover_pos(p);
                let k = pos.x.round();
                if k < 1.0 || k > n as f64 {
                    return None;
                }
                let k = k as usize;
                Some(match a_ref.level(frame, k) {
                    HarmonicLevel::Level { db, .. } => {
                        format!("第{k}倍音  {:.1} Hz\n{}", a_ref.harmonic_hz(k), fmt_db(db))
                    }
                    HarmonicLevel::OutOfRange => format!("第{k}倍音  {:.1} Hz\n範囲外（fs/2 以上）", a_ref.harmonic_hz(k)),
                })
            });
        let resp = plot.show(ui, |pu| {
            pu.bar_chart(BarChart::new("levels", bars).allow_hover(false));
            if n > a.num_in_range {
                let x0 = a.num_in_range as f64 + 0.5;
                pu.span(
                    Span::new("範囲外", x0..=(n as f64 + 0.7))
                        .fill(Color32::from_rgba_unmultiplied(160, 160, 160, 40))
                        .border(Stroke::new(1.0, Color32::GRAY)),
                );
                pu.text(Text::new(
                    "oor",
                    PlotPoint::new((x0 + n as f64 + 0.7) / 2.0, (floor + 3.0) / 2.0),
                    RichText::new("範囲外\n(≥ fs/2)").color(Color32::GRAY),
                ));
            }
            pu.hline(HLine::new("floor", FLOOR_DB as f64).color(Color32::DARK_GRAY).style(LineStyle::dashed_dense()));
            if pu.response().clicked() { pu.pointer_coordinate() } else { None }
        });
        if let Some(p) = resp.inner {
            let k = p.x.round();
            if k >= 1.0 && k <= n as f64 {
                let k = k as usize;
                self.focused = Some(k);
                if a.is_in_range(k) && !self.selected.insert(k) {
                    self.selected.remove(&k);
                }
            }
        }

        // 選んだ倍音の読み取り
        ui.add_space(6.0);
        if let Some(k) = self.focused {
            let txt = match a.level(frame, k) {
                HarmonicLevel::Level { db, peak_hz } => format!(
                    "第{k}倍音   対象 {:.2} Hz   実測ピーク {:.2} Hz   {}",
                    a.harmonic_hz(k),
                    peak_hz,
                    fmt_db(db)
                ),
                HarmonicLevel::OutOfRange => {
                    format!("第{k}倍音   対象 {:.2} Hz   範囲外（fs/2 = {:.0} Hz 以上）", a.harmonic_hz(k), a.nyquist())
                }
            };
            ui.label(RichText::new(txt).strong());
        }
        ui.horizontal(|ui| {
            ui.label(format!("折れ線に重ねる倍音（{}）", self.selected.len()));
            if ui.small_button("全解除").clicked() {
                self.selected.clear();
            }
            if ui.small_button("1〜8").clicked() {
                self.selected = (1..=8.min(a.num_in_range)).collect();
            }
        });
        let mut remove = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("readout").striped(true).num_columns(5).show(ui, |ui| {
                ui.label("倍音");
                ui.label("対象周波数");
                ui.label("実測ピーク");
                ui.label(format!("レベル @ {t:.3} s"));
                ui.label("");
                ui.end_row();
                for &k in &self.selected {
                    if let HarmonicLevel::Level { db, peak_hz } = a.level(frame, k) {
                        ui.label(RichText::new(format!("■ 第{k}倍音")).color(harmonic_color(k)));
                        ui.label(format!("{:.2} Hz", a.harmonic_hz(k)));
                        ui.label(format!("{peak_hz:.2} Hz"));
                        ui.label(fmt_db(db));
                        if ui.small_button("×").clicked() {
                            remove = Some(k);
                        }
                        ui.end_row();
                    }
                }
            });
        });
        if let Some(k) = remove {
            self.selected.remove(&k);
        }
    }

    fn time_views(&mut self, ui: &mut egui::Ui) {
        let Some(clip) = &self.clip else {
            ui.centered_and_justified(|ui| ui.label("WAV ファイルを開いてください"));
            return;
        };
        let dur = clip.duration();
        let link = egui::Id::unique(("time-link", self.generation));
        let playhead = self.player.is_playing().then(|| self.player.position());
        let cursor = self.cursor;
        let floor = self.display_floor as f64;
        let total_h = ui.available_height();
        let mut clicked: Option<f64> = None;

        let time_plot = |name: &str, h: f32| {
            Plot::new((name.to_string(), self.generation))
                .height(h)
                .link_axis(link, [true, false])
                .link_cursor(link, [true, false])
                .default_x_bounds(0.0, dur)
                .allow_zoom([true, false])
                .allow_drag([true, false])
                .allow_scroll([true, false])
                .allow_boxed_zoom(false)
                .y_axis_min_width(64.0) // 3つのプロットの時間軸を横方向にそろえる
        };
        let draw_cursors = |pu: &mut egui_plot::PlotUi| {
            pu.vline(VLine::new("解析位置", cursor).color(CURSOR_COLOR).width(1.5));
            if let Some(p) = playhead {
                pu.vline(VLine::new("再生位置", p).color(PLAYHEAD_COLOR).width(1.5));
            }
        };
        let click_of = |pu: &egui_plot::PlotUi| {
            if pu.response().clicked() { pu.pointer_coordinate().map(|p| p.x) } else { None }
        };

        // 波形
        ui.label(RichText::new("波形（クリックで解析位置・再生開始位置を指定、ドラッグ/ホイールで移動・拡大）").weak());
        let mono = &clip.mono;
        let sr = clip.sample_rate as f64;
        let r = time_plot("wave", total_h * 0.2)
            .default_y_bounds(-1.0, 1.0)
            .y_axis_label("振幅")
            .label_formatter(|p| Some(format!("{:.3} s", hover_pos(p).x)))
            .show(ui, |pu| {
                let b = pu.plot_bounds();
                let (t0, t1) = (b.min()[0].max(0.0), b.max()[0].min(dur));
                if t1 > t0 {
                    let buckets = 1200usize;
                    let i0 = (t0 * sr) as usize;
                    let i1 = ((t1 * sr) as usize).min(mono.len());
                    let step = ((i1 - i0) as f64 / buckets as f64).max(1.0);
                    let mut xs = Vec::new();
                    let mut lo = Vec::new();
                    let mut hi = Vec::new();
                    let mut j = i0 as f64;
                    while (j as usize) < i1 {
                        let s = j as usize;
                        let e = ((j + step) as usize).min(i1).max(s + 1);
                        let seg = &mono[s..e];
                        xs.push(s as f64 / sr);
                        lo.push(seg.iter().cloned().fold(f32::MAX, f32::min) as f64);
                        hi.push(seg.iter().cloned().fold(f32::MIN, f32::max) as f64);
                        j += step;
                    }
                    pu.add(
                        FilledArea::new("波形", &xs, &lo, &hi)
                            .fill_color(Color32::from_rgb(110, 170, 255))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(110, 170, 255))),
                    );
                }
                draw_cursors(pu);
                click_of(pu)
            });
        clicked = clicked.or(r.inner);

        let Some(a) = &self.analysis else { return };

        // スペクトログラム
        ui.label(RichText::new("スペクトログラム（縦軸 Hz、色 = レベル。縦方向はドラッグ/ホイールで拡大）").weak());
        let f0 = a.f0;
        let nyq = a.nyquist();
        let fmax = (f0 * (self.bar_count.min(16) as f64 + 1.0)).clamp(1000.0, nyq); // 既定は第16倍音付近まで
        let spec = &a.spectrogram;
        let shift = ui.input(|i| i.modifiers.shift);
        let band = self.filter_on.then_some(self.band);
        let drag_start = self.band_drag_start;
        let r = time_plot("spec", total_h * 0.36)
            .default_y_bounds(0.0, fmax)
            .allow_zoom([true, true])
            .allow_drag([!shift, !shift])
            .allow_scroll([true, true])
            .y_axis_label("周波数 (Hz)")
            .label_formatter(|p| {
                let pos = hover_pos(p);
                let k = pos.y / f0;
                let v = spec.value_at(pos.x, pos.y).map(fmt_db).unwrap_or_default();
                Some(format!("{:.3} s\n{:.1} Hz（基音の {:.2} 倍）\n{v}", pos.x, pos.y, k))
            })
            .show(ui, |pu| {
                if let Some(tex) = &self.spec_texture {
                    let w = spec.cols as f64 * spec.col_dt;
                    let h = spec.rows as f64 * spec.row_hz;
                    let x0 = -spec.col_dt / 2.0;
                    let y0 = -spec.row_hz / 2.0;
                    pu.image(PlotImage::new(
                        "spec",
                        tex.id(),
                        PlotPoint::new(x0 + w / 2.0, y0 + h / 2.0),
                        [w as f32, h as f32],
                    ));
                }
                if self.show_harmonic_guides {
                    for k in 1..=self.bar_count.min(a.num_in_range) {
                        let sel = self.selected.contains(&k);
                        let c = if sel { harmonic_color(k) } else { Color32::from_rgba_unmultiplied(255, 255, 255, 50) };
                        pu.hline(
                            HLine::new(format!("第{k}倍音"), k as f64 * f0)
                                .color(c)
                                .width(if sel { 1.2 } else { 0.6 })
                                .style(LineStyle::dashed_loose()),
                        );
                    }
                }
                if let Some(b) = band {
                    let (fill, edge) = match b.mode {
                        BandMode::Keep => (Color32::from_rgba_unmultiplied(80, 220, 255, 35), Color32::from_rgb(80, 220, 255)),
                        BandMode::Remove => (Color32::from_rgba_unmultiplied(255, 80, 80, 45), Color32::from_rgb(255, 90, 90)),
                    };
                    pu.span(
                        Span::new("試聴フィルタ", b.low_hz..=b.high_hz)
                            .axis(egui_plot::Axis::Y)
                            .fill(fill)
                            .border(Stroke::new(1.5, edge)),
                    );
                }
                draw_cursors(pu);
                // Shift+ドラッグで試聴フィルタの範囲を指定
                let resp = pu.response();
                let y = pu.pointer_coordinate().map(|p| p.y.max(0.0));
                let band_drag = if shift && resp.drag_started() {
                    y.map(|y| (y, y, false))
                } else if let (Some(s), Some(y)) = (drag_start, y)
                    && (resp.dragged() || resp.drag_stopped())
                {
                    Some((s, y, resp.drag_stopped()))
                } else {
                    None
                };
                (click_of(pu), band_drag)
            });
        let (spec_click, band_drag) = r.inner;
        clicked = clicked.or(spec_click);
        if let Some((s, y, done)) = band_drag {
            self.band_drag_start = (!done).then_some(s);
            let (lo, hi) = (s.min(y), s.max(y).min(nyq));
            if hi - lo > 1.0 {
                self.band.low_hz = lo;
                self.band.high_hz = hi;
                self.filter_on = true;
            }
        }

        // 選んだ倍音の時間変化
        ui.label(RichText::new("選んだ倍音の時間変化（共通の dB 目盛り）").weak());
        let frame = a.frame_at(cursor);
        let r = time_plot("tracks", ui.available_height().max(120.0))
            .default_y_bounds(floor, 3.0)
            .x_axis_label("時間 (秒)")
            .y_axis_label("レベル (dB)")
            .legend(Legend::default())
            .label_formatter(|p| {
                let pos = hover_pos(p);
                Some(format!("{:.3} s\n{:.1} dB", pos.x, pos.y))
            })
            .show(ui, |pu| {
                for &k in &self.selected {
                    if let Some(pts) = a.track(k) {
                        let name = format!("第{k}倍音 {:.0} Hz", a.harmonic_hz(k));
                        pu.line(Line::new(name.clone(), pts).color(harmonic_color(k)).width(1.5));
                        if let Some(db) = a.level_db(frame, k) {
                            pu.points(
                                Points::new(name, vec![[a.frame_time(frame), db as f64]])
                                    .color(harmonic_color(k))
                                    .radius(4.0)
                                    .filled(true),
                            );
                        }
                    }
                }
                draw_cursors(pu);
                click_of(pu)
            });
        clicked = clicked.or(r.inner);

        if let Some(t) = clicked {
            self.set_cursor(t.clamp(0.0, dur));
        }
    }
}
