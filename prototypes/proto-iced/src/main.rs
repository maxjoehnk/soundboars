//! Click prototype for the soundboard UI using iced with default styling.
//! All data is fake: there is no audio, no hotkeys, no downloads.

use std::time::Instant;

use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke};
use iced::widget::{
    button, center, checkbox, column, container, grid, mouse_area, opaque, pick_list,
    progress_bar, radio, row, rule, scrollable, slider, space, stack, text, text_input,
};
use iced::{mouse, window, Color, Element, Length, Point, Rectangle, Renderer, Size, Subscription, Theme};

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("Soundboard – iced prototype")
        .theme(App::theme)
        .subscription(App::subscription)
        .window_size((960.0, 640.0))
        .run()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyBackend {
    Portal,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    folder: String,
    download_started: Option<Instant>,
}

#[derive(Debug, Clone)]
enum Message {
    Tick(#[allow(dead_code)] Instant),
    SelectTab(usize),
    AddFolder,
    Search(String),
    Play(usize),
    StopAll,
    Target(String),
    LocalVolume(f32),
    CallVolume(f32),
    PlayLocally(bool),
    // URL dialog
    OpenUrl,
    UrlChanged(String),
    UrlFolder(String),
    Download,
    // Editor
    Edit(usize),
    EditName(String),
    Trim(f32, f32),
    Preview,
    ResetTrim,
    RecordHotkey,
    ClearHotkey,
    LearnMidi,
    ClearMidi,
    Save,
    // Settings
    OpenSettings,
    Backend(HotkeyBackend),
    MidiDevice(String),
    Retrigger(Retrigger),
    Dark(bool),
    CloseDialog,
}

struct App {
    folders: Vec<String>,
    sounds: Vec<Sound>,
    tab: usize,
    search: String,
    playing: Vec<Playing>,
    targets: Vec<String>,
    target: String,
    local_volume: f32,
    call_volume: f32,
    play_locally: bool,
    edit: Option<EditState>,
    url: Option<UrlState>,
    settings_open: bool,
    backend: HotkeyBackend,
    midi_devices: Vec<String>,
    midi_device: String,
    retrigger: Retrigger,
    dark: bool,
}

impl App {
    fn new() -> Self {
        let folders = ["Memes", "Music", "SFX", "Downloads"].map(String::from).to_vec();
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
        let targets: Vec<String> = [
            "Discord (WEBRTC VoiceEngine)",
            "Firefox (meet.google.com)",
            "OBS Studio",
            "TeamSpeak 6",
        ]
        .map(String::from)
        .to_vec();
        let midi_devices: Vec<String> =
            ["(none)", "Akai MPD218", "Arturia BeatStep"].map(String::from).to_vec();
        Self {
            folders,
            sounds,
            tab: 0,
            search: String::new(),
            playing: Vec::new(),
            target: targets[0].clone(),
            targets,
            local_volume: 0.6,
            call_volume: 0.8,
            play_locally: true,
            edit: None,
            url: None,
            settings_open: false,
            backend: HotkeyBackend::Portal,
            midi_device: midi_devices[0].clone(),
            midi_devices,
            retrigger: Retrigger::Overlap,
            dark: true,
        }
    }

    fn theme(&self) -> Theme {
        if self.dark { Theme::Dark } else { Theme::Light }
    }

    fn animating(&self) -> bool {
        !self.playing.is_empty()
            || self.url.as_ref().is_some_and(|u| u.download_started.is_some())
            || self.edit.as_ref().is_some_and(|e| e.preview_started.is_some())
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.animating() {
            window::frames().map(Message::Tick)
        } else {
            Subscription::none()
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

    fn update(&mut self, message: Message) {
        match message {
            Message::Tick(_) => {
                let sounds = &self.sounds;
                self.playing
                    .retain(|p| p.started.elapsed().as_secs_f32() < sounds[p.sound].end - sounds[p.sound].start);
                if let Some(ed) = &mut self.edit {
                    if let Some(t) = ed.preview_started {
                        if t.elapsed().as_secs_f32() >= ed.end - ed.start {
                            ed.preview_started = None;
                        }
                    }
                }
                let done = self
                    .url
                    .as_ref()
                    .and_then(|u| u.download_started)
                    .is_some_and(|t| t.elapsed().as_secs_f32() >= 2.5);
                if done {
                    let u = self.url.take().unwrap();
                    let folder = self.folders.iter().position(|f| *f == u.folder).unwrap_or(0);
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
                    self.tab = folder;
                    self.open_editor(self.sounds.len() - 1);
                }
            }
            Message::SelectTab(i) => self.tab = i,
            Message::AddFolder => {
                let n = self.folders.len() + 1;
                self.folders.push(format!("New Folder {n}"));
            }
            Message::Search(s) => self.search = s,
            Message::Play(i) => {
                let already = self.playing.iter().position(|p| p.sound == i);
                match (self.retrigger, already) {
                    (Retrigger::Stop, Some(j)) => {
                        self.playing.remove(j);
                    }
                    (Retrigger::Restart, Some(j)) => self.playing[j].started = Instant::now(),
                    _ => self.playing.push(Playing { sound: i, started: Instant::now() }),
                }
            }
            Message::StopAll => self.playing.clear(),
            Message::Target(t) => self.target = t,
            Message::LocalVolume(v) => self.local_volume = v,
            Message::CallVolume(v) => self.call_volume = v,
            Message::PlayLocally(b) => self.play_locally = b,
            Message::OpenUrl => {
                self.url = Some(UrlState {
                    url: String::new(),
                    folder: self.folders.last().cloned().unwrap_or_default(),
                    download_started: None,
                })
            }
            Message::UrlChanged(s) => {
                if let Some(u) = &mut self.url {
                    u.url = s
                }
            }
            Message::UrlFolder(f) => {
                if let Some(u) = &mut self.url {
                    u.folder = f
                }
            }
            Message::Download => {
                if let Some(u) = &mut self.url {
                    u.download_started = Some(Instant::now())
                }
            }
            Message::Edit(i) => self.open_editor(i),
            Message::EditName(s) => {
                if let Some(e) = &mut self.edit {
                    e.name = s
                }
            }
            Message::Trim(s, e) => {
                if let Some(ed) = &mut self.edit {
                    ed.start = s;
                    ed.end = e;
                }
            }
            Message::Preview => {
                if let Some(ed) = &mut self.edit {
                    ed.preview_started = match ed.preview_started {
                        Some(_) => None,
                        None => Some(Instant::now()),
                    }
                }
            }
            Message::ResetTrim => {
                if let Some(ed) = &mut self.edit {
                    ed.start = 0.0;
                    ed.end = self.sounds[ed.sound].duration;
                }
            }
            Message::RecordHotkey => {
                if let Some(ed) = &mut self.edit {
                    if ed.recording_hotkey {
                        ed.hotkey = Some("Ctrl+Shift+F18".into());
                    }
                    ed.recording_hotkey = !ed.recording_hotkey;
                }
            }
            Message::ClearHotkey => {
                if let Some(ed) = &mut self.edit {
                    ed.hotkey = None
                }
            }
            Message::LearnMidi => {
                if let Some(ed) = &mut self.edit {
                    if ed.learning_midi {
                        ed.midi = Some("Note 42 ch10".into());
                    }
                    ed.learning_midi = !ed.learning_midi;
                }
            }
            Message::ClearMidi => {
                if let Some(ed) = &mut self.edit {
                    ed.midi = None
                }
            }
            Message::Save => {
                if let Some(ed) = self.edit.take() {
                    let s = &mut self.sounds[ed.sound];
                    s.name = ed.name;
                    s.start = ed.start;
                    s.end = ed.end;
                    s.hotkey = ed.hotkey;
                    s.midi = ed.midi;
                }
            }
            Message::OpenSettings => self.settings_open = true,
            Message::Backend(b) => self.backend = b,
            Message::MidiDevice(d) => self.midi_device = d,
            Message::Retrigger(r) => self.retrigger = r,
            Message::Dark(d) => self.dark = d,
            Message::CloseDialog => {
                self.edit = None;
                self.url = None;
                self.settings_open = false;
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let base = column![self.top_bar(), rule::horizontal(1), self.board(), rule::horizontal(1), self.bottom_bar()];

        let dialog: Option<Element<'_, Message>> = if let Some(ed) = &self.edit {
            Some(self.edit_dialog(ed))
        } else if let Some(u) = &self.url {
            Some(self.url_dialog(u))
        } else if self.settings_open {
            Some(self.settings_dialog())
        } else {
            None
        };

        match dialog {
            None => base.into(),
            Some(d) => modal(base.into(), d),
        }
    }

    fn top_bar(&self) -> Element<'_, Message> {
        let header = row![
            text("Soundboard").size(22),
            text("Inject into:"),
            pick_list(self.targets.as_slice(), Some(&self.target), Message::Target).width(260),
            button("⟳").style(button::secondary),
            space::horizontal(),
            button("Add from URL").on_press(Message::OpenUrl),
            button("Settings").style(button::secondary).on_press(Message::OpenSettings),
        ]
        .spacing(10)
        .align_y(iced::Center);

        let mut tabs = row![].spacing(4).align_y(iced::Center);
        for (i, f) in self.folders.iter().enumerate() {
            let style = if i == self.tab { button::primary } else { button::text };
            tabs = tabs.push(button(text(f.as_str())).style(style).on_press(Message::SelectTab(i)));
        }
        tabs = tabs
            .push(button("+ Folder").style(button::text).on_press(Message::AddFolder))
            .push(space::horizontal())
            .push(text_input("Search all sounds…", &self.search).on_input(Message::Search).width(240));

        column![header, tabs].spacing(10).padding(10).into()
    }

    fn board(&self) -> Element<'_, Message> {
        let query = self.search.to_lowercase();
        let tiles: Vec<Element<'_, Message>> = self
            .sounds
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                if query.is_empty() {
                    s.folder == self.tab
                } else {
                    s.name.to_lowercase().contains(&query)
                }
            })
            .map(|(i, s)| self.tile(i, s))
            .collect();

        if tiles.is_empty() {
            return center(text("No sounds. Drop audio files into this folder or use “Add from URL”."))
                .into();
        }

        scrollable(container(grid(tiles).spacing(8).fluid(190).height(76.0)).padding(10))
            .height(Length::Fill)
            .into()
    }

    fn tile<'a>(&'a self, i: usize, s: &'a Sound) -> Element<'a, Message> {
        let progress = self.progress(i);
        let binds: Vec<&str> = [s.hotkey.as_deref(), s.midi.as_deref()].into_iter().flatten().collect();
        let sub = if binds.is_empty() { fmt_time(s.end - s.start) } else { binds.join(" · ") };
        let mut content = column![text(s.name.as_str()), text(sub).size(12)].spacing(4);
        if let Some(p) = progress {
            content = content.push(progress_bar(0.0..=1.0, p).girth(3));
        }
        let style = if progress.is_some() { button::primary } else { button::secondary };
        mouse_area(
            button(content)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(style)
                .on_press(Message::Play(i)),
        )
        .on_right_press(Message::Edit(i))
        .into()
    }

    fn bottom_bar(&self) -> Element<'_, Message> {
        let now: Vec<&str> = self.playing.iter().map(|p| self.sounds[p.sound].name.as_str()).collect();
        let status = if now.is_empty() { "Nothing playing".to_string() } else { format!("▶ {}", now.join(", ")) };
        let stop = button("Stop all")
            .style(button::danger)
            .on_press_maybe((!now.is_empty()).then_some(Message::StopAll));
        let local_slider = slider(0.0..=1.0, self.local_volume, Message::LocalVolume).step(0.01_f32).width(120);
        row![
            stop,
            text(status),
            space::horizontal(),
            checkbox(self.play_locally).label("Local").on_toggle(Message::PlayLocally),
            local_slider,
            text("Call"),
            slider(0.0..=1.0, self.call_volume, Message::CallVolume).step(0.01_f32).width(120),
        ]
        .spacing(12)
        .padding(10)
        .align_y(iced::Center)
        .into()
    }

    fn url_dialog<'a>(&'a self, u: &'a UrlState) -> Element<'a, Message> {
        let action: Element<'_, Message> = match u.download_started {
            None => button("Download")
                .on_press_maybe((!u.url.trim().is_empty()).then_some(Message::Download))
                .into(),
            Some(t) => {
                let p = (t.elapsed().as_secs_f32() / 2.5).min(1.0);
                let stage = if p < 0.6 { "Downloading…" } else { "Converting to .ogg…" };
                column![progress_bar(0.0..=1.0, p), text(format!("{stage} {:.0}%", p * 100.0)).size(12)]
                    .spacing(4)
                    .into()
            }
        };
        dialog_card(
            "Add sound from URL",
            column![
                text("Paste a YouTube (or any yt-dlp supported) URL:"),
                text_input("https://www.youtube.com/watch?v=…", &u.url).on_input(Message::UrlChanged),
                row![
                    text("Save to folder:"),
                    pick_list(self.folders.as_slice(), Some(&u.folder), Message::UrlFolder)
                ]
                .spacing(8)
                .align_y(iced::Center),
                action,
            ]
            .spacing(10)
            .into(),
            460,
        )
    }

    fn edit_dialog<'a>(&'a self, ed: &'a EditState) -> Element<'a, Message> {
        let s = &self.sounds[ed.sound];
        let playhead = ed.preview_started.map(|t| ed.start + t.elapsed().as_secs_f32());
        let wave = Canvas::new(Waveform {
            peaks: &s.peaks,
            duration: s.duration,
            start: ed.start,
            end: ed.end,
            playhead,
        })
        .width(Length::Fill)
        .height(120);

        let trim_row = row![
            text(format!("Start {}", fmt_time(ed.start))),
            text(format!("End {}", fmt_time(ed.end))),
            text(format!("Length {}", fmt_time(ed.end - ed.start))).size(12),
            space::horizontal(),
            button(if ed.preview_started.is_some() { "Stop" } else { "Preview" }).on_press(Message::Preview),
            button("Reset").style(button::secondary).on_press(Message::ResetTrim),
        ]
        .spacing(12)
        .align_y(iced::Center);

        let hotkey_row = row![
            text("Hotkey:").width(70),
            text(ed.hotkey.clone().unwrap_or("—".into())).font(iced::Font::MONOSPACE).width(160),
            button(if ed.recording_hotkey { "Press a key combination…" } else { "Record" })
                .style(button::secondary)
                .on_press(Message::RecordHotkey),
            button("✕").style(button::text).on_press(Message::ClearHotkey),
        ]
        .spacing(8)
        .align_y(iced::Center);
        let midi_row = row![
            text("MIDI:").width(70),
            text(ed.midi.clone().unwrap_or("—".into())).font(iced::Font::MONOSPACE).width(160),
            button(if ed.learning_midi { "Hit a pad / key…" } else { "Learn" })
                .style(button::secondary)
                .on_press(Message::LearnMidi),
            button("✕").style(button::text).on_press(Message::ClearMidi),
        ]
        .spacing(8)
        .align_y(iced::Center);

        let mut body = column![
            row![text("Name:"), text_input("", &ed.name).on_input(Message::EditName)]
                .spacing(8)
                .align_y(iced::Center),
            text("Trim (drag the handles):").size(13),
            wave,
            trim_row,
            rule::horizontal(1),
            hotkey_row,
            midi_row,
        ]
        .spacing(10);
        if ed.recording_hotkey || ed.learning_midi {
            body = body.push(text("(prototype: click the button again to simulate input)").size(12));
        }
        body = body.push(rule::horizontal(1)).push(
            row![
                button("Save").on_press(Message::Save),
                button("Cancel").style(button::secondary).on_press(Message::CloseDialog)
            ]
            .spacing(8),
        );
        dialog_card("Edit sound", body.into(), 680)
    }

    fn settings_dialog(&self) -> Element<'_, Message> {
        let b = Some(self.backend);
        let r = Some(self.retrigger);
        let line = |label: &'static str, w: Element<'static, Message>| -> Element<'static, Message> {
            row![text(label).width(190), w].spacing(8).align_y(iced::Center).into()
        };
        dialog_card(
            "Settings",
            column![
                line(
                    "Hotkey backend:",
                    column![
                        radio("XDG portal (Wayland)", HotkeyBackend::Portal, b, Message::Backend),
                        radio("global-hotkey (X11)", HotkeyBackend::X11, b, Message::Backend),
                    ]
                    .spacing(6)
                    .into()
                ),
                line(
                    "MIDI input:",
                    pick_list(self.midi_devices.clone(), Some(self.midi_device.clone()), Message::MidiDevice).into()
                ),
                line(
                    "Re-trigger playing sound:",
                    row![
                        radio("Overlap", Retrigger::Overlap, r, Message::Retrigger),
                        radio("Restart", Retrigger::Restart, r, Message::Retrigger),
                        radio("Stop", Retrigger::Stop, r, Message::Retrigger),
                    ]
                    .spacing(12)
                    .into()
                ),
                line(
                    "Sound folders root:",
                    row![
                        text("~/Music/Soundboard").font(iced::Font::MONOSPACE),
                        button("Change…").style(button::secondary)
                    ]
                    .spacing(8)
                    .align_y(iced::Center)
                    .into()
                ),
                line(
                    "Theme:",
                    row![
                        radio("Dark", true, Some(self.dark), Message::Dark),
                        radio("Light", false, Some(self.dark), Message::Dark),
                    ]
                    .spacing(12)
                    .into()
                ),
            ]
            .spacing(14)
            .into(),
            560,
        )
    }
}

fn dialog_card<'a>(title: &'a str, body: Element<'a, Message>, width: u32) -> Element<'a, Message> {
    container(
        column![
            row![text(title).size(20), space::horizontal(), button("✕").style(button::text).on_press(Message::CloseDialog)]
                .align_y(iced::Center),
            body
        ]
        .spacing(12),
    )
    .width(width)
    .padding(16)
    .style(container::rounded_box)
    .into()
}

fn modal<'a>(base: Element<'a, Message>, content: Element<'a, Message>) -> Element<'a, Message> {
    stack![
        base,
        opaque(
            mouse_area(center(opaque(content)).style(|_theme| container::Style {
                background: Some(Color { a: 0.6, ..Color::BLACK }.into()),
                ..container::Style::default()
            }))
            .on_press(Message::CloseDialog)
        )
    ]
    .into()
}

/// Waveform with draggable start/end handles.
struct Waveform<'a> {
    peaks: &'a [f32],
    duration: f32,
    start: f32,
    end: f32,
    playhead: Option<f32>,
}

#[derive(Default)]
enum Drag {
    #[default]
    None,
    Start,
    End,
}

impl canvas::Program<Message> for Waveform<'_> {
    type State = Drag;

    fn update(
        &self,
        state: &mut Drag,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let to_t = |x: f32| ((x - bounds.x) / bounds.width * self.duration).clamp(0.0, self.duration);
        let apply = |state: &Drag, t: f32| match state {
            Drag::Start => Some(Message::Trim(t.min(self.end - 0.05), self.end)),
            Drag::End => Some(Message::Trim(self.start, t.max(self.start + 0.05))),
            Drag::None => None,
        };
        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let pos = cursor.position_in(bounds)?;
                let t = to_t(pos.x + bounds.x);
                *state = if (t - self.start).abs() < (t - self.end).abs() { Drag::Start } else { Drag::End };
                apply(state, t).map(|m| canvas::Action::publish(m).and_capture())
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                apply(state, to_t(position.x)).map(|m| canvas::Action::publish(m).and_capture())
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                *state = Drag::None;
                None
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &Drag,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let pal = theme.extended_palette();
        let mut frame = Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), pal.background.weak.color);

        let to_x = |t: f32| w * t / self.duration;
        let n = self.peaks.len();
        for (i, p) in self.peaks.iter().enumerate() {
            let x = w * (i as f32 + 0.5) / n as f32;
            let t = self.duration * i as f32 / n as f32;
            let color = if t >= self.start && t <= self.end {
                pal.primary.base.color
            } else {
                pal.background.strong.color
            };
            let amp = p * h * 0.45;
            frame.stroke(
                &Path::line(Point::new(x, h / 2.0 - amp), Point::new(x, h / 2.0 + amp)),
                Stroke::default().with_color(color).with_width(1.0),
            );
        }
        let shade = Color { a: 0.35, ..Color::BLACK };
        frame.fill_rectangle(Point::ORIGIN, Size::new(to_x(self.start), h), shade);
        frame.fill_rectangle(Point::new(to_x(self.end), 0.0), Size::new(w - to_x(self.end), h), shade);
        for t in [self.start, self.end] {
            let x = to_x(t);
            frame.stroke(
                &Path::line(Point::new(x, 0.0), Point::new(x, h)),
                Stroke::default().with_color(pal.background.base.text).with_width(2.0),
            );
        }
        if let Some(t) = self.playhead.filter(|t| *t < self.end) {
            let x = to_x(t);
            frame.stroke(
                &Path::line(Point::new(x, 0.0), Point::new(x, h)),
                Stroke::default().with_color(pal.danger.base.color).with_width(1.5),
            );
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, _state: &Drag, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if cursor.is_over(bounds) { mouse::Interaction::ResizingHorizontally } else { mouse::Interaction::default() }
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
