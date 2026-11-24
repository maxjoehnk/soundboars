//! The application: [`App`] owns the state and the services, and runs the frame loop: poll the
//! services, let [`ui`] draw the state and collect [`Action`]s, then apply them. Drawing never
//! changes state or talks to services itself.

mod services;
mod state;
mod targets;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use eframe::egui;

use crate::audio::{EngineEvent, Retrigger};
use crate::config::{ConfigStore, SavedTarget, Settings, TabConfig, Theme};
use crate::library::watch::{FolderEvent, FolderWatcher};

pub use services::{AudioBackend, FolderPicker, PortalFolderPicker};
pub use state::{AppState, format_duration};
pub use targets::{NO_APPS_HINT, Targets};

/// How often to look for engine and folder events while idle.
const IDLE_POLL: Duration = Duration::from_millis(250);
/// How often to redraw while sounds play, for their progress bars.
const PLAYING_REPAINT: Duration = Duration::from_millis(50);

/// Something the user did.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Play(PathBuf),
    StopAll,
    SelectTab(usize),
    Search(String),
    /// Link to this recording node, or unlink with `None`.
    SelectTarget(Option<u32>),
    /// Retry linking to the remembered target.
    RefreshTargets,
    AddFolder,
    /// Removes the tab; the folder and its files stay.
    RemoveTab(usize),
    ChangeSetting(Setting),
    ShowSettings(bool),
    DismissError(usize),
}

/// A change to [`Settings`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Setting {
    CallVolume(f32),
    LocalVolume(f32),
    LocalEnabled(bool),
    Retrigger(Retrigger),
    Theme(Theme),
}

impl Setting {
    fn write_to(self, settings: &mut Settings) {
        match self {
            Self::CallVolume(volume) => settings.call_volume = volume,
            Self::LocalVolume(volume) => settings.local_volume = volume,
            Self::LocalEnabled(enabled) => settings.local_enabled = enabled,
            Self::Retrigger(retrigger) => settings.retrigger = retrigger,
            Self::Theme(theme) => settings.theme = theme,
        }
    }

    /// Tells the audio engine, if the setting concerns it.
    fn apply_to(self, audio: &dyn AudioBackend) {
        match self {
            Self::CallVolume(volume) => audio.set_call_volume(volume),
            Self::LocalVolume(volume) => audio.set_local_volume(volume),
            Self::LocalEnabled(enabled) => audio.set_local_enabled(enabled),
            Self::Retrigger(retrigger) => audio.set_retrigger_policy(retrigger),
            Self::Theme(_) => {}
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
}

pub struct App {
    state: AppState,
    audio: Option<Box<dyn AudioBackend>>,
    watcher: Box<dyn FolderWatcher>,
    folder_picker: Box<dyn FolderPicker>,
    picked_folders: (mpsc::Sender<PathBuf>, mpsc::Receiver<PathBuf>),
    store: ConfigStore,
    /// The config changed and hasn't been saved yet.
    unsaved: bool,
    applied_theme: Option<Theme>,
}

impl App {
    /// Loads the config, scans and watches the tab folders and applies the settings to the audio
    /// engine.
    pub fn new(services: Services) -> Self {
        let (config, config_error) = services.config.load();
        let mut state = AppState::new(config);
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
        let mut app = Self {
            state,
            audio,
            watcher: services.watcher,
            folder_picker: services.folder_picker,
            picked_folders: mpsc::channel(),
            store: services.config,
            unsaved: false,
            applied_theme: None,
        };
        app.watch_tabs();
        app.apply_settings();
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
        while let Ok(folder) = self.picked_folders.1.try_recv() {
            self.add_tab(folder);
        }
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Play(path) => self.play(&path),
            Action::StopAll => self.with_audio(|audio| audio.stop_all()),
            Action::SelectTab(tab) => self.state.select_tab(tab),
            Action::Search(query) => self.state.search = query,
            Action::SelectTarget(node_id) => self.select_target(node_id),
            Action::RefreshTargets => self.relink(),
            Action::AddFolder => self
                .folder_picker
                .pick_folder(self.picked_folders.0.clone()),
            Action::RemoveTab(tab) => self.remove_tab(tab),
            Action::ChangeSetting(setting) => self.change_setting(setting),
            Action::ShowSettings(open) => self.state.settings_open = open,
            Action::DismissError(index) => {
                if index < self.state.errors.len() {
                    self.state.errors.remove(index);
                }
            }
        }
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
            EngineEvent::Error(message) => self.show_error(message),
            EngineEvent::DecodeFailed { path, error } => {
                self.show_error(format!("Could not play {}: {error}", path.display()));
            }
            EngineEvent::Started(playing) => {
                self.state.durations.insert(playing.path, playing.duration);
            }
            EngineEvent::Finished { .. } => {}
        }
    }

    fn folder_event(&mut self, event: FolderEvent) {
        match event {
            FolderEvent::Listing(path) => self.state.library.changed(&path),
            FolderEvent::Contents(path) => {
                self.state.durations.remove(&path);
                self.with_audio(|audio| audio.invalidate(&path));
            }
        }
    }

    fn play(&mut self, path: &Path) {
        if let Some(request) = self.state.play_request(path) {
            self.with_audio(|audio| audio.play(request));
        }
    }

    fn select_target(&mut self, node_id: Option<u32>) {
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

    /// Links to the remembered target if it runs and we aren't linked to it already.
    fn relink(&mut self) {
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

    fn watch(&mut self, dir: &Path) {
        if let Err(error) = self.watcher.watch(dir) {
            // The tab shows why the folder can't be read; a missing folder is the usual reason.
            tracing::warn!("could not watch {}: {error}", dir.display());
        }
    }

    fn change_setting(&mut self, setting: Setting) {
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        self.apply_theme(&ctx);
        for action in ui::draw(ui, &self.state) {
            self.apply(action);
        }
        self.save_if_idle(&ctx);
        ctx.request_repaint_after(if self.state.playing.is_empty() {
            IDLE_POLL
        } else {
            PLAYING_REPAINT
        });
    }
}
