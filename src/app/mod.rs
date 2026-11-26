//! The application: [`App`] owns the state and the services, and runs the frame loop: poll the
//! services, let [`ui`] draw the state and collect [`Action`]s, then apply them. Drawing never
//! changes state or talks to services itself. Hotkeys and MIDI triggers play without frames, on
//! the `triggers` thread ([`Dispatcher`]).

mod hotkeys;
mod midi;
mod services;
mod sound_editor;
mod state;
mod targets;
mod triggers;
mod ui;
mod url_import;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::Duration;

use eframe::egui;

use crate::audio::decode::probe_duration;
use crate::audio::{EngineEvent, Retrigger, Waveform};
use crate::config::{ConfigStore, Injection, SavedTarget, Settings, TabConfig, Theme};
use crate::hotkeys::{BackendKind, Hotkey};
use crate::import::{DownloadEvent, Downloader};
use crate::library::durations::DurationProbe;
use crate::library::watch::{FolderEvent, FolderWatcher};
use crate::midi::{HOTPLUG_POLL, MidiEvent, MidiInput, MidiService, MidiTrigger};

pub use hotkeys::STOP_ALL_ID;
pub use midi::{LearnTarget, MidiAction, MidiState};
pub use services::{
    AudioBackend, FolderPicker, HotkeyBackends, Player, PortalFolderPicker, SystemHotkeys,
};
pub use sound_editor::{EditorAction, SoundEditor, WaveformState};
pub use state::{AppState, format_duration};
pub use targets::{NO_APPS_HINT, Targets, VIRTUAL_MIC_HINT, VIRTUAL_MIC_LABEL};
use triggers::{Dispatcher, Trigger};
use ui::{Dialog, View};
pub use url_import::{ImportStatus, UrlImport, UrlImportAction};

/// The app ID of all windows (Wayland app ID, X11 class), and the portal's and `.desktop` file's.
pub const APP_ID: &str = "me.maxjoehnk.soundboars";
/// The window icon.
pub const ICON_PNG: &[u8] = include_bytes!("../../assets/icon-256.png");

/// How often to look for engine and folder events while idle.
const IDLE_POLL: Duration = Duration::from_millis(250);
/// How often to redraw while sounds play (for their progress bars), a waveform is loading or a
/// download runs.
const PLAYING_REPAINT: Duration = Duration::from_millis(50);

/// Something the user did.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Play(PathBuf),
    StopAll,
    SelectTab(usize),
    Search(String),
    /// Link to this recording node, or unlink with `None` (both [`Injection::App`]).
    SelectTarget(Option<u32>),
    /// Inject through the virtual microphone ([`Injection::VirtualMic`]).
    SelectVirtualMic,
    /// Retry linking to the remembered target.
    RefreshTargets,
    AddFolder,
    /// Removes the tab; the folder and its files stay.
    RemoveTab(usize),
    ChangeSetting(Setting),
    ShowSettings(bool),
    DismissError(usize),
    /// Opens the "Edit sound" window for the sound at this path.
    EditSound(PathBuf),
    Editor(EditorAction),
    /// Starts (`true`) or cancels recording the "Stop all" hotkey in the settings.
    RecordStopAllHotkey(bool),
    Midi(MidiAction),
    /// Opens the "Add sound from URL" window.
    OpenUrlImport,
    UrlImport(UrlImportAction),
}

/// A change to [`Settings`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Setting {
    CallVolume(f32),
    LocalVolume(f32),
    LocalEnabled(bool),
    Retrigger(Retrigger),
    Theme(Theme),
    HotkeyBackend(BackendKind),
    StopAllHotkey(Option<Hotkey>),
}

impl Setting {
    fn write_to(self, settings: &mut Settings) {
        match self {
            Self::CallVolume(volume) => settings.call_volume = volume,
            Self::LocalVolume(volume) => settings.local_volume = volume,
            Self::LocalEnabled(enabled) => settings.local_enabled = enabled,
            Self::Retrigger(retrigger) => settings.retrigger = retrigger,
            Self::Theme(theme) => settings.theme = theme,
            Self::HotkeyBackend(kind) => settings.hotkey_backend = Some(kind),
            Self::StopAllHotkey(hotkey) => settings.stop_all_hotkey = hotkey,
        }
    }

    /// Tells the audio engine, if the setting concerns it.
    fn apply_to(self, audio: &dyn AudioBackend) {
        match self {
            Self::CallVolume(volume) => audio.set_call_volume(volume),
            Self::LocalVolume(volume) => audio.set_local_volume(volume),
            Self::LocalEnabled(enabled) => audio.set_local_enabled(enabled),
            Self::Retrigger(retrigger) => audio.set_retrigger_policy(retrigger),
            Self::Theme(_) | Self::HotkeyBackend(_) | Self::StopAllHotkey(_) => {}
        }
    }
}

/// Everything the app talks to.
pub struct Services {
    /// The error explains why there's no audio (e.g. PipeWire isn't running).
    pub audio: Result<Box<dyn AudioBackend>, String>,
    pub watcher: Box<dyn FolderWatcher>,
    pub folder_picker: Box<dyn FolderPicker>,
    pub config: ConfigStore,
    pub hotkeys: Box<dyn HotkeyBackends>,
    pub midi: Box<dyn MidiInput>,
    pub downloader: Box<dyn Downloader>,
}

pub struct App {
    state: AppState,
    audio: Option<Box<dyn AudioBackend>>,
    watcher: Box<dyn FolderWatcher>,
    folder_picker: Box<dyn FolderPicker>,
    /// `None` if its thread couldn't start.
    midi: Option<MidiService>,
    /// Plays hotkeys and MIDI triggers; `None` if its thread couldn't start.
    triggers: Option<Dispatcher>,
    picked_folders: (mpsc::Sender<PathBuf>, mpsc::Receiver<PathBuf>),
    store: ConfigStore,
    downloader: Box<dyn Downloader>,
    /// Measures the sounds for their tiles; `None` if its thread couldn't start.
    durations: Option<DurationProbe>,
    /// The config changed and hasn't been saved yet.
    unsaved: bool,
    applied_theme: Option<Theme>,
    hotkeys: hotkeys::Hotkeys,
}

impl App {
    /// Loads the config, scans and watches the tab folders, applies the settings to the audio
    /// engine and starts handling hotkeys and MIDI. Background threads wake `ctx` when there is
    /// something new to show.
    pub fn new(services: Services, ctx: &egui::Context) -> Self {
        let (config, config_error) = services.config.load();
        let mut state = AppState::new(config);
        state.url_import_unavailable = services.downloader.unavailable();
        state
            .errors
            .extend(config_error.map(|error| error.to_string()));
        let audio = match services.audio {
            Ok(audio) => Some(audio),
            Err(error) => {
                state.audio_available = false;
                state
                    .errors
                    .push(format!("No audio, playback is disabled: {error}"));
                None
            }
        };
        let ctx = ctx.clone();
        let wake = move || ctx.request_repaint();
        let (trigger_tx, trigger_rx) = mpsc::channel();
        let player = audio.as_ref().map(|audio| audio.player());
        let triggers = Dispatcher::start(trigger_rx, player, wake.clone())
            .inspect_err(|error| {
                state
                    .errors
                    .push(format!("Hotkeys and MIDI can't play sounds: {error}"));
            })
            .ok();
        let hotkeys = hotkeys::Hotkeys::new(services.hotkeys, trigger_tx.clone(), wake.clone());
        let mut app = Self {
            state,
            audio,
            watcher: services.watcher,
            folder_picker: services.folder_picker,
            midi: start_midi(services.midi, trigger_tx, wake),
            triggers,
            picked_folders: mpsc::channel(),
            store: services.config,
            downloader: services.downloader,
            durations: start_duration_probe(),
            unsaved: false,
            applied_theme: None,
            hotkeys,
        };
        app.watch_tabs();
        app.library_changed();
        app.apply_settings();
        app.start_injection();
        app.sync_hotkeys();
        app.select_midi_input();
        app.sync_triggers();
        for error in &app.state.errors {
            tracing::warn!("{error}");
        }
        app
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Handles everything that happened since the last frame.
    pub fn poll(&mut self) {
        let events = self.audio.as_ref().map(|audio| audio.poll_events());
        for event in events.into_iter().flatten() {
            self.engine_event(event);
        }
        if let Some(audio) = &self.audio {
            self.state.playing = audio.playing();
        }
        for event in self.watcher.poll() {
            self.folder_event(event);
        }
        let midi_events = self.midi.as_ref().map(MidiService::poll);
        for event in midi_events.into_iter().flatten() {
            self.midi_event(event);
        }
        while let Ok(folder) = self.picked_folders.1.try_recv() {
            self.add_tab(folder);
        }
        for error in self.hotkeys.errors() {
            self.show_error(error);
        }
        let learned = self.triggers.as_ref().map(Dispatcher::learned);
        for trigger in learned.into_iter().flatten() {
            self.midi_learned(&trigger);
        }
        for event in self.downloader.poll() {
            self.download_event(event);
        }
        let measured = self.durations.as_ref().map(DurationProbe::poll);
        for (path, duration) in measured.into_iter().flatten() {
            match duration {
                Ok(duration) => {
                    self.state.durations.insert(path, duration);
                }
                Err(error) => tracing::debug!("no length for {}: {error}", path.display()),
            }
        }
        self.sync_triggers();
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Play(path) => self.play(&path),
            Action::StopAll => self.with_audio(|audio| audio.stop_all()),
            Action::SelectTab(tab) => self.state.select_tab(tab),
            Action::Search(query) => self.state.search = query,
            Action::SelectTarget(node_id) => self.select_target(node_id),
            Action::SelectVirtualMic => self.select_virtual_mic(),
            Action::RefreshTargets => self.relink(),
            Action::AddFolder => self
                .folder_picker
                .pick_folder(self.picked_folders.0.clone()),
            Action::RemoveTab(tab) => self.remove_tab(tab),
            Action::ChangeSetting(setting) => self.change_setting(setting),
            Action::ShowSettings(open) => {
                self.state.settings_open = open;
                if !open {
                    self.state.midi.stop_learning(LearnTarget::StopAll);
                }
            }
            Action::DismissError(index) => {
                if index < self.state.errors.len() {
                    self.state.errors.remove(index);
                }
            }
            Action::EditSound(path) => self.edit_sound(&path),
            Action::Editor(action) => self.editor_action(action),
            Action::Midi(action) => self.midi_action(action),
            Action::OpenUrlImport => {
                let tab = self.state.tab;
                self.state
                    .url_import
                    .get_or_insert_with(|| UrlImport::open(tab));
            }
            Action::UrlImport(action) => self.url_import_action(action),
            Action::RecordStopAllHotkey(recording) => self.state.recording_stop_all = recording,
        }
        self.sync_hotkeys();
        self.sync_triggers();
    }

    /// Tells the `triggers` thread what hotkeys and MIDI triggers do now. Cheap enough to do after
    /// every change: it only resolves the bound sounds.
    fn sync_triggers(&self) {
        if let Some(triggers) = &self.triggers {
            triggers.set_table(self.state.trigger_table());
        }
    }

    /// Starts the chosen hotkey backend and registers the config's bindings, if either changed.
    fn sync_hotkeys(&mut self) {
        let kind = self.state.hotkey_backend();
        if let Some(error) = self.hotkeys.sync(kind, &self.state.config) {
            self.show_error(error);
        }
        self.state.hotkey_capabilities = self.hotkeys.capabilities();
    }

    /// Saves the config if it changed, but not while a button is held (e.g. while dragging a
    /// slider), so a drag writes the file once.
    pub fn save_if_idle(&mut self, ctx: &egui::Context) {
        if self.unsaved && !ctx.input(|input| input.pointer.any_down()) {
            self.save();
        }
    }

    fn save(&mut self) {
        self.unsaved = false;
        if let Err(error) = self.store.save(&self.state.config) {
            self.show_error(error.to_string());
        }
    }

    fn engine_event(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::RecordingApps(apps) => {
                self.state.targets.set_apps(apps);
                self.relink();
            }
            EngineEvent::Microphone(microphone) => self.state.targets.set_microphone(microphone),
            EngineEvent::Error(message) => self.show_error(message),
            EngineEvent::DecodeFailed { path, error } => {
                self.show_error(format!("Could not play {}: {error}", path.display()));
            }
            EngineEvent::Started(_) | EngineEvent::Finished { .. } => {}
            EngineEvent::Waveform { path, waveform } => self.waveform_loaded(&path, waveform),
        }
    }

    fn url_import_action(&mut self, action: UrlImportAction) {
        let Some(import) = &mut self.state.url_import else {
            return;
        };
        match action {
            UrlImportAction::Download => {
                if let Some((url, folder)) = import.start(&self.state.config.tabs) {
                    self.downloader.start(&url, &folder);
                }
            }
            UrlImportAction::Close => {
                self.downloader.cancel();
                self.state.url_import = None;
            }
            _ => import.edit(action),
        }
    }

    fn download_event(&mut self, event: DownloadEvent) {
        let Some(import) = &mut self.state.url_import else {
            return;
        };
        match event {
            DownloadEvent::Progress(stage) => import.status = ImportStatus::Running(stage),
            DownloadEvent::Failed(error) => import.status = ImportStatus::Failed(error),
            DownloadEvent::Done(path) => {
                self.state.url_import = None;
                if let Some(tab) = path.parent().and_then(|dir| self.state.tab_index(dir)) {
                    self.state.select_tab(tab);
                }
                self.edit_sound(&path);
            }
        }
    }

    fn edit_sound(&mut self, path: &Path) {
        if self.state.library.find(path).is_none() {
            // E.g. a file that was just downloaded, before the folder watcher reported it.
            self.state.library.changed(path);
            self.library_changed();
        }
        let Some(saved) = self.state.sound_config(path) else {
            self.show_error(format!("{} is not in any tab", path.display()));
            return;
        };
        self.close_editor();
        let mut editor = SoundEditor::open(path, &saved);
        match &self.audio {
            Some(audio) => audio.load_waveform(path),
            None => editor.waveform_loaded(Err("no audio".to_owned())),
        }
        self.state.editor = Some(editor);
    }

    fn editor_action(&mut self, action: EditorAction) {
        let Some(editor) = &mut self.state.editor else {
            return;
        };
        match action {
            EditorAction::Preview => {
                let request = editor.preview_request();
                self.with_audio(|audio| audio.play(request));
            }
            EditorAction::StopPreview => self.with_audio(|audio| audio.stop_preview()),
            EditorAction::Save => self.save_editor(),
            EditorAction::Close => self.close_editor(),
            EditorAction::Hotkey(_) | EditorAction::AddShortcut => {
                editor.apply(action);
                self.assign_shortcut_id();
            }
            _ => editor.apply(action),
        }
    }

    /// Gives the edited sound a global shortcut id if it has none: the one it had when the editor
    /// opened (so removing and adding it again keeps it), else a new one.
    fn assign_shortcut_id(&mut self) {
        let Some(editor) = &self.state.editor else {
            return;
        };
        if editor.shortcut_id.is_some() {
            return;
        }
        let saved = self.state.sound_config(&editor.path);
        let id = saved
            .and_then(|saved| saved.shortcut_id)
            .unwrap_or_else(|| {
                let file_name = editor.path.file_name().unwrap_or_default();
                hotkeys::unique_shortcut_id(&self.state.config, &file_name.to_string_lossy())
            });
        if let Some(editor) = &mut self.state.editor {
            editor.shortcut_id = Some(id);
        }
    }

    fn waveform_loaded(&mut self, path: &Path, waveform: Result<Waveform, String>) {
        if let Some(editor) = &mut self.state.editor
            && editor.path == path
        {
            editor.waveform_loaded(waveform);
        }
    }

    /// Writes the editor's changes into the config and closes it.
    fn save_editor(&mut self) {
        let Some(editor) = self.state.editor.clone() else {
            return;
        };
        if let Some(config) = self.state.sound_config_mut(&editor.path) {
            editor.apply_to(config);
            self.unsaved = true;
        }
        self.close_editor();
    }

    /// Closes the editor, if open, discarding unsaved changes and stopping its preview.
    fn close_editor(&mut self) {
        self.state.midi.stop_learning(LearnTarget::Sound);
        if self.state.editor.take().is_some() {
            self.with_audio(|audio| audio.stop_preview());
        }
    }

    fn midi_event(&mut self, event: MidiEvent) {
        match event {
            MidiEvent::Ports(ports) => self.state.midi.ports = ports,
            MidiEvent::Connected(ports) => self.state.midi.connected = ports,
            MidiEvent::Error(message) => self.show_error(message),
        }
    }

    /// Learns a trigger the `triggers` thread captured while learning. Ignored if learning was
    /// cancelled since.
    fn midi_learned(&mut self, trigger: &MidiTrigger) {
        match self.state.midi.learn(trigger) {
            Some((LearnTarget::Sound, binding)) => {
                if let Some(editor) = &mut self.state.editor {
                    editor.midi_learned(binding);
                }
            }
            Some((LearnTarget::StopAll, binding)) => {
                self.state.config.settings.midi_stop_all = Some(binding);
                self.unsaved = true;
            }
            None => {}
        }
    }

    fn midi_action(&mut self, action: MidiAction) {
        match action {
            MidiAction::SelectInput(device) => {
                self.state.config.settings.midi_input = device;
                self.select_midi_input();
                self.unsaved = true;
            }
            MidiAction::Learn(target) => self.state.midi.learning = Some(target),
            MidiAction::CancelLearn => self.state.midi.learning = None,
            MidiAction::ClearStopAll => {
                self.state.config.settings.midi_stop_all = None;
                self.unsaved = true;
            }
        }
    }

    /// Tells the MIDI thread which ports to listen to.
    fn select_midi_input(&self) {
        if let Some(midi) = &self.midi {
            midi.select(self.state.config.settings.midi_input.clone());
        }
    }

    fn folder_event(&mut self, event: FolderEvent) {
        match event {
            FolderEvent::Listing(path) => {
                self.state.library.changed(&path);
                self.library_changed();
            }
            FolderEvent::Contents(path) => {
                self.state.durations.remove(&path);
                if let Some(durations) = &mut self.durations {
                    durations.forget(&path);
                }
                self.with_audio(|audio| audio.invalidate(&path));
                self.library_changed();
            }
        }
    }

    fn play(&mut self, path: &Path) {
        if let Some(request) = self.state.play_request(path) {
            self.with_audio(|audio| audio.play(request));
        }
    }

    /// Sets up the saved injection mode; app targets are linked once their app shows up
    /// ([`Self::relink`]).
    fn start_injection(&mut self) {
        if self.state.config.injection == Injection::VirtualMic {
            self.with_audio(|audio| audio.use_virtual_mic());
        }
    }

    /// Switches to the virtual microphone; the remembered app target stays for switching back.
    fn select_virtual_mic(&mut self) {
        self.state.config.injection = Injection::VirtualMic;
        self.state.targets.set_linked(None);
        self.with_audio(|audio| audio.use_virtual_mic());
        self.unsaved = true;
    }

    fn select_target(&mut self, node_id: Option<u32>) {
        self.state.config.injection = Injection::App;
        let app = node_id.and_then(|id| {
            self.state
                .targets
                .apps()
                .iter()
                .find(|app| app.node_id == id)
        });
        self.state.config.target = app.map(SavedTarget::of);
        match app.map(|app| app.node_id) {
            Some(node_id) => self.link_to(node_id),
            None => {
                self.state.targets.set_linked(None);
                self.with_audio(|audio| audio.unlink());
            }
        }
        self.unsaved = true;
    }

    /// Links to the remembered target if it runs and we aren't linked to it already (only with
    /// [`Injection::App`]).
    fn relink(&mut self) {
        if self.state.config.injection != Injection::App {
            return;
        }
        if let Some(node_id) = self.state.targets.relink(self.state.config.target.as_ref()) {
            tracing::info!("linking to the remembered target (node {node_id})");
            self.link_to(node_id);
        }
    }

    fn link_to(&mut self, node_id: u32) {
        if let Some(audio) = &self.audio {
            audio.link_to(node_id);
            self.state.targets.set_linked(Some(node_id));
        }
    }

    fn add_tab(&mut self, dir: PathBuf) {
        if let Some(tab) = self.state.tab_index(&dir) {
            self.state.select_tab(tab);
            return;
        }
        self.watch(&dir);
        self.state.library.add(dir.clone());
        self.library_changed();
        self.state.config.tabs.push(TabConfig::for_folder(dir));
        self.state.select_tab(self.state.config.tabs.len() - 1);
        self.unsaved = true;
    }

    fn remove_tab(&mut self, tab: usize) {
        if tab >= self.state.config.tabs.len() {
            return;
        }
        let removed = self.state.config.tabs.remove(tab);
        self.state.library.remove(tab);
        self.watcher.unwatch(&removed.path);
        self.library_changed();
        self.state.select_tab(self.state.tab);
        self.unsaved = true;
    }

    fn watch_tabs(&mut self) {
        let dirs: Vec<PathBuf> = self
            .state
            .config
            .tabs
            .iter()
            .map(|tab| tab.path.clone())
            .collect();
        for dir in dirs {
            self.watch(&dir);
        }
    }

    /// Follows a change of the library's sounds: measures the new ones and keeps exactly the
    /// listed ones decoded, so they play right away.
    fn library_changed(&mut self) {
        self.measure_sounds();
        let sounds = self.state.library.folders().iter().flat_map(|f| &f.sounds);
        let paths = sounds.map(|sound| sound.path.clone()).collect();
        self.with_audio(|audio| audio.preload(paths));
    }

    /// Starts measuring the sounds whose length isn't known yet (each sound only once).
    fn measure_sounds(&mut self) {
        let Some(durations) = &mut self.durations else {
            return;
        };
        let sounds = self.state.library.folders().iter().flat_map(|f| &f.sounds);
        for sound in sounds {
            durations.request(&sound.path);
        }
    }

    fn watch(&mut self, dir: &Path) {
        if let Err(error) = self.watcher.watch(dir) {
            // The tab shows why the folder can't be read; a missing folder is the usual reason.
            tracing::warn!("could not watch {}: {error}", dir.display());
        }
    }

    fn change_setting(&mut self, setting: Setting) {
        if let Setting::StopAllHotkey(_) = setting {
            self.state.recording_stop_all = false;
        }
        setting.write_to(&mut self.state.config.settings);
        self.with_audio(|audio| setting.apply_to(audio));
        self.unsaved = true;
    }

    /// Sends all settings to the audio engine, e.g. on startup.
    fn apply_settings(&self) {
        let settings = &self.state.config.settings;
        self.with_audio(|audio| {
            audio.set_call_volume(settings.call_volume);
            audio.set_local_volume(settings.local_volume);
            audio.set_local_enabled(settings.local_enabled);
            audio.set_retrigger_policy(settings.retrigger);
        });
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let theme = self.state.config.settings.theme;
        if self.applied_theme != Some(theme) {
            ctx.set_theme(match theme {
                Theme::Dark => egui::Theme::Dark,
                Theme::Light => egui::Theme::Light,
            });
            self.applied_theme = Some(theme);
        }
    }

    fn with_audio(&self, call: impl FnOnce(&dyn AudioBackend)) {
        if let Some(audio) = &self.audio {
            call(audio.as_ref());
        }
    }

    fn show_error(&mut self, message: String) {
        tracing::warn!("{message}");
        self.state.errors.push(message);
    }
}

/// The MIDI thread, sending its triggers to `triggers`; `None` if it couldn't start.
fn start_midi(
    input: Box<dyn MidiInput>,
    triggers: mpsc::Sender<Trigger>,
    wake: impl Fn() + Send + Sync + 'static,
) -> Option<MidiService> {
    let on_trigger = move |trigger| {
        // Fails only when the app is gone.
        let _ = triggers.send(Trigger::Midi(trigger));
    };
    MidiService::start(input, HOTPLUG_POLL, wake, on_trigger)
        .inspect_err(|error| tracing::error!("could not start MIDI: {error}"))
        .ok()
}

fn start_duration_probe() -> Option<DurationProbe> {
    DurationProbe::start(probe_duration)
        .inspect_err(|error| tracing::error!("could not start measuring sounds: {error}"))
        .ok()
}

/// The eframe app. The main window and every open dialog are drawn in passes of their own
/// (deferred viewports), each over the shared [`App`]: Wayland compositors don't draw hidden
/// windows, and a dialog drawn as part of the main window would freeze while it's hidden.
pub struct AppWindows(Arc<Mutex<App>>);

impl AppWindows {
    pub fn new(app: App) -> Self {
        Self(Arc::new(Mutex::new(app)))
    }
}

/// Closes the OS window of the dialog `view` when it was drawn in a pass of its own and isn't open
/// anymore (Save, Cancel, …). The main window would drop it with its next pass, but that may only
/// come much later: Wayland compositors don't let hidden windows draw. Needs our eframe patch
/// (`vendor/README.md`). Embedded dialogs (tests) are drawn in the main window's pass and go away
/// on their own.
fn close_if_gone(ctx: &egui::Context, view: View, open: &[Dialog]) {
    let own_window = ctx.viewport_id() != egui::ViewportId::ROOT;
    if view != View::Main && own_window && !open.iter().any(|dialog| dialog.view == view) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// Locks the app for one pass. Passes run one after another on the UI thread; a panic in one
/// leaves the app as usable as it was before.
fn lock(app: &Mutex<App>) -> MutexGuard<'_, App> {
    app.lock().unwrap_or_else(PoisonError::into_inner)
}

impl eframe::App for AppWindows {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Unlocked before showing the dialogs: embedded ones (tests) are drawn right away.
        let dialogs = lock(&self.0).frame(ui, View::Main);
        for dialog in dialogs {
            let app = Arc::clone(&self.0);
            let view = dialog.view;
            dialog.show(ui.ctx(), move |ui| {
                lock(&app).frame(ui, view);
            });
        }
    }
}

impl App {
    /// One pass of the window `view`: takes in what happened in the background, draws the window
    /// and applies what the user did. Returns the open dialogs.
    fn frame(&mut self, ui: &mut egui::Ui, view: View) -> Vec<Dialog> {
        let ctx = ui.ctx().clone();
        self.poll();
        self.apply_theme(&ctx);
        let actions = ui::draw(ui, &self.state, view);
        let changed = !actions.is_empty();
        for action in actions {
            self.apply(action);
        }
        let dialogs = ui::dialogs(&self.state);
        close_if_gone(&ctx, view, &dialogs);
        if changed {
            // Show the new state in every window right away, not with their next poll.
            ctx.request_repaint_of(egui::ViewportId::ROOT);
            for dialog in &dialogs {
                ctx.request_repaint_of(dialog.viewport_id());
            }
        }
        self.save_if_idle(&ctx);
        self.schedule_repaint(&ctx);
        dialogs
    }

    /// Redraws the current window soon while something moves (playing sounds, a loading waveform,
    /// a download), else now and then to pick up background events.
    fn schedule_repaint(&self, ctx: &egui::Context) {
        let loading = self
            .state
            .editor
            .as_ref()
            .is_some_and(|editor| editor.waveform == WaveformState::Loading)
            || self
                .state
                .url_import
                .as_ref()
                .is_some_and(UrlImport::is_running);
        ctx.request_repaint_after(if self.state.playing.is_empty() && !loading {
            IDLE_POLL
        } else {
            PLAYING_REPAINT
        });
    }
}
