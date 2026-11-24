//! Click prototype for the soundboard UI using egui/eframe with default styling.
//! All data is fake: there is no audio, no hotkeys, no downloads.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 640.0])
            .with_title("Soundboard – egui prototype"),
        ..Default::default()
    };
    eframe::run_native(
        "soundboard-egui-proto",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}

struct Sound {
    name: String,
    folder: usize,
    hotkey: Option<String>,
    midi: Option<String>,
    duration: f32,
    start: f32,
    end: f32,
    peaks: Vec<f32>,
}

struct Playing {
    sound: usize,
    started: Instant,
}

#[derive(PartialEq, Clone, Copy)]
enum HotkeyBackend {
    Portal,
    X11,
}

#[derive(PartialEq, Clone, Copy)]
enum Retrigger {
    Overlap,
    Restart,
    Stop,
}

struct EditState {
    sound: usize,
    name: String,
    start: f32,
    end: f32,
    hotkey: Option<String>,
    midi: Option<String>,
    recording_hotkey: bool,
    learning_midi: bool,
    preview_started: Option<Instant>,
}

struct UrlState {
    url: String,
    folder: usize,
    download_started: Option<Instant>,
}

struct App {
    folders: Vec<String>,
    sounds: Vec<Sound>,
    tab: usize,
    search: String,
    playing: Vec<Playing>,
    targets: Vec<&'static str>,
    target: usize,
    local_volume: f32,
    call_volume: f32,
    play_locally: bool,
    edit: Option<EditState>,
    url: Option<UrlState>,
    settings_open: bool,
    backend: HotkeyBackend,
    midi_device: usize,
    retrigger: Retrigger,
    dark: bool,
}

impl App {
    fn new() -> Self {
        let folders = ["Memes", "Music", "SFX", "Downloads"]
            .map(String::from)
            .to_vec();
        let names = [
            ("Airhorn", 0, Some("Ctrl+F13"), Some("Note 36 ch1"), 2.1),
            ("Bruh", 0, Some("Ctrl+F14"), None, 0.8),
            ("Sad Trombone", 0, None, Some("Note 37 ch1"), 3.4),
            ("Vine Boom", 0, Some("F15"), None, 1.2),
            ("Windows XP Error", 0, None, None, 1.0),
            ("Crickets", 0, None, None, 4.5),
            ("Intro Theme", 1, Some("Ctrl+Alt+1"), None, 28.0),
            ("Elevator Music", 1, None, Some("CC 20 ch1"), 95.0),
            ("Drumroll", 2, None, Some("Note 40 ch1"), 5.0),
            ("Applause", 2, Some("F16"), None, 6.2),
            ("Rimshot", 2, None, None, 1.4),
            ("Door Knock", 2, None, None, 1.9),
            ("Notification Ding", 2, None, None, 0.6),
            ("Laugh Track", 2, Some("F17"), None, 3.8),
        ];
        let sounds = names
            .iter()
            .enumerate()
            .map(|(i, (n, f, h, m, d))| Sound {
                name: n.to_string(),
                folder: *f,
                hotkey: h.map(String::from),
                midi: m.map(String::from),
                duration: *d,
                start: 0.0,
                end: *d,
                peaks: fake_peaks(i as u32, 400),
            })
            .collect();
        Self {
            folders,
            sounds,
            tab: 0,
            search: String::new(),
            playing: Vec::new(),
            targets: vec![
                "Discord (WEBRTC VoiceEngine)",
                "Firefox (meet.google.com)",
                "OBS Studio",
                "TeamSpeak 6",
            ],
            target: 0,
            local_volume: 0.6,
            call_volume: 0.8,
            play_locally: true,
            edit: None,
            url: None,
            settings_open: false,
            backend: HotkeyBackend::Portal,
            midi_device: 0,
            retrigger: Retrigger::Overlap,
            dark: true,
        }
    }

    fn play(&mut self, sound: usize) {
        let already = self.playing.iter().position(|p| p.sound == sound);
        match (self.retrigger, already) {
            (Retrigger::Stop, Some(i)) => {
                self.playing.remove(i);
            }
            (Retrigger::Restart, Some(i)) => self.playing[i].started = Instant::now(),
            _ => self.playing.push(Playing {
                sound,
                started: Instant::now(),
            }),
        }
    }

    fn progress(&self, sound: usize) -> Option<f32> {
        let s = &self.sounds[sound];
        self.playing
            .iter()
            .filter(|p| p.sound == sound)
            .map(|p| p.started.elapsed().as_secs_f32() / (s.end - s.start))
            .fold(None, |acc: Option<f32>, v| Some(acc.map_or(v, |a| a.min(v))))
    }

    fn open_editor(&mut self, sound: usize) {
        let s = &self.sounds[sound];
        self.edit = Some(EditState {
            sound,
            name: s.name.clone(),
            start: s.start,
            end: s.end,
            hotkey: s.hotkey.clone(),
            midi: s.midi.clone(),
            recording_hotkey: false,
            learning_midi: false,
            preview_started: None,
        });
    }
}

/// Deterministic pseudo-random waveform envelope.
fn fake_peaks(seed: u32, n: usize) -> Vec<f32> {
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = (x % 1000) as f32 / 1000.0;
            let t = i as f32 / n as f32;
            let env = (t * 3.0).min(1.0) * (1.0 - t).powf(0.6);
            let beat = 0.6 + 0.4 * ((t * 40.0 + seed as f32).sin()).abs();
            (env * beat * (0.5 + 0.5 * noise)).clamp(0.02, 1.0)
        })
        .collect()
}

fn fmt_time(secs: f32) -> String {
    format!("{}:{:05.2}", (secs / 60.0) as u32, secs % 60.0)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.set_visuals(if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });

        // Drop finished fake playbacks.
        let sounds = &self.sounds;
        self.playing.retain(|p| {
            let s = &sounds[p.sound];
            p.started.elapsed().as_secs_f32() < s.end - s.start
        });
        if !self.playing.is_empty()
            || self.url.as_ref().is_some_and(|u| u.download_started.is_some())
            || self.edit.as_ref().is_some_and(|e| e.preview_started.is_some())
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        self.top_bar(ui);
        self.bottom_bar(ui);
        egui::CentralPanel::default().show(ui, |ui| self.board(ui));

        self.url_dialog(&ctx);
        self.edit_dialog(&ctx);
        self.settings_dialog(&ctx);
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Soundboard");
                ui.separator();
                ui.label("Inject into:");
                egui::ComboBox::from_id_salt("target")
                    .width(240.0)
                    .selected_text(self.targets[self.target])
                    .show_ui(ui, |ui| {
                        for (i, t) in self.targets.iter().enumerate() {
                            ui.selectable_value(&mut self.target, i, *t);
                        }
                    });
                if ui.button("⟳").on_hover_text("Refresh applications").clicked() {}
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙ Settings").clicked() {
                        self.settings_open = true;
                    }
                    if ui.button("🔗 Add from URL").clicked() {
                        self.url = Some(UrlState {
                            url: String::new(),
                            folder: self.folders.len() - 1,
                            download_started: None,
                        });
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for (i, f) in self.folders.iter().enumerate() {
                    ui.selectable_value(&mut self.tab, i, f.as_str());
                }
                if ui.button("➕ Folder").on_hover_text("Add folder as tab").clicked() {
                    self.folders.push(format!("New Folder {}", self.folders.len() + 1));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("🔍 Search all sounds…")
                            .desired_width(220.0),
                    );
                });
            });
            ui.add_space(4.0);
        });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("bottom").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.playing.is_empty(), egui::Button::new("⏹ Stop all"))
                    .clicked()
                {
                    self.playing.clear();
                }
                ui.separator();
                let now: Vec<_> = self
                    .playing
                    .iter()
                    .map(|p| self.sounds[p.sound].name.as_str())
                    .collect();
                if now.is_empty() {
                    ui.weak("Nothing playing");
                } else {
                    ui.label(format!("▶ {}", now.join(", ")));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::Slider::new(&mut self.call_volume, 0.0..=1.0).show_value(false));
                    ui.label("📞 Call");
                    ui.separator();
                    ui.add_enabled(
                        self.play_locally,
                        egui::Slider::new(&mut self.local_volume, 0.0..=1.0).show_value(false),
                    );
                    ui.checkbox(&mut self.play_locally, "🔊 Local");
                });
            });
            ui.add_space(4.0);
        });
    }

    fn board(&mut self, ui: &mut egui::Ui) {
        let query = self.search.to_lowercase();
        let visible: Vec<usize> = (0..self.sounds.len())
            .filter(|&i| {
                let s = &self.sounds[i];
                if query.is_empty() {
                    s.folder == self.tab
                } else {
                    s.name.to_lowercase().contains(&query)
                }
            })
            .collect();

        if visible.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.weak("No sounds. Drop audio files into this folder or use “Add from URL”.");
            });
            return;
        }

        let tile = Vec2::new(170.0, 74.0);
        let cols = ((ui.available_width() + 8.0) / (tile.x + 8.0)).floor().max(1.0) as usize;
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("sounds")
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    for (n, &i) in visible.iter().enumerate() {
                        self.tile(ui, i, tile);
                        if (n + 1) % cols == 0 {
                            ui.end_row();
                        }
                    }
                });
        });
    }

    fn tile(&mut self, ui: &mut egui::Ui, i: usize, size: Vec2) {
        let progress = self.progress(i);
        let s = &self.sounds[i];
        let mut label = s.name.clone();
        let binds: Vec<&str> = [s.hotkey.as_deref(), s.midi.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        label.push('\n');
        label.push_str(&if binds.is_empty() {
            fmt_time(s.end - s.start)
        } else {
            binds.join(" · ")
        });

        let resp = ui.add_sized(
            size,
            egui::Button::new(label).selected(progress.is_some()),
        );
        if let Some(p) = progress {
            let r = resp.rect;
            let bar = Rect::from_min_size(
                Pos2::new(r.left(), r.bottom() - 3.0),
                Vec2::new(r.width() * p.clamp(0.0, 1.0), 3.0),
            );
            ui.painter()
                .rect_filled(bar, 0.0, ui.visuals().selection.stroke.color);
        }
        if resp.clicked() {
            self.play(i);
        }
        resp.context_menu(|ui| {
            if ui.button("✏ Edit / trim…").clicked() {
                self.open_editor(i);
                ui.close();
            }
            if ui.button("⌨ Set hotkey…").clicked() {
                self.open_editor(i);
                ui.close();
            }
            if ui.button("🎹 MIDI learn…").clicked() {
                self.open_editor(i);
                ui.close();
            }
            ui.separator();
            if ui.button("📂 Show in file manager").clicked() {
                ui.close();
            }
        });
        resp.on_hover_text("Click to play · Right-click for options");
    }

    fn url_dialog(&mut self, ctx: &egui::Context) {
        let Some(st) = &mut self.url else { return };
        let mut open = true;
        let mut finished = false;
        egui::Window::new("Add sound from URL")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Paste a YouTube (or any yt-dlp supported) URL:");
                ui.add(
                    egui::TextEdit::singleline(&mut st.url)
                        .hint_text("https://www.youtube.com/watch?v=…")
                        .desired_width(420.0),
                );
                ui.horizontal(|ui| {
                    ui.label("Save to folder:");
                    egui::ComboBox::from_id_salt("url-folder")
                        .selected_text(&self.folders[st.folder])
                        .show_ui(ui, |ui| {
                            for (i, f) in self.folders.iter().enumerate() {
                                ui.selectable_value(&mut st.folder, i, f.as_str());
                            }
                        });
                });
                ui.add_space(6.0);
                match st.download_started {
                    None => {
                        if ui
                            .add_enabled(!st.url.trim().is_empty(), egui::Button::new("⬇ Download"))
                            .clicked()
                        {
                            st.download_started = Some(Instant::now());
                        }
                    }
                    Some(t) => {
                        let p = t.elapsed().as_secs_f32() / 2.5;
                        let stage = if p < 0.6 { "Downloading…" } else { "Converting to .ogg…" };
                        ui.add(egui::ProgressBar::new(p.min(1.0)).text(stage).show_percentage());
                        if p >= 1.0 {
                            finished = true;
                        }
                    }
                }
            });
        if finished {
            let folder = st.folder;
            self.sounds.push(Sound {
                name: "Downloaded Clip".into(),
                folder,
                hotkey: None,
                midi: None,
                duration: 12.0,
                start: 0.0,
                end: 12.0,
                peaks: fake_peaks(99, 400),
            });
            self.url = None;
            self.tab = folder;
            self.open_editor(self.sounds.len() - 1);
        } else if !open {
            self.url = None;
        }
    }

    fn edit_dialog(&mut self, ctx: &egui::Context) {
        let Some(ed) = &mut self.edit else { return };
        let sound = &self.sounds[ed.sound];
        let duration = sound.duration;
        let mut open = true;
        let mut save = false;
        let mut cancel = false;

        egui::Window::new("Edit sound")
            .open(&mut open)
            .collapsible(false)
            .default_width(640.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut ed.name);
                });
                ui.add_space(6.0);
                ui.label("Trim (drag the handles):");

                // Waveform with start/end handles.
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 120.0), Sense::drag());
                let painter = ui.painter_at(rect);
                let vis = ui.visuals();
                painter.rect_filled(rect, 4.0, vis.extreme_bg_color);
                let to_x = |t: f32| rect.left() + rect.width() * (t / duration);
                let from_x = |x: f32| ((x - rect.left()) / rect.width() * duration).clamp(0.0, duration);
                let n = sound.peaks.len();
                for (i, p) in sound.peaks.iter().enumerate() {
                    let x = rect.left() + rect.width() * (i as f32 + 0.5) / n as f32;
                    let t = duration * i as f32 / n as f32;
                    let inside = t >= ed.start && t <= ed.end;
                    let color = if inside {
                        vis.selection.stroke.color
                    } else {
                        vis.weak_text_color()
                    };
                    let h = p * rect.height() * 0.45;
                    painter.line_segment(
                        [Pos2::new(x, rect.center().y - h), Pos2::new(x, rect.center().y + h)],
                        Stroke::new(1.0, color),
                    );
                }
                let shade = Color32::from_black_alpha(90);
                painter.rect_filled(
                    Rect::from_min_max(rect.min, Pos2::new(to_x(ed.start), rect.bottom())),
                    0.0,
                    shade,
                );
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(to_x(ed.end), rect.top()), rect.max),
                    0.0,
                    shade,
                );
                for t in [ed.start, ed.end] {
                    let x = to_x(t);
                    painter.line_segment(
                        [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                        Stroke::new(2.0, vis.strong_text_color()),
                    );
                }
                if let Some(st) = ed.preview_started {
                    let t = ed.start + st.elapsed().as_secs_f32();
                    if t >= ed.end {
                        ed.preview_started = None;
                    } else {
                        let x = to_x(t);
                        painter.line_segment(
                            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                            Stroke::new(1.5, Color32::from_rgb(230, 80, 80)),
                        );
                    }
                }
                if let Some(pos) = resp.interact_pointer_pos() {
                    let t = from_x(pos.x);
                    // Move whichever handle is closer.
                    if (t - ed.start).abs() < (t - ed.end).abs() {
                        ed.start = t.min(ed.end - 0.05);
                    } else {
                        ed.end = t.max(ed.start + 0.05);
                    }
                }

                ui.horizontal(|ui| {
                    ui.label("Start");
                    ui.add(egui::DragValue::new(&mut ed.start).range(0.0..=ed.end).speed(0.01).suffix(" s"));
                    ui.label("End");
                    ui.add(egui::DragValue::new(&mut ed.end).range(ed.start..=duration).speed(0.01).suffix(" s"));
                    ui.weak(format!("Length {}", fmt_time(ed.end - ed.start)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("↺ Reset").clicked() {
                            ed.start = 0.0;
                            ed.end = duration;
                        }
                        let label = if ed.preview_started.is_some() { "⏹ Stop" } else { "▶ Preview" };
                        if ui.button(label).clicked() {
                            ed.preview_started = match ed.preview_started {
                                Some(_) => None,
                                None => Some(Instant::now()),
                            };
                        }
                    });
                });

                ui.separator();
                egui::Grid::new("binds").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Hotkey:");
                    ui.monospace(ed.hotkey.as_deref().unwrap_or("—"));
                    ui.horizontal(|ui| {
                        let label = if ed.recording_hotkey { "Press a key combination…" } else { "⌨ Record" };
                        if ui.button(label).clicked() {
                            if ed.recording_hotkey {
                                ed.hotkey = Some("Ctrl+Shift+F18".into());
                            }
                            ed.recording_hotkey = !ed.recording_hotkey;
                        }
                        if ui.button("✖").on_hover_text("Clear").clicked() {
                            ed.hotkey = None;
                        }
                    });
                    ui.end_row();
                    ui.label("MIDI:");
                    ui.monospace(ed.midi.as_deref().unwrap_or("—"));
                    ui.horizontal(|ui| {
                        let label = if ed.learning_midi { "Hit a pad / key…" } else { "🎹 Learn" };
                        if ui.button(label).clicked() {
                            if ed.learning_midi {
                                ed.midi = Some("Note 42 ch10".into());
                            }
                            ed.learning_midi = !ed.learning_midi;
                        }
                        if ui.button("✖").on_hover_text("Clear").clicked() {
                            ed.midi = None;
                        }
                    });
                    ui.end_row();
                });
                if ed.recording_hotkey {
                    ui.weak("(prototype: click the button again to simulate a key press)");
                }
                if ed.learning_midi {
                    ui.weak("(prototype: click the button again to simulate a MIDI note)");
                }

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if save {
            let ed = self.edit.take().unwrap();
            let s = &mut self.sounds[ed.sound];
            s.name = ed.name;
            s.start = ed.start;
            s.end = ed.end;
            s.hotkey = ed.hotkey;
            s.midi = ed.midi;
        } else if cancel || !open {
            self.edit = None;
        }
    }

    fn settings_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("settings").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                    ui.label("Hotkey backend:");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.backend, HotkeyBackend::Portal, "XDG portal (Wayland)");
                        ui.radio_value(&mut self.backend, HotkeyBackend::X11, "global-hotkey (X11)");
                    });
                    ui.end_row();
                    ui.label("MIDI input:");
                    let devices = ["(none)", "Akai MPD218", "Arturia BeatStep"];
                    egui::ComboBox::from_id_salt("midi")
                        .selected_text(devices[self.midi_device])
                        .show_ui(ui, |ui| {
                            for (i, d) in devices.iter().enumerate() {
                                ui.selectable_value(&mut self.midi_device, i, *d);
                            }
                        });
                    ui.end_row();
                    ui.label("Re-trigger playing sound:");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.retrigger, Retrigger::Overlap, "Overlap");
                        ui.radio_value(&mut self.retrigger, Retrigger::Restart, "Restart");
                        ui.radio_value(&mut self.retrigger, Retrigger::Stop, "Stop");
                    });
                    ui.end_row();
                    ui.label("Sound folders root:");
                    ui.horizontal(|ui| {
                        ui.monospace("~/Music/Soundboard");
                        let _ = ui.button("Change…");
                    });
                    ui.end_row();
                    ui.label("Theme:");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.dark, true, "Dark");
                        ui.radio_value(&mut self.dark, false, "Light");
                    });
                    ui.end_row();
                });
            });
        self.settings_open = open;
    }
}
