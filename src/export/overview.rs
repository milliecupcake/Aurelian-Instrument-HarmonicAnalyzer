//! overview.png（波形・スペクトログラム・表示 ON の倍音の時間変化）。
//!
//! 画面のウィンドウ幅とは無関係に、固定の大きさでスナップショットから描く。
//! 文字は英数字（egui 同梱の Hack）で描き、音源名などの日本語は OS の日本語フォントがあれば使う。

use super::*;
use crate::analysis::FLOOR_DB;
use crate::colors::{CURSOR_RGB, colormap, harmonic_rgb};
use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect, Stroke, StrokeDash, Transform};

const W: u32 = 1600;
const X0: f32 = 100.0;
const X1: f32 = 1440.0;
const BG: [u8; 3] = [18, 20, 24];
const PANEL_BG: [u8; 3] = [10, 11, 14];
const GRID: [u8; 4] = [255, 255, 255, 28];
const TEXT: [u8; 3] = [225, 228, 235];
const DIM: [u8; 3] = [150, 155, 165];
const WAVE: [u8; 3] = [110, 170, 255];
const LEGEND_COLS: usize = 6;

/// overview.png のスペクトログラムの周波数範囲（Hz）。第16倍音か、表示 ON の最も高い倍音の少し上まで
pub(super) fn freq_range(snap: &Snapshot) -> (f64, f64) {
    let a = &snap.analysis;
    let top = snap.view.shown.iter().copied().max().unwrap_or(0).max(16);
    (0.0, (a.f0 * (top as f64 + 1.0)).max(1000.0).min(a.nyquist()))
}

struct Canvas<'f> {
    px: Pixmap,
    fonts: Vec<FontRef<'f>>,
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

fn paint(rgba: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    p.anti_alias = true;
    p
}

fn rgb(c: [u8; 3]) -> [u8; 4] {
    [c[0], c[1], c[2], 255]
}

impl<'f> Canvas<'f> {
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
        if let Some(r) = Rect::from_xywh(x, y, w.max(0.5), h.max(0.5)) {
            self.px.fill_rect(r, &paint(c), Transform::identity(), None);
        }
    }

    fn polyline(&mut self, pts: &[(f32, f32)], c: [u8; 4], width: f32, dash: Option<(f32, f32)>) {
        if pts.len() < 2 {
            return;
        }
        let mut pb = PathBuilder::new();
        pb.move_to(pts[0].0, pts[0].1);
        for &(x, y) in &pts[1..] {
            pb.line_to(x, y);
        }
        let Some(path) = pb.finish() else { return };
        let mut stroke = Stroke { width, ..Default::default() };
        if let Some((on, off)) = dash {
            stroke.dash = StrokeDash::new(vec![on, off], 0.0);
        }
        self.px.stroke_path(&path, &paint(c), &stroke, Transform::identity(), None);
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [u8; 4], width: f32) {
        self.polyline(&[a, b], c, width, None);
    }

    /// 文字ごとに、グリフのある最初のフォントを使う
    fn glyphs(&self, text: &str, size: f32) -> Vec<(usize, ab_glyph::GlyphId, f32)> {
        let scale = PxScale::from(size);
        text.chars()
            .map(|ch| {
                let i = self.fonts.iter().position(|f| f.glyph_id(ch).0 != 0).unwrap_or(0);
                let f = self.fonts[i].as_scaled(scale);
                let id = f.glyph_id(ch);
                (i, id, f.h_advance(id))
            })
            .collect()
    }

    fn text_width(&self, text: &str, size: f32) -> f32 {
        self.glyphs(text, size).iter().map(|g| g.2).sum()
    }

    fn text(&mut self, x: f32, baseline: f32, size: f32, color: [u8; 3], text: &str, align: Align) {
        let width = self.text_width(text, size);
        let mut pen = match align {
            Align::Left => x,
            Align::Center => x - width / 2.0,
            Align::Right => x - width,
        };
        let (w, h) = (self.px.width() as i32, self.px.height() as i32);
        for (i, id, adv) in self.glyphs(text, size) {
            let glyph = id.with_scale_and_position(PxScale::from(size), point(pen, baseline));
            if let Some(og) = self.fonts[i].outline_glyph(glyph) {
                let b = og.px_bounds();
                let pixels = self.px.pixels_mut();
                og.draw(|gx, gy, cov| {
                    let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px < 0 || py < 0 || px >= w || py >= h {
                        return;
                    }
                    let p = &mut pixels[(py * w + px) as usize];
                    let a = cov.clamp(0.0, 1.0);
                    let mix = |src: u8, dst: u8| (src as f32 * a + dst as f32 * (1.0 - a)).round() as u8;
                    if let Some(c) = PremultipliedColorU8::from_rgba(
                        mix(color[0], p.red()),
                        mix(color[1], p.green()),
                        mix(color[2], p.blue()),
                        255,
                    ) {
                        *p = c;
                    }
                });
            }
            pen += adv;
        }
    }
}

/// lo〜hi に 1・2・5 × 10^n 刻みの目盛りを count 個前後
fn ticks(lo: f64, hi: f64, count: usize) -> Vec<f64> {
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return vec![lo];
    }
    let raw = (hi - lo) / count.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag);
    let mut t = (lo / step).ceil() * step;
    let mut out = Vec::new();
    while t <= hi + step * 1e-9 {
        out.push(if t.abs() < step * 1e-9 { 0.0 } else { t });
        t += step;
    }
    out
}

fn fmt_tick(v: f64, step: f64) -> String {
    let decimals = if step >= 1.0 { 0 } else { (-step.log10().floor()) as usize };
    format!("{v:.decimals$}")
}

/// 1 つのグラフの枠（横軸は時間で共通）
struct Panel {
    y0: f32,
    y1: f32,
    lo: f64,
    hi: f64,
}

impl Panel {
    fn y(&self, v: f64) -> f32 {
        let f = ((v - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0);
        self.y1 - f as f32 * (self.y1 - self.y0)
    }
}

pub fn render_overview(snap: &Snapshot, feats: &features::Features, cjk_font: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let a = &snap.analysis;
    let c = &snap.clip;
    let dur = c.duration().max(1e-9);
    let shown = &snap.view.shown;
    let legend_rows = shown.len().div_ceil(LEGEND_COLS).max(1);
    let legend_y = 1196.0;
    let footer_y = legend_y + legend_rows as f32 * 24.0 + 18.0;
    let height = (footer_y + 4.0 * 22.0 + 16.0) as u32;

    let mut fonts = vec![FontRef::try_from_slice(epaint_default_fonts::HACK_REGULAR).map_err(|e| e.to_string())?];
    if let Some(Ok(f)) = cjk_font.map(|b| FontRef::try_from_slice_and_index(b, 0)) {
        fonts.push(f);
    }
    let mut cv = Canvas { px: Pixmap::new(W, height).ok_or("画像を作れません")?, fonts };
    cv.px.fill(Color::from_rgba8(BG[0], BG[1], BG[2], 255));

    let tx = |t: f64| X0 + ((t / dur).clamp(0.0, 1.0) as f32) * (X1 - X0);
    let floor = (snap.view.display_floor_db as f64).min(-1.0);
    let cursor_t = snap.cursor_frame().map(|f| a.frame_time(f));

    // 見出し
    cv.text(X0, 34.0, 22.0, TEXT, "Harmonic Analyzer - overview (unfiltered source audio)", Align::Left);
    let range = match snap.excerpt {
        None => "whole audio".to_string(),
        Some(x) => format!(
            "excerpt {:.3}-{:.3} s of the source (t = 0 here = {:.3} s in the source)",
            snap.offset_s(),
            x.end_sample as f64 / a.sample_rate as f64,
            snap.offset_s()
        ),
    };
    cv.text(X0, 62.0, 16.0, TEXT, &format!("source: {}  |  {range}", snap.source_name), Align::Left);
    cv.text(
        X0,
        86.0,
        15.0,
        DIM,
        &format!(
            "f0 = {:.3} Hz (fixed) | fs = {} Hz, {} ch -> mono mix | {:.3} s | window {} smp ({:.1} ms), hop {}, FFT {}",
            a.f0,
            c.sample_rate,
            c.channels,
            c.duration(),
            a.window,
            a.window as f64 / a.sample_rate as f64 * 1000.0,
            a.hop,
            a.fft_size
        ),
        Align::Left,
    );

    // 時間軸の目盛り（3 つのグラフで共通）
    let tticks = ticks(0.0, dur, 10);
    let tstep = if tticks.len() > 1 { tticks[1] - tticks[0] } else { dur };
    let time_axis = |cv: &mut Canvas, p: &Panel| {
        for &t in &tticks {
            let x = tx(t);
            cv.line((x, p.y0), (x, p.y1), GRID, 1.0);
            cv.text(x, p.y1 + 18.0, 13.0, DIM, &fmt_tick(t, tstep), Align::Center);
        }
    };
    let y_axis = |cv: &mut Canvas, p: &Panel, count: usize, unit: &str| {
        let ts = ticks(p.lo, p.hi, count);
        let step = if ts.len() > 1 { ts[1] - ts[0] } else { 1.0 };
        for &v in &ts {
            let y = p.y(v);
            cv.line((X0, y), (X1, y), GRID, 1.0);
            cv.text(X0 - 8.0, y + 5.0, 13.0, DIM, &format!("{}{unit}", fmt_tick(v, step)), Align::Right);
        }
    };
    let cursor_line = |cv: &mut Canvas, p: &Panel| {
        let Some(t) = cursor_t else { return };
        let x = tx(t);
        cv.line((x, p.y0), (x, p.y1), rgb(CURSOR_RGB), 1.5);
    };

    // 波形（解析に使ったモノラル）
    cv.text(X0, 128.0, 17.0, TEXT, "Waveform (amplitude; mono mix used for analysis)", Align::Left);
    let wp = Panel { y0: 140.0, y1: 330.0, lo: -1.0, hi: 1.0 };
    cv.rect(X0, wp.y0, X1 - X0, wp.y1 - wp.y0, rgb(PANEL_BG));
    y_axis(&mut cv, &wp, 4, "");
    time_axis(&mut cv, &wp);
    let cols = (X1 - X0) as usize;
    let mono = &c.mono;
    for col in 0..cols {
        let i0 = col * mono.len() / cols;
        let i1 = ((col + 1) * mono.len() / cols).max(i0 + 1).min(mono.len());
        if i0 >= i1 {
            continue;
        }
        let seg = &mono[i0..i1];
        let lo = seg.iter().cloned().fold(f32::MAX, f32::min) as f64;
        let hi = seg.iter().cloned().fold(f32::MIN, f32::max) as f64;
        let x = X0 + col as f32 + 0.5;
        let (ya, yb) = (wp.y(hi), wp.y(lo));
        cv.rect(x - 0.5, ya, 1.0, (yb - ya).max(1.0), rgb(WAVE));
    }
    cursor_line(&mut cv, &wp);

    // スペクトログラム（最大値縮約済みのセル）
    let (f_lo, f_hi) = freq_range(snap);
    cv.text(
        X0,
        380.0,
        17.0,
        TEXT,
        &format!(
            "Spectrogram (Hz; color = level in dB; each cell = max over {:.1} ms x {:.2} Hz)",
            a.spectrogram.col_dt * 1000.0,
            a.spectrogram.row_hz
        ),
        Align::Left,
    );
    let sp = Panel { y0: 392.0, y1: 752.0, lo: f_lo, hi: f_hi };
    {
        let s = &a.spectrogram;
        let (pw, ph) = ((X1 - X0) as usize, (sp.y1 - sp.y0) as usize);
        let width = cv.px.width() as usize;
        let pixels = cv.px.pixels_mut();
        for yy in 0..ph {
            let hz = f_hi - (yy as f64 + 0.5) / ph as f64 * (f_hi - f_lo);
            let row = ((hz / s.row_hz).floor() as usize).min(s.rows - 1);
            for xx in 0..pw {
                let t = (xx as f64 + 0.5) / pw as f64 * dur;
                let col = ((t / s.col_dt).round() as usize).min(s.cols - 1);
                let v = s.db[row * s.cols + col] as f64;
                let rgbv = colormap(((v - floor) / (0.0 - floor)) as f32);
                let idx = (sp.y0 as usize + yy) * width + X0 as usize + xx;
                if let Some(p) = PremultipliedColorU8::from_rgba(rgbv[0], rgbv[1], rgbv[2], 255) {
                    pixels[idx] = p;
                }
            }
        }
    }
    // 表示 ON の倍音の位置（点線）
    for &k in shown {
        let hz = a.harmonic_hz(k);
        if hz <= f_hi {
            let y = sp.y(hz);
            let col = harmonic_rgb(k);
            cv.polyline(&[(X0, y), (X1, y)], [col[0], col[1], col[2], 150], 1.0, Some((6.0, 5.0)));
        }
    }
    y_axis(&mut cv, &sp, 8, "");
    time_axis(&mut cv, &sp);
    cursor_line(&mut cv, &sp);
    cv.text(X0 - 8.0, sp.y0 - 4.0, 13.0, DIM, "Hz", Align::Right);
    // カラーバー
    let (bx, bw) = (X1 + 24.0, 18.0);
    for yy in 0..(sp.y1 - sp.y0) as usize {
        let f = 1.0 - yy as f32 / (sp.y1 - sp.y0);
        let col = colormap(f);
        cv.rect(bx, sp.y0 + yy as f32, bw, 1.0, rgb(col));
    }
    cv.text(bx + bw + 6.0, sp.y0 + 10.0, 13.0, DIM, "0 dB", Align::Left);
    cv.text(bx + bw + 6.0, sp.y1, 13.0, DIM, &format!("{floor:.0} dB"), Align::Left);
    cv.text(bx + bw + 6.0, (sp.y0 + sp.y1) / 2.0, 13.0, DIM, &format!("{:.0}", floor / 2.0), Align::Left);

    // 倍音の時間変化（表示 ON のものだけ）
    cv.text(
        X0,
        802.0,
        17.0,
        TEXT,
        "Harmonic levels over time (dB; ON harmonics only - all harmonics are in harmonics.csv)",
        Align::Left,
    );
    let tp = Panel { y0: 814.0, y1: 1124.0, lo: floor, hi: 3.0 };
    cv.rect(X0, tp.y0, X1 - X0, tp.y1 - tp.y0, rgb(PANEL_BG));
    // 窓が音声の端をまたぐ区間を薄く塗る
    if let Some((first, last)) = summary::stats_frames(a) {
        let (xa, xb) = (tx(a.frame_time(first)), tx(a.frame_time(last)));
        cv.rect(X0, tp.y0, xa - X0, tp.y1 - tp.y0, [255, 255, 255, 18]);
        cv.rect(xb, tp.y0, X1 - xb, tp.y1 - tp.y0, [255, 255, 255, 18]);
    } else {
        cv.rect(X0, tp.y0, X1 - X0, tp.y1 - tp.y0, [255, 255, 255, 18]);
    }
    y_axis(&mut cv, &tp, 8, "");
    time_axis(&mut cv, &tp);
    cv.text(X0 - 8.0, tp.y0 - 4.0, 13.0, DIM, "dB", Align::Right);
    let focused = snap.view.focused.filter(|k| shown.contains(k));
    for &k in shown.iter().filter(|&&k| Some(k) != focused).chain(focused.iter()) {
        let Some(track) = a.track(k) else { continue };
        let col = harmonic_rgb(k);
        let width = if Some(k) == focused { 3.0 } else { 1.6 };
        let pts: Vec<(f32, f32)> = if track.len() > cols * 2 {
            // 1 ピクセルの列ごとに最小と最大をつなぐ
            let mut v = Vec::with_capacity(cols * 2);
            for col_i in 0..cols {
                let f0 = col_i * track.len() / cols;
                let f1 = ((col_i + 1) * track.len() / cols).max(f0 + 1).min(track.len());
                let seg = &track[f0..f1];
                let lo = seg.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
                let hi = seg.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
                let x = X0 + col_i as f32 + 0.5;
                v.push((x, tp.y(lo)));
                v.push((x, tp.y(hi)));
            }
            v
        } else {
            track.iter().map(|p| (tx(p[0]), tp.y(p[1]))).collect()
        };
        cv.polyline(&pts, rgb(col), width, None);
    }
    if shown.is_empty() {
        cv.text(
            (X0 + X1) / 2.0,
            (tp.y0 + tp.y1) / 2.0,
            18.0,
            DIM,
            "No harmonics shown (all OFF in the app). Every harmonic is still in harmonics.csv.",
            Align::Center,
        );
    }
    // 持続区間（指定したときだけ）と発音開始
    if let Some((f0, f1)) = feats.sustain_frames {
        let (xa, xb) = (tx(a.frame_time(f0)), tx(a.frame_time(f1)));
        cv.rect(xa, tp.y0, (xb - xa).max(1.0), 6.0, [120, 220, 255, 200]);
        cv.text(xa, tp.y0 + 34.0, 13.0, [120, 220, 255], "sustain interval (user)", Align::Left);
    }
    if let Some(f) = feats.onset.frame() {
        let x = tx(a.frame_time(f));
        cv.polyline(&[(x, tp.y0), (x, tp.y1)], [255, 255, 255, 140], 1.2, Some((4.0, 4.0)));
        let label = if matches!(feats.onset, features::Onset::Manual { .. }) { "onset (user)" } else { "onset (estimated)" };
        cv.text(x + 4.0, tp.y1 - 8.0, 13.0, DIM, label, Align::Left);
    }
    cursor_line(&mut cv, &tp);
    match cursor_t {
        Some(t) => cv.text(tx(t) + 6.0, tp.y0 + 16.0, 13.0, CURSOR_RGB, &format!("analysis position {t:.3} s"), Align::Left),
        None => cv.text(X1, tp.y0 + 16.0, 13.0, CURSOR_RGB, "analysis position: outside this excerpt", Align::Right),
    }
    cv.text((X0 + X1) / 2.0, tp.y1 + 40.0, 14.0, DIM, "time (s)", Align::Center);

    // 凡例
    if shown.is_empty() {
        cv.text(X0, legend_y + 16.0, 14.0, DIM, "legend: (no harmonics shown)", Align::Left);
    }
    for (i, &k) in shown.iter().enumerate() {
        let x = X0 + (i % LEGEND_COLS) as f32 * 220.0;
        let y = legend_y + (i / LEGEND_COLS) as f32 * 24.0;
        let col = harmonic_rgb(k);
        cv.rect(x, y + 4.0, 22.0, 12.0, rgb(col));
        let mark = if Some(k) == focused { " (focused)" } else { "" };
        cv.text(x + 30.0, y + 15.0, 14.0, TEXT, &format!("H{k}  {:.1} Hz{mark}", a.harmonic_hz(k)), Align::Left);
    }

    // 注意書き
    let listen = if !snap.listen.enabled {
        "OFF".to_string()
    } else {
        match snap.listen.kind {
            ListenKind::Band => format!(
                "{} {:.1}-{:.1} Hz",
                match snap.listen.band.mode {
                    crate::filter::BandMode::Keep => "keep",
                    crate::filter::BandMode::Remove => "remove",
                },
                snap.listen.band.low_hz,
                snap.listen.band.high_hz
            ),
            ListenKind::Harmonics => format!("keep ON harmonics +/- {:.0}% of f0", snap.listen.harm_width_pct),
        }
    };
    let notes = [
        format!("Listening filter: {listen} (playback only - NOT applied to this image, harmonics.csv or source.wav)"),
        format!(
            "Levels: sine of amplitude 1.0 = 0 dB; values at or below {FLOOR_DB:.0} dB are clamped (true level unknown). Plot floor: {floor:.0} dB."
        ),
        "Shaded areas in the level plot: frames whose window overlaps zero padding at the start/end (edge_padded).".to_string(),
        format!("Analysis range: harmonics 1-{} (below Nyquist {:.0} Hz). Dashed lines in the spectrogram: ON harmonics.", a.num_in_range, a.nyquist()),
    ];
    for (i, line) in notes.iter().enumerate() {
        cv.text(X0, footer_y + i as f32 * 22.0, 14.0, DIM, line, Align::Left);
    }

    cv.px.encode_png().map_err(|e| format!("PNG を作れません: {e}"))
}
