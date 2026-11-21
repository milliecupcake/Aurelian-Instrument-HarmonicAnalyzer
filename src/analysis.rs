//! STFT による倍音解析。
//!
//! - 各フレームで 4項 Blackman-Harris 窓（サイドローブ -92 dB）をかけ、2倍のゼロ詰めをして FFT する。
//! - 第k倍音（k·f0）の周辺 ±10%·f0 で振幅ピークを探し、対数振幅の放物線補間で
//!   ビンの間に来る周波数の振幅を推定する。範囲内に極大が無いときは k·f0 に最も近いビンの値を使う。
//! - 振幅は「振幅 1.0 の正弦波 = 0 dB」となるよう正規化し、録音全体・全倍音で共通の基準とする。
//! - k·f0 がナイキスト周波数（fs/2）以上の倍音は「範囲外」とし、値を持たない。

use realfft::RealFftPlanner;

/// 表示・計算上のレベル下限（dB）。無音時はこの値になる。
pub const FLOOR_DB: f32 = -120.0;
const FLOOR_AMP: f64 = 1.0e-6; // = -120 dB
/// 解析する倍音の最大数。
pub const MAX_HARMONICS: usize = 512;
/// 倍音ピークを探す範囲（基音周波数に対する比率、片側）。
pub const SEARCH_TOLERANCE: f64 = 0.10;
/// スペクトログラム画像の最大サイズ。
const MAX_SPEC_ROWS: usize = 2048;
const MAX_SPEC_COLS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSize {
    /// 基音の8周期以上を含む2のべき乗。
    Auto,
    Fixed(usize),
}

/// 基音の8周期以上を含む窓長（2のべき乗）。Blackman-Harris 窓のメインローブ（±4ビン）が
/// 隣の倍音に重ならないようにする。
pub fn auto_window(sample_rate: u32, f0: f64) -> usize {
    let n = (8.0 * sample_rate as f64 / f0).ceil().max(1.0) as usize;
    n.next_power_of_two().clamp(512, 32768)
}

pub fn hop_for(window: usize) -> usize {
    (window / 8).clamp(64, 512)
}

pub fn amp_to_db(a: f64) -> f32 {
    (20.0 * a.max(FLOOR_AMP).log10()) as f32
}

/// k·f0 < fs/2 を満たす最大の k。
pub fn harmonics_in_range(sample_rate: u32, f0: f64) -> usize {
    let nyq = sample_rate as f64 / 2.0;
    if f0 <= 0.0 || f0 >= nyq {
        return 0;
    }
    let k = (nyq / f0).ceil() as usize - 1;
    k.min(MAX_HARMONICS)
}

#[derive(Clone, Debug)]
pub struct Spectrogram {
    pub cols: usize,
    pub rows: usize,
    /// 行優先。row 0 が最も低い周波数。`db[row * cols + col]`
    pub db: Vec<f32>,
    /// 1行あたりの周波数幅（Hz）
    pub row_hz: f64,
    /// 1列あたりの時間幅（秒）
    pub col_dt: f64,
}

impl Spectrogram {
    pub fn value_at(&self, t: f64, hz: f64) -> Option<f32> {
        if t < 0.0 || hz < 0.0 {
            return None;
        }
        let c = (t / self.col_dt).round() as usize;
        let r = (hz / self.row_hz).round() as usize;
        (c < self.cols && r < self.rows).then(|| self.db[r * self.cols + c])
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HarmonicLevel {
    /// 範囲内。値は dB（下限 FLOOR_DB でクランプ済み）。
    Level { db: f32, peak_hz: f32 },
    /// k·f0 がナイキスト周波数以上。
    OutOfRange,
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub sample_rate: u32,
    pub f0: f64,
    pub window: usize,
    pub hop: usize,
    pub fft_size: usize,
    pub num_frames: usize,
    pub num_in_range: usize,
    /// `levels[frame * num_in_range + (k - 1)]`
    levels: Vec<f32>,
    peak_hz: Vec<f32>,
    pub spectrogram: Spectrogram,
}

impl Analysis {
    pub fn nyquist(&self) -> f64 {
        self.sample_rate as f64 / 2.0
    }

    pub fn frame_time(&self, frame: usize) -> f64 {
        frame as f64 * self.hop as f64 / self.sample_rate as f64
    }

    /// 時刻 t に最も近いフレーム（窓の中心が t に最も近いもの）。
    pub fn frame_at(&self, t: f64) -> usize {
        let f = (t * self.sample_rate as f64 / self.hop as f64).round();
        (f.max(0.0) as usize).min(self.num_frames.saturating_sub(1))
    }

    pub fn harmonic_hz(&self, k: usize) -> f64 {
        k as f64 * self.f0
    }

    pub fn is_in_range(&self, k: usize) -> bool {
        k >= 1 && k <= self.num_in_range
    }

    pub fn level(&self, frame: usize, k: usize) -> HarmonicLevel {
        if !self.is_in_range(k) || frame >= self.num_frames {
            return HarmonicLevel::OutOfRange;
        }
        let i = frame * self.num_in_range + (k - 1);
        HarmonicLevel::Level { db: self.levels[i], peak_hz: self.peak_hz[i] }
    }

    pub fn level_db(&self, frame: usize, k: usize) -> Option<f32> {
        match self.level(frame, k) {
            HarmonicLevel::Level { db, .. } => Some(db),
            HarmonicLevel::OutOfRange => None,
        }
    }

    /// 第k倍音の時間変化 `[(秒, dB)]`。範囲外なら None。
    pub fn track(&self, k: usize) -> Option<Vec<[f64; 2]>> {
        if !self.is_in_range(k) {
            return None;
        }
        Some(
            (0..self.num_frames)
                .map(|f| [self.frame_time(f), self.levels[f * self.num_in_range + k - 1] as f64])
                .collect(),
        )
    }
}

/// モノラル信号を解析する。f0 は (0, fs/2) の範囲であること。
pub fn analyze(samples: &[f32], sample_rate: u32, f0: f64, window: WindowSize) -> Result<Analysis, String> {
    let fs = sample_rate as f64;
    if !(f0.is_finite() && f0 > 0.0 && f0 < fs / 2.0) {
        return Err(format!("基音周波数は 0 より大きく {:.1} Hz（fs/2）未満にしてください", fs / 2.0));
    }
    let n = match window {
        WindowSize::Auto => auto_window(sample_rate, f0),
        WindowSize::Fixed(n) => n.max(64),
    };
    let hop = hop_for(n);
    let m = n * 2; // ゼロ詰め
    let bins = m / 2 + 1;
    let bin_hz = fs / m as f64;

    // 4項 Blackman-Harris 窓（周期形）
    let win: Vec<f32> = (0..n)
        .map(|i| {
            let x = std::f64::consts::TAU * i as f64 / n as f64;
            (0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos()) as f32
        })
        .collect();
    let win_sum: f64 = win.iter().map(|&w| w as f64).sum();
    let scale = 2.0 / win_sum; // 正弦波の振幅 A → A

    let num_frames = samples.len() / hop + 1;
    let num_in_range = harmonics_in_range(sample_rate, f0);

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(m);
    let mut input = fft.make_input_vec();
    let mut output = fft.make_output_vec();
    let mut mag = vec![0f64; bins];

    let mut levels = Vec::with_capacity(num_frames * num_in_range);
    let mut peaks = Vec::with_capacity(num_frames * num_in_range);

    // スペクトログラムの縮約
    let row_pool = bins.div_ceil(MAX_SPEC_ROWS);
    let rows = bins.div_ceil(row_pool);
    let col_pool = num_frames.div_ceil(MAX_SPEC_COLS).max(1);
    let cols = num_frames.div_ceil(col_pool);
    let mut spec = vec![FLOOR_DB; rows * cols];

    let tol_bins = (SEARCH_TOLERANCE * f0 / bin_hz).max(1.0);

    for frame in 0..num_frames {
        let center = (frame * hop) as isize;
        let start = center - (n / 2) as isize;
        input.iter_mut().for_each(|v| *v = 0.0);
        for i in 0..n {
            let s = start + i as isize;
            if s >= 0 && (s as usize) < samples.len() {
                input[i] = samples[s as usize] * win[i];
            }
        }
        fft.process(&mut input, &mut output).map_err(|e| e.to_string())?;
        for (b, c) in output.iter().enumerate() {
            mag[b] = (c.re as f64).hypot(c.im as f64) * scale;
        }

        // 倍音
        for k in 1..=num_in_range {
            let target = k as f64 * f0 / bin_hz;
            let (amp, hz) = estimate_peak(&mag, target, tol_bins, bin_hz);
            levels.push(amp_to_db(amp));
            peaks.push(hz as f32);
        }

        // スペクトログラム（各セルは最大値で縮約）
        let col = frame / col_pool;
        for r in 0..rows {
            let lo = r * row_pool;
            let hi = (lo + row_pool).min(bins);
            let a = mag[lo..hi].iter().cloned().fold(0.0, f64::max);
            let cell = &mut spec[r * cols + col];
            *cell = cell.max(amp_to_db(a));
        }
    }

    Ok(Analysis {
        sample_rate,
        f0,
        window: n,
        hop,
        fft_size: m,
        num_frames,
        num_in_range,
        levels,
        peak_hz: peaks,
        spectrogram: Spectrogram {
            cols,
            rows,
            db: spec,
            row_hz: row_pool as f64 * bin_hz,
            col_dt: col_pool as f64 * hop as f64 / fs,
        },
    })
}

/// target（ビン単位、小数可）の ±tol ビンの範囲でピークを探し、振幅と周波数を返す。
fn estimate_peak(mag: &[f64], target: f64, tol: f64, bin_hz: f64) -> (f64, f64) {
    let last = mag.len() - 1;
    let lo = ((target - tol).floor().max(1.0)) as usize;
    let hi = ((target + tol).ceil() as usize).min(last - 1);
    let nearest = (target.round() as usize).clamp(1, last - 1);
    if lo > hi {
        return (mag[nearest], nearest as f64 * bin_hz);
    }
    let mut best = lo;
    for b in lo..=hi {
        if mag[b] > mag[best] {
            best = b;
        }
    }
    let is_local_max = mag[best] >= mag[best - 1] && mag[best] >= mag[best + 1] && mag[best] > FLOOR_AMP;
    if !is_local_max {
        // 範囲内に極大が無い（隣の成分の裾など）→ 目標周波数のビンの値
        return (mag[nearest], target * bin_hz);
    }
    // 対数振幅の放物線補間
    // 下限より十分小さい値で切り、補正量はスカロッピング損失の上限（約 0.5 dB）までに抑える
    let l = |x: f64| x.max(FLOOR_AMP * 1e-3).ln();
    let (a, b, c) = (l(mag[best - 1]), l(mag[best]), l(mag[best + 1]));
    let denom = a - 2.0 * b + c;
    let p = if denom < -1e-12 { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) } else { 0.0 };
    let peak_ln = (b - 0.25 * (a - c) * p).min(b + 0.06);
    (peak_ln.exp(), (best as f64 + p) * bin_hz)
}
