//! 「AI解析用に書き出す」の画面（AIEXP-06）。
//!
//! 書き出しを始めた時点の状態を `Snapshot` に固定し、別スレッドで ZIP を作る。
//! 書き出しは解析位置・倍音の ON／OFF・試聴フィルタを変えない（読むだけ）。

use super::*;

const FILE_ROLES: [(&str, &str); 7] = [
    ("source.wav", "試聴フィルタ適用前の音声（32-bit float、元のチャンネルのまま）"),
    ("harmonics.csv", "解析範囲内の全倍音 × 全フレームの数値（表示 OFF の倍音も含む。基音に対する相対レベルつき）"),
    ("features.csv", "倍音ごとの最大・持続区間の代表値・相対レベル・減衰時間"),
    ("envelope.csv", "フレームごとの RMS・ピーク・スペクトル重心"),
    ("manifest.json", "解析条件・音源情報・表示と試聴の状態・列の定義"),
    ("overview.png", "波形・スペクトログラム・表示 ON の倍音の時間変化"),
    ("summary.txt", "主要な数値・解釈の制約・下に入力した背景と質問"),
];

impl App {
    /// 音が変わっていたら、区間などの初期値を入れ直す
    pub(super) fn export_defaults(&mut self) {
        let Some(c) = &self.clip else { return };
        if self.export.defaults_for == Some(self.clip_rev) {
            return;
        }
        let dur = c.duration();
        let e = &mut self.export;
        e.defaults_for = Some(self.clip_rev);
        e.range = (0.0, dur);
        e.onset = 0.0;
        e.sustain = (self.cursor.min(dur), (self.cursor + 0.5).min(dur));
    }

    /// 区間の指定の誤り（書き出す前に検出する）
    fn export_range_error(&self) -> Option<String> {
        let c = self.clip.as_ref()?;
        let e = &self.export;
        if e.range_on
            && let Err(msg) = export::sample_range(e.range.0, e.range.1, c.sample_rate, c.mono.len())
        {
            return Some(format!("書き出す範囲: {msg}"));
        }
        if e.sustain_on {
            let (a, b) = e.sustain;
            if !(a.is_finite() && b.is_finite()) || a >= b {
                return Some("持続区間: 開始を終了より前にしてください".into());
            }
            if a < 0.0 || b > c.duration() + 1e-9 {
                return Some(format!("持続区間: 音源の範囲（0〜{:.3} 秒）の外です", c.duration()));
            }
        }
        None
    }

    /// 書き出せない理由（書き出せるなら None）
    pub(super) fn export_block_reason(&self) -> Option<&'static str> {
        if self.clip.is_none() {
            Some("音源を読み込むと書き出せます")
        } else if self.live.is_some() {
            Some("ライブ停止後に書き出せます")
        } else if self.analysis.is_none() || self.analysis_rev != self.clip_rev {
            Some("解析が終わっていません（基音の設定を確かめてください）")
        } else if self.export.job.is_some() {
            Some("書き出し中です")
        } else {
            None
        }
    }

    /// 画面に出している音源の名前
    fn export_source(&self) -> Option<(SourceKind, String)> {
        let file_name = |p: &PathBuf| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if let Some((name, ch)) = &self.live_capture {
            let label = match &self.path {
                Some(p) => format!("{}（ライブ入力の録音）", file_name(p)),
                None => format!("ライブ入力の録音（{name}）"),
            };
            return Some((
                SourceKind::LiveRecording { input_name: name.clone(), input_channels: *ch, saved_path: self.path.clone() },
                label,
            ));
        }
        if let Some(i) = self.sample {
            let s = &SAMPLES[i];
            return Some((
                SourceKind::Sample { title: s.title.into(), about: s.about.into() },
                format!("サンプル: {}", s.title),
            ));
        }
        let p = self.path.as_ref()?;
        Some((SourceKind::File { path: p.clone() }, file_name(p)))
    }

    /// いまの音声・解析結果・設定・表示状態を固定する
    pub(super) fn export_snapshot(&self) -> Result<Snapshot, String> {
        if let Some(r) = self.export_block_reason() {
            return Err(r.into());
        }
        let (Some(clip), Some(analysis)) = (&self.clip, &self.analysis) else {
            return Err("音源がありません".into());
        };
        let (source, source_name) = self.export_source().ok_or("音源の種類が分かりません")?;
        if let Some(e) = self.export_range_error() {
            return Err(e);
        }
        let e = &self.export;
        let range = if e.range_on {
            Some(export::sample_range(e.range.0, e.range.1, clip.sample_rate, clip.mono.len())?)
        } else {
            None
        };
        let f0 = analysis.f0;
        let matches = self.resolved_f0().is_ok_and(|v| (v - f0).abs() <= 1e-9 * f0);
        let estimate = match self.f0_estimate {
            Some((e, rev)) if rev == self.clip_rev => EstimateState::Current(e),
            Some(_) => EstimateState::Unverified,
            None => EstimateState::NotEstimated,
        };
        Ok(Snapshot {
            clip: clip.clone(),
            analysis: analysis.clone(),
            source,
            source_name,
            pitch: PitchInput { text: self.f0_text.clone(), a4_hz: self.a4_hz, cents: self.cents, matches_analysis: matches },
            window_mode: self.window,
            estimate,
            view: ViewState {
                shown: self.shown.in_range(analysis.num_in_range).collect(),
                focused: self.focused,
                cursor_s: self.cursor,
                display_floor_db: self.display_floor,
                time_range_s: self.view_time,
                freq_range_hz: self.view_freq,
                level_range_db: self.view_level,
            },
            listen: ListenFilter {
                enabled: self.filter_on,
                kind: match self.filter_kind {
                    FilterKind::Band => ListenKind::Band,
                    FilterKind::Harmonics => ListenKind::Harmonics,
                },
                band: self.band,
                harm_width_pct: self.harm_width_pct,
            },
            notes: self.export.notes.clone(),
            intervals: export::Intervals {
                onset_s: e.onset_manual.then_some(e.onset),
                sustain_s: e.sustain_on.then_some(e.sustain),
            },
            range,
            excerpt: None,
            created: std::time::SystemTime::now(),
        })
    }

    /// スナップショットを固定してから保存先を聞き、裏で書き出す
    fn start_export(&mut self) {
        let snap = match self.export_snapshot() {
            Ok(s) => s,
            Err(e) => {
                self.export.last = Some(Err(e));
                return;
            }
        };
        let mut dialog = rfd::FileDialog::new()
            .add_filter("ZIP", &["zip"])
            .set_file_name(export::default_zip_name(&snap.source_name));
        // 前回の保存先か、開いている WAV のフォルダから始める
        let start = self.export.dir.clone().or_else(|| self.path.as_ref().and_then(|p| p.parent().map(PathBuf::from)));
        if let Some(d) = start {
            dialog = dialog.set_directory(d);
        }
        let Some(mut dest) = dialog.save_file() else { return };
        if !dest.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
            dest.as_mut_os_string().push(".zip");
        }
        self.export.dir = dest.parent().map(PathBuf::from);
        self.run_export(snap, dest);
    }

    pub(super) fn run_export(&mut self, snap: Snapshot, dest: PathBuf) {
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let (p, d) = (progress.clone(), dest.clone());
        std::thread::spawn(move || {
            let font = harmonic_analyzer::fonts::load_cjk_font();
            let _ = tx.send(export::export_zip(&snap, &d, font.as_deref(), &p));
        });
        self.export.last = None;
        self.export.job = Some(ExportJob { progress, rx, dest });
    }

    /// 書き出しが終わっていれば結果を受け取る
    pub(super) fn poll_export(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.export.job else { return };
        match job.rx.try_recv() {
            Ok(res) => {
                self.export.last = Some(match res {
                    Ok(()) => Ok(job.dest.clone()),
                    Err(ExportError::Cancelled) => Err("キャンセルしました（保存先は変えていません）".into()),
                    Err(ExportError::Failed(e)) => Err(format!("書き出せませんでした: {e}")),
                });
                self.export.job = None;
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.export.last = Some(Err("書き出しが途中で止まりました".into()));
                self.export.job = None;
            }
        }
    }

    pub(super) fn export_window(&mut self, ctx: &egui::Context) {
        if !self.export.open {
            return;
        }
        let mut open = true;
        egui::Window::new("AI解析用に書き出す")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| self.export_contents(ui));
        // 書き出し中に閉じても、書き出しは続く（ツールバーに状態が出る）
        self.export.open = open;
    }

    fn export_contents(&mut self, ui: &mut egui::Ui) {
        ui.label(hint(ui, "音声・全倍音の数値・解析条件・概要画像・要約を、1 つの ZIP にまとめて保存します。"));
        ui.add_space(6.0);

        egui::Grid::new("export-target").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new("対象").strong());
            match (self.export_source(), &self.clip) {
                (Some((_, name)), Some(c)) => {
                    ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                    ui.end_row();
                    ui.label("");
                    ui.label(format!("{:.3} 秒（音声全体）  {} Hz / {} ch", c.duration(), c.sample_rate, c.channels));
                }
                _ => {
                    ui.label(hint(ui, "なし"));
                }
            }
            ui.end_row();
            if let Some(a) = &self.analysis {
                ui.label(RichText::new("解析").strong());
                ui.label(format!(
                    "基音 {:.2} Hz、第1〜第{}倍音 × {} フレーム（{} 行）",
                    a.f0,
                    a.num_in_range,
                    a.num_frames,
                    a.num_frames * a.num_in_range
                ));
                ui.end_row();
            }
        });

        ui.add_space(6.0);
        self.export_ranges(ui);

        ui.add_space(6.0);
        egui::CollapsingHeader::new("含まれるファイル").default_open(false).show(ui, |ui| {
            egui::Grid::new("export-files").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                for (name, role) in FILE_ROLES {
                    ui.monospace(name);
                    ui.label(hint(ui, role));
                    ui.end_row();
                }
            });
        });

        ui.add_space(6.0);
        ui.label(RichText::new("背景と質問（任意。summary.txt と manifest.json に入る）").strong());
        let busy = self.export.job.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            egui::Grid::new("export-notes").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label("音源・楽器");
                ui.add(
                    egui::TextEdit::singleline(&mut self.export.notes.instrument)
                        .desired_width(380.0)
                        .hint_text("例: FM エレピのプリセット、アコースティックギターの 5 弦開放"),
                );
                ui.end_row();
                ui.label("奏法・エフェクト");
                ui.add(
                    egui::TextEdit::singleline(&mut self.export.notes.technique)
                        .desired_width(380.0)
                        .hint_text("例: 強めのベロシティ、リバーブなし"),
                );
                ui.end_row();
                ui.label("調べたいこと");
                ui.add(
                    egui::TextEdit::multiline(&mut self.export.notes.question)
                        .desired_width(380.0)
                        .desired_rows(3)
                        .hint_text("例: FM シンセで近づけるには、どの倍音の減衰を合わせればよい？"),
                );
                ui.end_row();
            });
        });
        ui.label(hint(ui, "未入力の項目は「不明」として記録します。音源の種類や原因を自動で決めつけることはしません。"));

        ui.add_space(8.0);
        if let Some(job) = &self.export.job {
            let mut cancel = false;
            ui.horizontal(|ui| {
                ui.spinner();
                ui.add(
                    egui::ProgressBar::new(job.progress.fraction())
                        .desired_width(300.0)
                        .text(format!("書き出し中… {:.0}%", job.progress.fraction() * 100.0)),
                );
                cancel = skin::button(ui, None, "キャンセル", skin::Active::No).clicked();
            });
            if cancel {
                job.progress.cancel();
            }
        } else {
            let range_err = self.export_range_error();
            let reason = self.export_block_reason().map(String::from).or(range_err);
            ui.horizontal(|ui| {
                let r = ui
                    .add_enabled_ui(reason.is_none(), |ui| {
                        skin::button(ui, Some(skin::Icon::Export), "保存先を選んで書き出す…", skin::Active::No)
                    })
                    .inner;
                if r.clicked() {
                    self.start_export();
                }
                if let Some(reason) = reason {
                    ui.colored_label(Color32::LIGHT_RED, &reason);
                }
            });
        }
        match &self.export.last {
            Some(Ok(path)) => {
                let path = path.clone();
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("保存しました:").color(KEEP_COLOR));
                    ui.label(path.display().to_string());
                    if cfg!(target_os = "macos") && ui.small_button("Finder で表示").clicked() {
                        let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn();
                    }
                });
            }
            Some(Err(e)) => {
                ui.colored_label(Color32::LIGHT_RED, e);
                ui.label(hint(ui, "保存先を変えてもう一度書き出せます。"));
            }
            None => {}
        }
        ui.add_space(4.0);
        ui.label(hint(ui, "書き出しても、解析位置・倍音の ON／OFF・試聴フィルタは変わりません。"));
    }
}

impl App {
    /// 「書き出す範囲」と「特徴量の区間」の入力（時刻は元の音源の秒）
    fn export_ranges(&mut self, ui: &mut egui::Ui) {
        let Some(c) = &self.clip else { return };
        let (dur, sr) = (c.duration(), c.sample_rate as f64);
        let cursor = self.cursor;
        let busy = self.export.job.is_some();
        let e = &mut self.export;
        // 秒の入力欄と「解析位置」ボタン
        let time_input = |ui: &mut egui::Ui, v: &mut f64, label: &str| {
            ui.label(label);
            ui.add(egui::DragValue::new(v).range(0.0..=dur).speed(0.005).fixed_decimals(3).suffix(" 秒"));
            if ui.small_button("← 解析位置").on_hover_text(format!("解析位置（{cursor:.3} 秒）を入れる")).clicked() {
                *v = cursor;
            }
        };
        ui.add_enabled_ui(!busy, |ui| {
            egui::Grid::new("export-ranges").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("書き出す範囲").strong());
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut e.range_on, false, "音声全体");
                        ui.radio_value(&mut e.range_on, true, "時間区間");
                    });
                    if e.range_on {
                        ui.horizontal(|ui| {
                            time_input(ui, &mut e.range.0, "開始");
                            time_input(ui, &mut e.range.1, "終了");
                        });
                        if let Ok((s, x)) = export::sample_range(e.range.0, e.range.1, sr as u32, c.mono.len()) {
                            ui.label(hint(
                                ui,
                                format!("長さ {:.3} 秒（サンプル {s}〜{x}、終了は含まない）。切り出して同じ条件で解析し直す", (x - s) as f64 / sr),
                            ));
                        }
                    }
                });
                ui.end_row();

                ui.label(RichText::new("発音開始").strong());
                ui.horizontal(|ui| {
                    ui.radio_value(&mut e.onset_manual, false, "自動（RMS から推定）");
                    ui.radio_value(&mut e.onset_manual, true, "指定");
                    if e.onset_manual {
                        time_input(ui, &mut e.onset, "");
                    }
                });
                ui.end_row();

                ui.label(RichText::new("持続区間").strong());
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut e.sustain_on, "指定する");
                        if e.sustain_on {
                            time_input(ui, &mut e.sustain.0, "開始");
                            time_input(ui, &mut e.sustain.1, "終了");
                        }
                    });
                    if !e.sustain_on {
                        ui.label(hint(ui, "指定しないと、持続区間の代表レベルは出しません（推測で補いません）"));
                    }
                });
                ui.end_row();
            });
        });
    }
}
