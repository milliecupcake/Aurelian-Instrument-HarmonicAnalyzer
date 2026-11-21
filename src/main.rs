//! 倍音解析エディタ（初版）

use eframe::egui::{self, Color32, RichText, Stroke};
use egui_plot::{
    Bar, BarChart, FilledArea, HLine, HoverPosition, Line, LineStyle, Plot, PlotImage, PlotPoint, Points, Span,
    Text, VLine,
};
use harmonic_analyzer::analysis::{Analysis, FLOOR_DB, HarmonicLevel, WindowSize, analyze, auto_window};
use harmonic_analyzer::audio_file::{AudioClip, load_wav, write_wav_mono};
use harmonic_analyzer::filter::{self, Band, BandMode};
use harmonic_analyzer::harmonics::{BAR_HALF_WIDTH, DEFAULT_ON, RangeOp, Shown, harmonics_in_span};
use harmonic_analyzer::live::{AppInfo, LiveInput, Source, audio_apps, input_device_names};
use harmonic_analyzer::note::{nearest_note, parse_pitch};
use harmonic_analyzer::playback::Player;
use harmonic_analyzer::test_tones;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// ライブ入力で保持・表示する長さ（秒）
const LIVE_SECONDS: f64 = 5.0;
/// ライブ入力の解析間隔
const LIVE_INTERVAL: Duration = Duration::from_millis(70);

type LiveResult = (Vec<f32>, u32, Result<Analysis, String>);

const CURSOR_COLOR: Color32 = Color32::from_rgb(255, 160, 40);
const PLAYHEAD_COLOR: Color32 = Color32::from_rgb(80, 220, 120);
const KEEP_COLOR: Color32 = Color32::from_rgb(80, 220, 255);
const REMOVE_COLOR: Color32 = Color32::from_rgb(255, 90, 90);
const LIVE_COLOR: Color32 = Color32::from_rgb(255, 110, 110);
/// OFF の倍音の棒（半透明の灰色）
const OFF_BAR_COLOR: Color32 = Color32::from_rgba_premultiplied(45, 45, 45, 75);
/// OFF の倍音の行の文字の濃さ（不透明度）
const OFF_TEXT_ALPHA: f32 = 0.35;
/// OFF の倍音の時間変化の線の不透明度
const OFF_LINE_ALPHA: f32 = 0.22;
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

/// 付属のサンプル（アプリの中で生成するので、配布用 .app でもファイル無しで開ける）
struct Sample {
    title: &'static str,
    about: &'static str,
    render: fn() -> Vec<f32>,
}

const SAMPLES: [Sample; 4] = [
    Sample {
        title: "220 Hz の正弦波",
        about: "基音だけの音。第2倍音以降は下限に張りつく",
        render: test_tones::sine_220,
    },
    Sample {
        title: "220 Hz ＋ 440 Hz",
        about: "第2倍音が基音より約 6 dB 小さい音",
        render: test_tones::sine_220_plus_half_440,
    },
    Sample {
        title: "第3倍音の減衰",
        about: "第3倍音だけが 2 秒で 40 dB 下がる。時間変化のグラフで右下がりの直線になる",
        render: test_tones::third_harmonic_decay,
    },
    Sample {
        title: "220 Hz ＋ 750 Hz（非整数倍）",
        about: "倍音の間にある成分。スペクトログラムで倍音位置の線から外れて見える",
        render: test_tones::inharmonic_750,
    },
];

fn harmonic_color(k: usize) -> Color32 {
    PALETTE[(k - 1) % PALETTE.len()]
}

/// 補足説明の文字色（無効状態の灰色より明るくする）
fn hint_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode { Color32::from_gray(178) } else { Color32::from_gray(70) }
}

fn hint(ui: &egui::Ui, text: impl Into<String>) -> RichText {
    RichText::new(text).color(hint_color(ui))
}

/// 折り返し配置の中に枠付きのまとまりを置く。前のフレームの幅を覚えておき、行に入らなければ改行する
fn wrap_group<R>(ui: &mut egui::Ui, name: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let id = egui::Id::unique(("wrap-group", name));
    let w: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
    let x = ui.cursor().min.x;
    if x > ui.max_rect().left() + 1.0 && x + w > ui.max_rect().right() {
        ui.end_row();
    }
    let r = ui.group(add);
    ui.data_mut(|d| d.insert_temp(id, r.response.rect.width()));
    r.inner
}

/// グラフの見出し（名前 + 補足。補足は狭いと省略し、全文はカーソルを乗せると出る）
fn section_title(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().size(15.0).color(ui.visuals().strong_text_color()));
        add(ui);
    });
}

fn main() -> eframe::Result {
    let initial = std::env::args().nth(1).map(PathBuf::from);
    // 開発用: HA_WIDTH=1070 で狭いウィンドウを確かめる
    let width = std::env::var("HA_WIDTH").ok().and_then(|v| v.parse().ok()).unwrap_or(1440.0);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([width, 920.0]).with_title("倍音解析エディタ"),
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

/// 試聴フィルタの境界線（下限／上限）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BandEdge {
    Low,
    High,
}

/// 倍音分布の上で範囲選択した横軸の位置（倍音番号。a = ドラッグ開始, b = 現在／終了）
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pick {
    a: f64,
    b: f64,
    dragging: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceTab {
    Device,
    App,
}

/// 境界線をつかめる距離（画面上のピクセル）
const EDGE_GRAB_PX: f32 = 6.0;
/// 解析位置・再生位置の縦線をつかめる距離（画面上のピクセル）
const SEEK_GRAB_PX: f32 = 6.0;

/// 時間グラフ上の縦線（解析位置・再生位置）のつかみ具合
#[derive(Clone, Copy, Debug, Default)]
struct SeekGrab {
    /// 線の近くにカーソルがある
    hover: bool,
    /// ドラッグ中の位置（秒）と、ドラッグが終わったか
    moved: Option<(f64, bool)>,
}

/// 縦線の近くでのドラッグを、グラフの移動ではなく位置の移動として扱う
fn seek_grab(pu: &egui_plot::PlotUi, lines: &[f64], dragging: bool) -> SeekGrab {
    let resp = pu.response();
    let hover = resp.hover_pos().is_some_and(|p| {
        lines.iter().any(|&t| (pu.screen_from_plot(PlotPoint::new(t, 0.0)).x - p.x).abs() <= SEEK_GRAB_PX)
    });
    let active = dragging || (hover && resp.drag_started());
    let moved = if active && (resp.drag_started() || resp.dragged() || resp.drag_stopped()) {
        // グラフの外へ出ても、横方向の位置だけは追う
        resp.interact_pointer_pos()
            .map(|p| pu.plot_from_screen(p).x)
            .map(|t| (t, resp.drag_stopped()))
    } else {
        None
    };
    SeekGrab { hover, moved }
}

/// 範囲選択した倍音の説明（例: 「第7〜第9倍音（3 件）」）
fn describe_pick(ks: &[usize]) -> String {
    match ks {
        [] => "範囲内に倍音がありません（0 件）".into(),
        [k] => format!("第{k}倍音（1 件）"),
        [a, .., b] => format!("第{a}〜第{b}倍音（{} 件）", ks.len()),
    }
}

struct App {
    player: Player,
    path: Option<PathBuf>,
    /// 付属サンプルを開いているとき、その番号
    sample: Option<usize>,
    clip: Option<AudioClip>,
    analysis: Option<Analysis>,
    error: Option<String>,

    f0_text: String,
    a4_hz: f64,
    cents: f64,
    window: WindowSize,

    /// 解析位置（秒）。常に解析フレームの中心時刻にそろえる。
    cursor: f64,
    /// 再生中に解析位置を再生位置へ追従させる
    follow: bool,
    /// 時間変化グラフに表示する倍音（一覧のトグル・範囲選択・凡例で共通）
    shown: Shown,
    /// 詳細を読んでいる倍音（表示の ON／OFF とは別）
    focused: Option<usize>,
    bar_count: usize,
    display_floor: f32,
    show_harmonic_guides: bool,
    /// ツールバーの詳細設定を出すか
    show_settings: bool,
    show_help: bool,

    /// 試聴フィルタ（再生音だけに掛ける）
    filter_on: bool,
    band: Band,
    /// いまプレーヤーに入っている音に掛かっているフィルタ（None = 元の音）
    filter_applied: Option<Band>,
    /// スペクトログラム上で Shift+ドラッグ中の開始周波数
    band_drag_start: Option<f64>,
    /// スペクトログラム上で境界線にカーソルが乗っているか（前フレーム）
    edge_hover: Option<BandEdge>,
    /// 境界線をドラッグ中
    edge_drag: Option<BandEdge>,
    /// 解析位置・再生位置の縦線にカーソルが乗っているか（前フレーム）
    seek_hover: bool,
    /// 縦線をドラッグ中
    seek_drag: bool,
    /// 倍音分布での範囲選択（確定するまでは ON／OFF に反映しない）
    pick: Option<Pick>,

    /// ライブ入力（動作中のみ Some）
    live: Option<LiveInput>,
    live_devices: Vec<String>,
    live_apps: Vec<AppInfo>,
    live_source: Source,
    /// 最後に解析へ回した時点の受信サンプル数
    live_seen: u64,
    live_last: Instant,
    /// 裏で走っている解析
    live_job: Option<mpsc::Receiver<LiveResult>>,
    /// 入力元を選ぶウィンドウ
    source_open: bool,
    source_tab: SourceTab,
    source_query: String,

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
            sample: None,
            clip: None,
            analysis: None,
            error: None,
            f0_text: "A3".into(),
            a4_hz: 440.0,
            cents: 0.0,
            window: WindowSize::Auto,
            cursor: 0.0,
            follow: false,
            shown: Shown::default(),
            focused: Some(1),
            bar_count: 32,
            display_floor: -100.0,
            show_harmonic_guides: true,
            show_settings: true,
            show_help: false,
            filter_on: false,
            band: Band { low_hz: 600.0, high_hz: 900.0, mode: BandMode::Keep },
            filter_applied: None,
            band_drag_start: None,
            edge_hover: None,
            edge_drag: None,
            seek_hover: false,
            seek_drag: false,
            pick: None,
            live: None,
            live_devices: Vec::new(),
            live_apps: Vec::new(),
            live_source: Source::Device(None),
            live_seen: 0,
            live_last: Instant::now(),
            live_job: None,
            source_open: false,
            source_tab: SourceTab::Device,
            source_query: String::new(),
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
                self.load_clip(clip);
                self.path = Some(path);
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn open_sample(&mut self, i: usize) {
        let clip = AudioClip::from_mono((SAMPLES[i].render)(), test_tones::SR);
        // サンプルはどれも基音 220 Hz
        self.f0_text = "220".into();
        self.cents = 0.0;
        self.load_clip(clip);
        self.sample = Some(i);
    }

    fn load_clip(&mut self, clip: AudioClip) {
        self.stop_live();
        self.player.set_clip(&clip.interleaved, clip.channels, clip.sample_rate);
        self.filter_applied = None; // 新しい音は元のまま入る。必要なら次のフレームで掛け直す
        self.cursor = (clip.duration() * 0.25).min(0.5);
        self.clip = Some(clip);
        self.path = None;
        self.sample = None;
        self.pick = None;
        self.generation += 1;
        self.reanalyze();
    }

    /// フィルタ設定が変わっていて、マウスを離していれば、再生する音を作り直す
    fn sync_filter(&mut self, ctx: &egui::Context) {
        if self.live.is_some() {
            return;
        }
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
            Ok(a) => self.set_analysis(a),
            Err(e) => self.error = Some(e),
        }
    }

    fn set_analysis(&mut self, a: Analysis) {
        self.bar_count = self.bar_count.clamp(1, 128);
        self.cursor = if self.live.is_some() {
            // 窓がすべて実際の音に収まる、最も新しいフレーム
            let dur = a.num_frames.saturating_sub(1) as f64 * a.hop as f64 / a.sample_rate as f64;
            a.frame_time(a.frame_at(dur - a.window as f64 / 2.0 / a.sample_rate as f64))
        } else {
            a.frame_time(a.frame_at(self.cursor))
        };
        self.analysis = Some(a);
        self.error = None;
        self.spec_dirty = true;
    }

    fn refresh_sources(&mut self) {
        self.live_devices = input_device_names();
        self.live_apps = audio_apps();
    }

    fn open_source_window(&mut self) {
        self.refresh_sources();
        self.source_tab = if matches!(self.live_source, Source::App(_)) { SourceTab::App } else { SourceTab::Device };
        self.source_open = true;
    }

    fn source_text(&self) -> String {
        match &self.live_source {
            Source::Device(None) => "既定の入力".to_string(),
            Source::Device(Some(n)) => n.clone(),
            Source::App(n) => format!("アプリ: {n}"),
        }
    }

    fn start_live(&mut self) {
        self.player.stop();
        match LiveInput::open(&self.live_source, LIVE_SECONDS) {
            Ok(live) => {
                let silent = vec![0.0; (LIVE_SECONDS * live.sample_rate as f64).round() as usize];
                self.clip = Some(AudioClip::from_mono(silent, live.sample_rate));
                self.path = None;
                self.sample = None;
                self.pick = None;
                self.live = Some(live);
                self.live_seen = 0;
                self.live_job = None;
                self.generation += 1;
                self.reanalyze();
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// ライブ入力を止め、直近の音をふつうの録音として残す（再生・解析・保存ができる）
    fn stop_live(&mut self) {
        if self.live.take().is_none() {
            return;
        }
        self.live_job = None;
        if let Some(clip) = &self.clip {
            self.player.set_clip(&clip.interleaved, clip.channels, clip.sample_rate);
            self.filter_applied = None;
        }
    }

    /// 解析結果を受け取り、間隔が空いていれば次の解析を裏で始める
    fn poll_live(&mut self) {
        if let Some(rx) = &self.live_job {
            match rx.try_recv() {
                Ok((samples, sr, res)) => {
                    self.live_job = None;
                    self.clip = Some(AudioClip::from_mono(samples, sr));
                    match res {
                        Ok(a) => self.set_analysis(a),
                        Err(e) => self.error = Some(e),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => self.live_job = None,
            }
        }
        let Some(live) = &self.live else { return };
        if self.live_last.elapsed() < LIVE_INTERVAL {
            return;
        }
        let (samples, total) = live.snapshot();
        if total == self.live_seen {
            return;
        }
        let Ok(f0) = self.resolved_f0() else { return };
        let (sr, window) = (live.sample_rate, self.window);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let res = analyze(&samples, sr, f0, window);
            let _ = tx.send((samples, sr, res));
        });
        self.live_job = Some(rx);
        self.live_seen = total;
        self.live_last = Instant::now();
    }

    fn set_cursor(&mut self, t: f64) {
        if self.live.is_some() {
            return; // ライブ中は常に最新の位置を見る
        }
        if let Some(a) = &self.analysis {
            self.cursor = a.frame_time(a.frame_at(t));
        } else {
            self.cursor = t.max(0.0);
        }
        if self.player.is_playing() {
            self.player.play_from(self.cursor);
        }
    }

    /// グラフのクリックやドラッグで解析位置を決めたとき。追従していたら固定に戻す。
    fn seek_by_hand(&mut self, t: f64) {
        self.follow = false;
        self.set_cursor(t);
    }

    fn toggle_play(&mut self) {
        if self.player.is_playing() {
            self.player.stop();
        } else if self.clip.is_some() && self.live.is_none() {
            // 最後のフレームから再生してもすぐ終わるので、先頭へ戻す
            if let Some(a) = &self.analysis
                && a.frame_at(self.cursor) + 1 >= a.num_frames
            {
                self.cursor = a.frame_time(0);
            }
            self.player.play_from(self.cursor);
        }
    }

    /// 追従モードで再生中なら、解析位置を再生位置のフレームに合わせる
    fn follow_playhead(&mut self) {
        if !self.follow || self.live.is_some() || self.seek_drag || !self.player.is_playing() {
            return;
        }
        if let Some(a) = &self.analysis {
            self.cursor = a.frame_time(a.frame_at(self.player.position()));
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
        if self.pick.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.pick = None;
        }
        if self.player.is_playing() || self.live.is_some() {
            ctx.request_repaint();
        }
        self.poll_live();
        self.follow_playhead();
        if self.spec_dirty {
            self.rebuild_spec_texture(&ctx);
        }
        self.sync_filter(&ctx);
        self.frames += 1;
        if let Ok(path) = std::env::var("HA_SCREENSHOT") {
            if self.frames == 1 {
                self.dev_setup();
            }
            // ライブ入力では音がたまるのを待つ（約2秒）
            if self.frames == if self.live.is_some() { 120 } else { 15 } {
                ctx.request_screenshot(move |img| {
                    save_png(&path, &img);
                    std::process::exit(0);
                });
            }
            ctx.request_repaint();
        }

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        if self.clip.is_some() {
            egui::Panel::right("harmonics").resizable(true).default_size(470.0).show(ui, |ui| self.harmonic_panel(ui));
        }
        egui::CentralPanel::default_margins().show(ui, |ui| {
            if self.clip.is_some() {
                self.time_views(ui);
            } else {
                self.start_screen(ui);
            }
        });
        self.source_window(&ctx);
        self.help_window(&ctx);
    }
}

impl App {
    /// 開発用: スクリーンショットの前に状態を整える（README の「開発用」を参照）
    fn dev_setup(&mut self) {
        let var = |k: &str| std::env::var(k).ok();
        if let Some(i) = var("HA_SAMPLE").and_then(|v| v.parse::<usize>().ok()).filter(|i| (1..=SAMPLES.len()).contains(i)) {
            self.open_sample(i - 1);
        }
        if let Some(t) = var("HA_CURSOR").map(|v| v.parse::<f64>().unwrap_or(0.0)) {
            self.set_cursor(t);
        }
        // 例: HA_FILTER=600-900 または 600-900:cut
        if let Some(f) = var("HA_FILTER") {
            let (range, mode) = f.split_once(':').unwrap_or((&f, "keep"));
            if let Some((lo, hi)) = range.split_once('-') {
                self.band.low_hz = lo.parse().unwrap_or(0.0);
                self.band.high_hz = hi.parse().unwrap_or(0.0);
                self.band.mode = if mode == "cut" { BandMode::Remove } else { BandMode::Keep };
                self.filter_on = true;
            }
        }
        if let Some(src) = var("HA_LIVE") {
            // HA_LIVE=1 は既定の入力、HA_LIVE=app:名前 はアプリの音
            if let Some(name) = src.strip_prefix("app:") {
                self.live_source = Source::App(name.to_string());
            }
            self.start_live();
        }
        if let Some(list) = var("HA_SELECT") {
            self.shown = Shown::from_iter(list.split(',').filter_map(|v| v.trim().parse().ok()));
        }
        // 例: HA_PICK=7-9（倍音番号）で、倍音分布を範囲選択して確定前の状態にする
        if let Some((a, b)) = var("HA_PICK").as_deref().and_then(|v| v.split_once('-')) {
            self.pick = Some(Pick { a: a.parse().unwrap_or(0.0), b: b.parse().unwrap_or(0.0), dragging: false });
        }
        // HA_SOURCES=1 で入力元のウィンドウを開く（HA_SOURCES=app でアプリの音のタブ）
        if let Some(v) = var("HA_SOURCES") {
            self.open_source_window();
            if v == "app" {
                self.source_tab = SourceTab::App;
            }
        }
        if var("HA_HELP").is_some() {
            self.show_help = true;
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        // 1 行目: ファイル・入力、再生、時刻、試聴の状態（常に見える）
        let mut label_below = false;
        ui.horizontal(|ui| {
            if ui.button("WAV を開く…").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("WAV", &["wav", "wave"]).pick_file()
            {
                self.open(p);
            }
            self.live_controls(ui);
            ui.separator();

            let playing = self.player.is_playing();
            let label = if playing { "■ 停止" } else { "▶ 再生" };
            let can_play = self.clip.is_some() && self.live.is_none();
            if ui.add_enabled(can_play, egui::Button::new(label)).on_hover_text("Space").clicked() {
                self.toggle_play();
            }
            ui.label(RichText::new(format!("解析 {:.3} s", self.cursor)).color(CURSOR_COLOR))
                .on_hover_text("解析位置（橙の縦線）");
            ui.label(RichText::new(format!("再生 {:.3} s", self.player.position())).color(PLAYHEAD_COLOR))
                .on_hover_text("再生位置（緑の縦線）");
            ui.add_enabled_ui(can_play, |ui| {
                ui.selectable_value(&mut self.follow, false, "固定")
                    .on_hover_text("解析位置を固定する。再生しても倍音分布は変わらない（比べるとき向き）");
                ui.selectable_value(&mut self.follow, true, "再生に追従").on_hover_text(
                    "再生中は、再生位置のフレームを解析位置にする。\n\
                     グラフをクリック・ドラッグして解析位置を決めると「固定」に戻る。止めた位置はそのまま残る",
                );
            });
            ui.separator();
            self.listen_status(ui);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("？ 操作方法").clicked() {
                    self.show_help = !self.show_help;
                }
                if self.clip.is_some() {
                    let label = if self.show_settings { "設定を隠す ▴" } else { "設定 ▾" };
                    if ui.button(label).on_hover_text("基音・窓長・表示・試聴フィルタの設定").clicked() {
                        self.show_settings = !self.show_settings;
                    }
                }
                if let Some(e) = &self.player.error {
                    ui.colored_label(Color32::LIGHT_RED, "再生不可").on_hover_text(e);
                }
                // 狭いときは名前を潰さず、次の行に出す
                if ui.available_width() >= 240.0 {
                    ui.separator();
                    self.source_label(ui);
                } else {
                    label_below = true;
                }
            });
        });
        if label_below {
            ui.horizontal(|ui| self.source_label(ui));
        }

        // 2 行目以降: 用途ごとの設定。幅が足りなければ折り返す
        if self.clip.is_some() && self.show_settings {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                let mut changed = self.pitch_group(ui);
                changed |= self.window_group(ui);
                if changed {
                    self.reanalyze();
                }
                self.display_group(ui);
                self.filter_group(ui);
            });
        }
        // 基音の読み取りエラーは設定欄にも出るので、設定を出しているときは重ねない
        let f0_err = self.resolved_f0().err();
        if let Some(e) = &self.error
            && !(self.show_settings && self.clip.is_some() && f0_err.as_ref() == Some(e))
        {
            ui.colored_label(Color32::LIGHT_RED, e);
        }
        ui.add_space(4.0);
    }

    /// いま何を解析しているか（長い名前は省略し、全文はカーソルを乗せると出る）
    fn source_label(&mut self, ui: &mut egui::Ui) {
        let (text, tip) = if let Some(live) = &self.live {
            (format!("● {}  {} Hz", live.device_name, live.sample_rate), "ライブ入力中".to_string())
        } else if let (Some(p), Some(c)) = (&self.path, &self.clip) {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            let info = format!("{} Hz / {} ch / {} / {:.3} 秒", c.sample_rate, c.channels, c.format_desc, c.duration());
            (format!("{name}   {info}"), format!("{}\n{info}", p.display()))
        } else if let (Some(i), Some(c)) = (self.sample, &self.clip) {
            let s = &SAMPLES[i];
            (format!("サンプル: {}   {} Hz / {:.3} 秒", s.title, c.sample_rate, c.duration()), s.about.to_string())
        } else if let Some(c) = &self.clip {
            if ui.button("WAV に保存…").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).set_file_name("live.wav").save_file()
            {
                match write_wav_mono(&p, &c.mono, c.sample_rate) {
                    Ok(()) => self.path = Some(p),
                    Err(e) => self.error = Some(e),
                }
            }
            (
                format!("ライブ入力から取り込んだ音   {} Hz / {:.3} 秒", c.sample_rate, c.duration()),
                "止めた時点の直近の音。再生・解析・保存できる".to_string(),
            )
        } else {
            return;
        };
        let color = if self.live.is_some() { LIVE_COLOR } else { ui.visuals().strong_text_color() };
        ui.add(egui::Label::new(RichText::new(text).color(color)).truncate()).on_hover_text(tip);
    }

    /// 再生音に何が掛かっているか（グラフは常に元の音）
    fn listen_status(&self, ui: &mut egui::Ui) {
        let (lo, hi) = (self.band.low_hz, self.band.high_hz);
        let (text, color, tip) = if self.live.is_some() {
            (
                "試聴: ライブ中は使えない".to_string(),
                hint_color(ui),
                "ライブ入力中は再生と試聴フィルタを使えない（ハウリング防止）。停止すると使える",
            )
        } else if !self.filter_on {
            ("試聴: 元の音".to_string(), ui.visuals().text_color(), "再生音にフィルタは掛かっていない")
        } else {
            match self.band.mode {
                BandMode::Keep => (format!("試聴: {lo:.0}–{hi:.0} Hz だけ"), KEEP_COLOR, "この帯域だけを鳴らしている"),
                BandMode::Remove => (format!("試聴: {lo:.0}–{hi:.0} Hz を除く"), REMOVE_COLOR, "この帯域を取り除いて鳴らしている"),
            }
        };
        ui.label(RichText::new(text).color(color)).on_hover_text(format!("{tip}\n解析・グラフは常に元の音"));
        if self.filter_on && self.live.is_none() && Some(self.band) != self.filter_applied && self.clip.is_some() {
            ui.spinner().on_hover_text("フィルタを掛けた音を準備中");
        }
    }

    /// 基音とチューニング。解析し直す必要があれば true
    fn pitch_group(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        wrap_group(ui, "pitch", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("基音").strong());
                let r = ui.add(egui::TextEdit::singleline(&mut self.f0_text).desired_width(70.0).hint_text("A3 / 220"));
                if r.lost_focus() || (r.changed() && parse_pitch(&self.f0_text, self.a4_hz).is_ok()) {
                    changed = true;
                }
                r.on_hover_text("音名（A3, C#4, Bb2）または周波数（220, 220 Hz）");
                ui.label("A4 =");
                changed |= ui
                    .add(egui::DragValue::new(&mut self.a4_hz).range(400.0..=480.0).speed(0.1).suffix(" Hz"))
                    .changed();
                ui.label("補正");
                changed |= ui
                    .add(egui::Slider::new(&mut self.cents, -100.0..=100.0).suffix(" cent").fixed_decimals(1))
                    .on_hover_text("チューニング補正（録音の音程に合わせる）")
                    .changed();
                if ui.add_enabled(self.cents != 0.0, egui::Button::new("補正をリセット").small()).clicked() {
                    self.cents = 0.0;
                    changed = true;
                }
                match self.resolved_f0() {
                    Ok(f0) => {
                        let (name, dev) = nearest_note(f0, self.a4_hz);
                        ui.label(RichText::new(format!("使用 {f0:.2} Hz")).strong())
                            .on_hover_text("実際に解析に使う基音の周波数");
                        ui.label(hint(ui, format!("（{name} {dev:+.1} cent）")));
                    }
                    Err(e) => {
                        ui.colored_label(Color32::LIGHT_RED, e);
                    }
                }
            });
        });
        changed
    }

    fn window_group(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        wrap_group(ui, "window", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("窓長").strong());
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
                    ui.label(hint(ui, format!("{:.1} ms / 間隔 {:.1} ms", ms(a.window), ms(a.hop))))
                        .on_hover_text("窓が長いほど周波数の分離が良く、短いほど立ち上がりを細かく追える");
                }
            });
        });
        changed
    }

    fn display_group(&mut self, ui: &mut egui::Ui) {
        wrap_group(ui, "display", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("表示").strong());
                ui.label("下限");
                if ui
                    .add(egui::Slider::new(&mut self.display_floor, -120.0..=-30.0).suffix(" dB").integer())
                    .on_hover_text(format!(
                        "グラフの dB 目盛りの下端。\n0 dB = 振幅 1.0（フルスケール）の正弦波。録音全体・全倍音で共通。\
                         解析の下限は {FLOOR_DB:.0} dB"
                    ))
                    .changed()
                {
                    self.spec_dirty = true;
                }
                ui.checkbox(&mut self.show_harmonic_guides, "倍音位置の線")
                    .on_hover_text("スペクトログラムに各倍音の周波数を破線で重ねる");
            });
        });
    }

    fn filter_group(&mut self, ui: &mut egui::Ui) {
        let live = self.live.is_some();
        let nyq = self.clip.as_ref().map(|c| c.sample_rate as f64 / 2.0).unwrap_or(24000.0);
        wrap_group(ui, "filter", |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!live, |ui| {
                    ui.checkbox(&mut self.filter_on, RichText::new("試聴フィルタ").strong())
                        .on_hover_text("再生する音だけに掛かる。解析・グラフは元の音のまま");
                    ui.add_enabled_ui(self.filter_on, |ui| {
                        ui.selectable_value(&mut self.band.mode, BandMode::Keep, "この帯域だけ聴く")
                            .on_hover_text("指定した帯域だけを残す（バンドパス）。スペクトログラムでは水色");
                        ui.selectable_value(&mut self.band.mode, BandMode::Remove, "この帯域を除く")
                            .on_hover_text("指定した帯域だけを取り除く（バンドストップ）。スペクトログラムでは赤");
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
                });
                if live {
                    ui.label(hint(ui, "ライブ中は使えない（停止すると使える）"));
                } else {
                    ui.label(hint(ui, "？")).on_hover_text(format!(
                        "スペクトログラムで Shift+ドラッグすると範囲を指定できる。帯の上下の線は個別にドラッグできる。\n\
                         境界はなだらかで、境界の幅は {:.0}〜{:.0} Hz",
                        filter::transition_width(self.band.low_hz),
                        filter::transition_width(self.band.high_hz)
                    ));
                }
            });
        });
    }

    fn live_controls(&mut self, ui: &mut egui::Ui) {
        let Some(live) = &self.live else {
            if ui.button("● ライブ入力…").on_hover_text("入力元を選んでから、リアルタイムに解析する").clicked() {
                self.open_source_window();
            }
            return;
        };
        if ui
            .button(RichText::new("■ ライブ停止").color(LIVE_COLOR))
            .on_hover_text("止めた時点の直近の音が残り、再生・解析・保存できる")
            .clicked()
        {
            self.stop_live();
            return;
        }
        let peak = live.peak();
        let db = 20.0 * peak.max(1e-6).log10();
        let frac = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let color = if peak >= 0.99 { Color32::RED } else { PLAYHEAD_COLOR };
        ui.add(egui::ProgressBar::new(frac).desired_width(90.0).fill(color).text(format!("{db:.0} dBFS")))
            .on_hover_text("入力レベル（直近のピーク）。赤は振り切れ");
        if let Some(e) = live.error() {
            ui.colored_label(Color32::LIGHT_RED, "入力エラー").on_hover_text(e);
        }
        if live.is_app() && live.silent_so_far() {
            ui.colored_label(Color32::from_rgb(255, 200, 60), "無音のまま").on_hover_text(
                "アプリが音を出していないか、システムオーディオ録音が許可されていません。\n\
                 システム設定 > プライバシーとセキュリティ > 画面収録とシステムオーディオ録音 で、\n\
                 このアプリ（.app から起動した場合）か起動元のターミナルを許可してください。\n\
                 アプリを選んだ後に始まった音のプロセスは入らないので、そのときは選び直してください。",
            );
        }
    }

    /// 入力元を選ぶウィンドウ。選んでから「開始」で始める
    fn source_window(&mut self, ctx: &egui::Context) {
        if !self.source_open || self.live.is_some() {
            return;
        }
        let mut open = true;
        let (mut start, mut close, mut refresh) = (false, false, false);
        egui::Window::new("ライブ入力の入力元")
            .open(&mut open)
            .collapsible(false)
            .default_size([460.0, 480.0])
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.source_tab, SourceTab::Device, "入力デバイス")
                        .on_hover_text("マイク、オーディオインターフェイス、BlackHole などの仮想デバイス");
                    if cfg!(target_os = "macos") {
                        ui.selectable_value(&mut self.source_tab, SourceTab::App, "アプリの音")
                            .on_hover_text("ブラウザなど、特定のアプリが出している音だけを取り込む（macOS 14.2 以降）");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        refresh = ui.button("一覧を更新").clicked();
                    });
                });
                ui.horizontal(|ui| {
                    ui.label("絞り込み");
                    ui.add(egui::TextEdit::singleline(&mut self.source_query).desired_width(220.0).hint_text("名前の一部"));
                    if !self.source_query.is_empty() && ui.small_button("解除").clicked() {
                        self.source_query.clear();
                    }
                });
                ui.separator();
                let q = self.source_query.to_lowercase();
                let matches = |n: &str| q.is_empty() || n.to_lowercase().contains(&q);
                let none_matched = |ui: &mut egui::Ui| {
                    ui.label(hint(ui, format!("「{}」に一致する入力元がありません", self.source_query)));
                };
                egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, false]).show(ui, |ui| match self.source_tab {
                    SourceTab::Device => {
                        let mut any = false;
                        if matches("既定の入力") {
                            any = true;
                            ui.selectable_value(&mut self.live_source, Source::Device(None), "既定の入力")
                                .on_hover_text("システム設定 > サウンド で選んでいる入力");
                        }
                        for name in self.live_devices.iter().filter(|n| matches(n)) {
                            any = true;
                            ui.selectable_value(&mut self.live_source, Source::Device(Some(name.clone())), name);
                        }
                        if !any {
                            none_matched(ui);
                        }
                    }
                    SourceTab::App => {
                        if self.live_apps.is_empty() {
                            ui.label(hint(
                                ui,
                                "音を出したことのあるアプリがありません。\n取り込みたいアプリで音を鳴らしてから「一覧を更新」を押してください。",
                            ));
                            return;
                        }
                        let row = |ui: &mut egui::Ui, source: &mut Source, app: &AppInfo| {
                            let mut text = RichText::new(format!("{} {}", if app.playing { "●" } else { "○" }, app.name));
                            if app.playing {
                                text = text.color(PLAYHEAD_COLOR);
                            }
                            ui.horizontal(|ui| {
                                ui.selectable_value(source, Source::App(app.name.clone()), text);
                                if app.playing {
                                    ui.label(RichText::new("発音中").color(PLAYHEAD_COLOR).small());
                                }
                                if app.name.contains("WebKit") {
                                    ui.label(hint(ui, "Safari などの Web ページの音").small());
                                }
                            });
                        };
                        let (apps, internal): (Vec<&AppInfo>, Vec<&AppInfo>) =
                            self.live_apps.iter().filter(|a| matches(&a.name)).partition(|a| a.bundled);
                        if apps.is_empty() && internal.is_empty() {
                            none_matched(ui);
                        } else if apps.is_empty() {
                            ui.label(hint(ui, "一致するアプリはありません（内部プロセスは下の「詳細」）"));
                        }
                        for app in apps {
                            row(ui, &mut self.live_source, app);
                        }
                        let all_internal = self.live_apps.iter().filter(|a| !a.bundled).count();
                        if all_internal > 0 {
                            ui.add_space(4.0);
                            egui::CollapsingHeader::new(format!("詳細: システムの内部プロセス（{}）", internal.len()))
                                .id_salt("internal-processes")
                                .open((!q.is_empty() && !internal.is_empty()).then_some(true))
                                .show(ui, |ui| {
                                    ui.label(hint(ui, "ふつうは選ぶ必要はありません。Safari の音は WebKit の名前で出ることがあります"));
                                    for app in internal {
                                        row(ui, &mut self.live_source, app);
                                    }
                                });
                        }
                    }
                });
                ui.separator();
                if self.source_tab == SourceTab::App {
                    ui.label(hint(
                        ui,
                        "アプリの音には「システムオーディオ録音」の許可が要ります（.app から起動した場合）。\n\
                         選んだ後に新しく音を出し始めたプロセスは入らないので、そのときは選び直してください。",
                    ));
                }
                ui.label(RichText::new(format!("選択中: {}", self.source_text())).strong());
                if let Some(e) = &self.error {
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
                ui.horizontal(|ui| {
                    start = ui.button(RichText::new("● この入力元で開始").color(LIVE_COLOR)).clicked();
                    close = ui.button("閉じる").clicked();
                });
            });
        if refresh {
            self.refresh_sources();
        }
        if start {
            self.start_live();
        }
        if !open || close {
            self.source_open = false;
        }
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        egui::Window::new("操作方法").open(&mut open).default_width(520.0).show(ctx, |ui| {
            let items: [(&str, &[&str]); 5] = [
                (
                    "波形・スペクトログラム・倍音の時間変化（時間軸は共通）",
                    &[
                        "クリック: 解析位置（橙）を移す。再生中ならそこから鳴り直す",
                        "橙・緑の縦線をドラッグ: 解析位置・再生位置を動かす（シーク）",
                        "ほかの所をドラッグ／ホイール: 表示の移動・拡大",
                    ],
                ),
                (
                    "スペクトログラム",
                    &[
                        "Shift+ドラッグ: 試聴フィルタの帯域を指定",
                        "帯の上下の線をドラッグ: 帯域の下限・上限を個別に動かす",
                    ],
                ),
                (
                    "倍音分布と一覧",
                    &[
                        "棒・一覧の名前をクリック: その倍音に注目する（詳細表示、「第k倍音の周り」）",
                        "棒の上を横にドラッグ: 範囲内の倍音を選び、まとめて ON／OFF（Esc で取り消し）",
                        "一覧のチェック: 時間変化のグラフに表示する／しない",
                    ],
                ),
                ("再生", &["Space: 再生／停止", "「再生に追従」: 再生中は解析位置が再生位置についていく"]),
                (
                    "レベルの基準",
                    &["0 dB = 振幅 1.0（フルスケール）の正弦波。録音全体・全倍音で共通。下限は −120 dB"],
                ),
            ];
            for (title, lines) in items {
                ui.label(RichText::new(title).strong());
                for l in lines {
                    ui.label(format!("・{l}"));
                }
                ui.add_space(6.0);
            }
        });
        self.show_help = open;
    }

    fn start_screen(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.08).max(12.0));
                ui.label(RichText::new("倍音解析エディタ").size(26.0).strong());
                ui.label(hint(ui, "単音を読み込むと、倍音ごとの強さと時間変化を調べられます"));
                ui.add_space(20.0);
                let big = |text: &str| egui::Button::new(RichText::new(text).size(16.0)).min_size(egui::vec2(300.0, 36.0));
                if ui.add(big("WAV を開く…")).clicked()
                    && let Some(p) = rfd::FileDialog::new().add_filter("WAV", &["wav", "wave"]).pick_file()
                {
                    self.open(p);
                }
                ui.label(hint(ui, "WAV ファイルをこのウィンドウへドラッグ＆ドロップしても開けます"));
                ui.add_space(10.0);
                if ui.add(big("● ライブ入力…")).clicked() {
                    self.open_source_window();
                }
                ui.label(hint(ui, "マイク・オーディオインターフェイス・アプリの音を、入力元を確かめてから開始"));
                ui.add_space(22.0);
                ui.label(RichText::new("サンプルで試す（基音 220 Hz）").strong());
                ui.add_space(4.0);
                for (i, s) in SAMPLES.iter().enumerate() {
                    if ui.add(egui::Button::new(s.title).min_size(egui::vec2(300.0, 28.0))).clicked() {
                        self.open_sample(i);
                    }
                    ui.label(hint(ui, s.about));
                    ui.add_space(4.0);
                }
            });
        });
    }

    fn harmonic_panel(&mut self, ui: &mut egui::Ui) {
        let Some(a) = &self.analysis else {
            ui.label(hint(ui, "解析結果なし"));
            return;
        };
        let frame = a.frame_at(self.cursor);
        let t = a.frame_time(frame);
        ui.horizontal(|ui| {
            ui.label(RichText::new("倍音分布").strong().size(17.0).color(ui.visuals().strong_text_color()));
            ui.label(RichText::new(format!("t = {t:.3} s")).size(15.0).color(CURSOR_COLOR));
        });
        ui.horizontal(|ui| {
            ui.label("棒の数");
            ui.add(egui::Slider::new(&mut self.bar_count, 1..=128));
        });
        ui.label(hint(
            ui,
            format!(
                "棒をクリックで注目、横にドラッグで範囲選択。範囲内: 第1〜第{}倍音（{:.0} Hz 未満）",
                a.num_in_range,
                a.nyquist()
            ),
        ));

        let n = self.bar_count;
        let floor = self.display_floor as f64;
        // 範囲選択できるのは、見えている棒のうち解析範囲内のもの
        let max_k = n.min(a.num_in_range);
        let pick = self.pick;
        let picking = pick.is_some_and(|p| p.dragging);
        let pick_ks = pick.map(|p| harmonics_in_span(p.a, p.b, max_k)).unwrap_or_default();
        let bars: Vec<Bar> = (1..=n.min(a.num_in_range))
            .map(|k| {
                let db = a.level_db(frame, k).unwrap() as f64;
                let on = self.shown.contains(k);
                // OFF の倍音は半透明の灰色にして、ON の倍音を目立たせる
                let fill = if on { harmonic_color(k) } else { OFF_BAR_COLOR };
                let mut b = Bar::new(k as f64, db - FLOOR_DB as f64)
                    .base_offset(FLOOR_DB as f64)
                    .width(2.0 * BAR_HALF_WIDTH)
                    .fill(fill)
                    .name(format!("第{k}倍音"));
                if self.focused == Some(k) {
                    b = b.stroke(Stroke::new(2.5, Color32::WHITE));
                }
                b
            })
            .collect();

        let a_ref = a;
        let plot = Plot::new(("bars", self.generation))
            .height(ui.available_height() * 0.42)
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
            // 範囲選択のプレビュー（確定するまで ON／OFF は変えない）
            if let Some(p) = pick {
                pu.span(
                    Span::new("", p.a.min(p.b)..=p.a.max(p.b))
                        .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 24))
                        .border(Stroke::new(1.0, Color32::WHITE)),
                );
                let marks: Vec<Bar> = pick_ks
                    .iter()
                    .map(|&k| {
                        Bar::new(k as f64, 3.0 - floor)
                            .base_offset(floor)
                            .width(2.0 * BAR_HALF_WIDTH)
                            .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 40))
                            .stroke(Stroke::new(1.5, Color32::WHITE))
                    })
                    .collect();
                pu.bar_chart(BarChart::new("picked", marks).allow_hover(false));
            }
            let resp = pu.response();
            // 横にドラッグして倍音を範囲選択: (新しく始めたなら開始位置, 現在の位置, 終了したか)
            let x = resp.interact_pointer_pos().map(|p| pu.plot_from_screen(p).x);
            let pick_move = if resp.drag_started() {
                x.map(|x| (Some(x), x, false))
            } else if picking && (resp.dragged() || resp.drag_stopped()) {
                x.map(|x| (None, x, resp.drag_stopped()))
            } else {
                None
            };
            let click = if resp.clicked() { pu.pointer_coordinate() } else { None };
            (click, pick_move)
        });
        let (click, pick_move) = resp.inner;
        // 棒のクリックは「注目」だけ。表示の ON／OFF は一覧のチェックか範囲選択で変える
        if let Some(p) = click {
            let k = p.x.round();
            if k >= 1.0 && k <= n as f64 {
                self.focused = Some(k as usize);
            }
        }
        if let Some((start, x, done)) = pick_move {
            let a0 = start.or(self.pick.map(|p| p.a)).unwrap_or(x);
            self.pick = Some(Pick { a: a0, b: x, dragging: !done });
        } else if let Some(p) = &mut self.pick
            && p.dragging
            && !ui.input(|i| i.pointer.any_down())
        {
            p.dragging = false;
        }
        if resp.response.hovered() || picking {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        // 範囲選択の確定
        let pick = self.pick;
        let pick_ks = pick.map(|p| harmonics_in_span(p.a, p.b, max_k)).unwrap_or_default();
        if let Some(p) = pick {
            let mut op = None;
            ui.horizontal_wrapped(|ui| {
                let head = if p.dragging { "範囲選択中:" } else { "範囲選択:" };
                ui.label(RichText::new(format!("{head} {}", describe_pick(&pick_ks))).strong());
                let ready = !p.dragging && !pick_ks.is_empty();
                for (o, label, tip) in [
                    (RangeOp::Only, "選択のみ ON", "選んだ倍音だけを ON にし、ほかはすべて OFF にする"),
                    (RangeOp::Add, "追加 ON", "選んだ倍音を ON にする。ほかはそのまま"),
                    (RangeOp::Off, "選択を OFF", "選んだ倍音を OFF にする。ほかはそのまま"),
                ] {
                    if ui.add_enabled(ready, egui::Button::new(label)).on_hover_text(tip).clicked() {
                        op = Some(o);
                    }
                }
                if ui.button("キャンセル").on_hover_text("Esc").clicked() {
                    self.pick = None;
                }
            });
            if let Some(o) = op {
                self.shown.apply(&pick_ks, o);
                self.pick = None;
            }
        }

        // 注目中の倍音の読み取り
        ui.add_space(6.0);
        if let Some(k) = self.focused {
            let txt = match a.level(frame, k) {
                HarmonicLevel::Level { db, peak_hz } => format!(
                    "▶ 第{k}倍音   対象 {:.2} Hz   実測ピーク {:.2} Hz   {}",
                    a.harmonic_hz(k),
                    peak_hz,
                    fmt_db(db)
                ),
                HarmonicLevel::OutOfRange => {
                    format!("▶ 第{k}倍音   対象 {:.2} Hz   範囲外（fs/2 = {:.0} Hz 以上）", a.harmonic_hz(k), a.nyquist())
                }
            };
            let color = if a.is_in_range(k) { harmonic_color(k) } else { Color32::GRAY };
            ui.label(RichText::new(txt).strong().color(color));
        }
        ui.add_space(4.0);
        let count = self.shown.in_range(a.num_in_range).count();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("時間変化に表示: {count} 本")).strong());
            if ui.small_button("すべて OFF").clicked() {
                self.shown.all_off();
            }
            if ui
                .small_button(format!("初期状態に戻す（1〜{DEFAULT_ON}）"))
                .on_hover_text(format!("第1〜第{DEFAULT_ON}倍音を ON、第{}倍音以降を OFF にする", DEFAULT_ON + 1))
                .clicked()
            {
                self.shown.reset();
            }
        });
        // 棒の数より先の倍音を範囲選択で ON にしていても、一覧から OFF にできるようにする
        let rows = n.max(self.shown.in_range(a.num_in_range).last().unwrap_or(0));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("readout").striped(true).num_columns(5).show(ui, |ui| {
                ui.label(RichText::new("表示").strong());
                ui.label(RichText::new("倍音").strong());
                ui.label(RichText::new("対象周波数").strong());
                ui.label(RichText::new("実測ピーク").strong());
                ui.label(RichText::new(format!("レベル @ {t:.3} s")).strong());
                ui.end_row();
                for k in 1..=rows {
                    let in_range = a.is_in_range(k);
                    let mut on = in_range && self.shown.contains(k);
                    let tip = if in_range { "時間変化のグラフに表示する" } else { "範囲外（fs/2 以上）の倍音は表示できない" };
                    if ui.add_enabled(in_range, egui::Checkbox::without_text(&mut on)).on_hover_text(tip).changed() {
                        self.shown.set(k, on);
                    }
                    let focused = self.focused == Some(k);
                    // OFF・範囲外の行は、名前も数値も半透明にして沈める
                    let color = match (in_range, on) {
                        (false, _) => Color32::GRAY.gamma_multiply(OFF_TEXT_ALPHA),
                        (true, true) => harmonic_color(k),
                        (true, false) => harmonic_color(k).gamma_multiply(OFF_TEXT_ALPHA),
                    };
                    let text_color = if on { ui.visuals().text_color() } else { ui.visuals().text_color().gamma_multiply(OFF_TEXT_ALPHA) };
                    let cell = |text: String| RichText::new(text).color(text_color);
                    let mut name = RichText::new(format!("{}■ 第{k}倍音", if focused { "▶ " } else { "" })).color(color);
                    if focused {
                        name = name.strong();
                    }
                    if ui.selectable_label(focused, name).on_hover_text("クリックで注目").clicked() {
                        self.focused = Some(k);
                    }
                    ui.label(cell(format!("{:.2} Hz", a.harmonic_hz(k))));
                    match a.level(frame, k) {
                        HarmonicLevel::Level { db, peak_hz } => {
                            ui.label(cell(format!("{peak_hz:.2} Hz")));
                            ui.label(cell(fmt_db(db)));
                        }
                        HarmonicLevel::OutOfRange => {
                            ui.label(cell("範囲外".into()));
                            ui.label("");
                        }
                    }
                    ui.end_row();
                }
            });
        });
    }

    fn time_views(&mut self, ui: &mut egui::Ui) {
        let Some(clip) = &self.clip else { return };
        let dur = clip.duration();
        let link = egui::Id::unique(("time-link", self.generation));
        let playhead = self.player.is_playing().then(|| self.player.position());
        let cursor = self.cursor;
        let floor = self.display_floor as f64;
        let total_h = ui.available_height();
        let mut clicked: Option<f64> = None;
        // つかめる縦線（ライブ中は解析位置が最新に固定なので無し）
        let live = self.live.is_some();
        let grab_lines: Vec<f64> = if live { Vec::new() } else { std::iter::once(cursor).chain(playhead).collect() };
        let seek_dragging = self.seek_drag;
        let seek_hold = self.seek_hover || seek_dragging;
        let mut seek = SeekGrab::default();
        let mut merge = |g: SeekGrab| {
            seek.hover |= g.hover;
            seek.moved = seek.moved.or(g.moved);
        };

        let time_plot = |name: &str, h: f32| {
            Plot::new((name.to_string(), self.generation))
                .height(h)
                .link_axis(link, [true, false])
                .link_cursor(link, [true, false])
                .default_x_bounds(0.0, dur)
                .allow_zoom([true, false])
                .allow_drag([!seek_hold, false])
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
        section_title(ui, "波形", |ui| {
            ui.label(RichText::new("解析: 元の音").small().color(Color32::BLACK).background_color(CURSOR_COLOR))
                .on_hover_text("グラフと倍音分布は常に元の音。試聴フィルタは再生音だけに掛かる");
            let text = if live {
                "ライブ入力: 右端が現在。解析位置は常に最新（止めると自由に調べられる）"
            } else {
                "クリックで解析位置、縦線はドラッグでシーク"
            };
            ui.add(egui::Label::new(hint(ui, text)).truncate()).on_hover_text("操作の一覧は「？ 操作方法」");
        });
        let mono = &clip.mono;
        let sr = clip.sample_rate as f64;
        let r = time_plot("wave", total_h * 0.18)
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
                (click_of(pu), seek_grab(pu, &grab_lines, seek_dragging))
            });
        clicked = clicked.or(r.inner.0);
        merge(r.inner.1);

        let Some(a) = &self.analysis else {
            self.finish_seek(ui, seek, dur);
            return;
        };

        // スペクトログラム
        let f0 = a.f0;
        let nyq = a.nyquist();
        section_title(ui, "スペクトログラム", |ui| {
            ui.add(egui::Label::new(hint(ui, "Shift+ドラッグで試聴帯域、帯の線はドラッグで調整")).truncate())
                .on_hover_text("縦軸 Hz、色 = レベル。ほかの所のドラッグ／ホイールで移動・拡大");
        });
        let fmax = (f0 * (self.bar_count.min(16) as f64 + 1.0)).clamp(1000.0, nyq); // 既定は第16倍音付近まで
        let spec = &a.spectrogram;
        let shift = ui.input(|i| i.modifiers.shift);
        let band = self.filter_on.then_some(self.band);
        let drag_start = self.band_drag_start;
        let edge_drag = self.edge_drag;
        // 境界線をつかんでいる間・つかめる位置にある間はグラフ自体を動かさない
        let hold = shift || self.edge_hover.is_some() || edge_drag.is_some() || seek_hold;
        let r = time_plot("spec", total_h * 0.33)
            .default_y_bounds(0.0, fmax)
            .allow_zoom([true, true])
            .allow_drag([!hold, !hold])
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
                        let on = self.shown.contains(k);
                        let c = if on { harmonic_color(k) } else { Color32::from_rgba_unmultiplied(255, 255, 255, 22) };
                        pu.hline(
                            HLine::new(format!("第{k}倍音"), k as f64 * f0)
                                .color(c)
                                .width(if on { 1.2 } else { 0.6 })
                                .style(LineStyle::dashed_loose()),
                        );
                    }
                }
                if let Some(b) = band {
                    let (fill, edge) = match b.mode {
                        BandMode::Keep => (Color32::from_rgba_unmultiplied(80, 220, 255, 35), KEEP_COLOR),
                        BandMode::Remove => (Color32::from_rgba_unmultiplied(255, 80, 80, 45), REMOVE_COLOR),
                    };
                    pu.span(
                        Span::new("試聴フィルタ", b.low_hz..=b.high_hz)
                            .axis(egui_plot::Axis::Y)
                            .fill(fill)
                            .border(Stroke::new(1.5, edge)),
                    );
                }
                draw_cursors(pu);
                let grab = seek_grab(pu, &grab_lines, seek_dragging);
                let resp = pu.response();
                let y = pu.pointer_coordinate().map(|p| p.y.max(0.0));
                // 縦線（解析位置）のほうを優先する
                let band = band.filter(|_| !grab.hover && !seek_dragging);
                // 境界線の近くにカーソルがあれば、その線をつかめる（近いほうを優先）
                let hover_edge = match (band, resp.hover_pos()) {
                    (Some(b), Some(p)) if !shift => {
                        let dist = |hz: f64| (pu.screen_from_plot(PlotPoint::new(0.0, hz)).y - p.y).abs();
                        let (dl, dh) = (dist(b.low_hz), dist(b.high_hz));
                        if dl.min(dh) > EDGE_GRAB_PX {
                            None
                        } else if dh <= dl {
                            Some(BandEdge::High)
                        } else {
                            Some(BandEdge::Low)
                        }
                    }
                    _ => None,
                };
                // 境界線のドラッグ: (どちらの線, 新しい周波数, 終了したか)
                let edge_move = if let Some(e) = edge_drag.or(hover_edge.filter(|_| resp.drag_started()))
                    && (resp.drag_started() || resp.dragged() || resp.drag_stopped())
                {
                    y.map(|y| (e, y, resp.drag_stopped()))
                } else {
                    None
                };
                // Shift+ドラッグで試聴フィルタの範囲を指定
                let band_drag = if edge_move.is_some() {
                    None
                } else if shift && resp.drag_started() {
                    y.map(|y| (y, y, false))
                } else if let (Some(s), Some(y)) = (drag_start, y)
                    && (resp.dragged() || resp.drag_stopped())
                {
                    Some((s, y, resp.drag_stopped()))
                } else {
                    None
                };
                let click = if edge_move.is_some() || hover_edge.is_some() { None } else { click_of(pu) };
                (click, band_drag, hover_edge, edge_move, grab)
            });
        let (spec_click, band_drag, hover_edge, edge_move, grab) = r.inner;
        clicked = clicked.or(spec_click);
        merge(grab);
        self.edge_hover = hover_edge;
        if hover_edge.is_some() || self.edge_drag.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if let Some((e, y, done)) = edge_move {
            self.edge_drag = (!done).then_some(e);
            // 上下の線は入れ替わらない（1 Hz 以上あける）
            match e {
                BandEdge::Low => self.band.low_hz = y.min(self.band.high_hz - 1.0).max(0.0),
                BandEdge::High => self.band.high_hz = y.max(self.band.low_hz + 1.0).min(nyq),
            }
        } else if self.edge_drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
            self.edge_drag = None;
        }
        if let Some((s, y, done)) = band_drag {
            self.band_drag_start = (!done).then_some(s);
            let (lo, hi) = (s.min(y), s.max(y).min(nyq));
            if hi - lo > 1.0 {
                self.band.low_hz = lo;
                self.band.high_hz = hi;
                self.filter_on = true;
            }
        }

        // 表示中の倍音の時間変化
        let on: Vec<usize> = self.shown.in_range(a.num_in_range).collect();
        let mut focus_click = None;
        section_title(ui, "倍音の時間変化", |ui| {
            ui.add(egui::Label::new(hint(ui, "共通の dB 目盛り。● は解析位置の値、薄い線は表示 OFF の倍音")).truncate());
        });
        // 凡例はグラフの外に置く（グラフを覆わない）。多いときは折り返し、さらに多ければスクロール
        if on.is_empty() {
            ui.label(hint(ui, "表示中の倍音はありません。右の一覧のチェックで ON にしてください"));
        } else {
            egui::ScrollArea::vertical().id_salt("legend").max_height(46.0).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for &k in &on {
                        let focused = self.focused == Some(k);
                        let mut text = RichText::new(format!("■ 第{k} {:.0} Hz", a.harmonic_hz(k))).color(harmonic_color(k));
                        if focused {
                            text = text.strong();
                        }
                        if ui.selectable_label(focused, text).on_hover_text("クリックで注目").clicked() {
                            focus_click = Some(k);
                        }
                    }
                });
            });
        }
        let frame = a.frame_at(cursor);
        let focused = self.focused;
        // OFF の倍音（棒が見えている範囲）は、ON の線の後ろに半透明で薄く描く
        let off: Vec<usize> = (1..=self.bar_count.min(a.num_in_range)).filter(|&k| !self.shown.contains(k)).collect();
        let r = time_plot("tracks", ui.available_height().max(120.0))
            .default_y_bounds(floor, 3.0)
            .x_axis_label("時間 (秒)")
            .y_axis_label("レベル (dB)")
            .label_formatter(|p| {
                let pos = hover_pos(p);
                Some(format!("{:.3} s\n{:.1} dB", pos.x, pos.y))
            })
            .show(ui, |pu| {
                for &k in &off {
                    if let Some(pts) = a.track(k) {
                        let name = format!("第{k}倍音 {:.0} Hz（OFF）", a.harmonic_hz(k));
                        pu.line(Line::new(name, pts).color(harmonic_color(k).gamma_multiply(OFF_LINE_ALPHA)).width(1.0));
                    }
                }
                for &k in &on {
                    if let Some(pts) = a.track(k) {
                        let name = format!("第{k}倍音 {:.0} Hz", a.harmonic_hz(k));
                        let width = if focused == Some(k) { 3.0 } else { 1.5 };
                        pu.line(Line::new(name.clone(), pts).color(harmonic_color(k)).width(width));
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
                (click_of(pu), seek_grab(pu, &grab_lines, seek_dragging))
            });
        clicked = clicked.or(r.inner.0);
        merge(r.inner.1);
        if let Some(k) = focus_click {
            self.focused = Some(k);
        }

        if let Some(t) = clicked {
            self.seek_by_hand(t.clamp(0.0, dur));
        }
        self.finish_seek(ui, seek, dur);
    }

    /// 縦線のドラッグを反映する。ドラッグ中は解析位置だけを動かし、離したときに再生位置も合わせる。
    fn finish_seek(&mut self, ui: &egui::Ui, seek: SeekGrab, dur: f64) {
        self.seek_hover = seek.hover;
        if seek.hover || self.seek_drag {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if let Some((t, done)) = seek.moved {
            let t = t.clamp(0.0, dur);
            self.seek_drag = !done;
            self.follow = false;
            if done {
                self.set_cursor(t);
            } else {
                self.cursor = match &self.analysis {
                    Some(a) => a.frame_time(a.frame_at(t)),
                    None => t,
                };
            }
        } else if self.seek_drag && !ui.input(|i| i.pointer.any_down()) {
            self.seek_drag = false;
        }
    }
}
