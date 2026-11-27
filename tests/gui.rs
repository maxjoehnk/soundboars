//! End-to-end tests of the UI with `egui_kittest`: the real [`App`] with a fake audio backend,
//! folder watcher and folder picker, temp folders and a temp config. Headless, no PipeWire.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::accesskit::Role;
use eframe::egui::{Key, Modifiers, PointerButton, Pos2, Vec2, ViewportEvent, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use soundboars::app::{
    App, AppWindows, AudioBackend, FolderPicker, HotkeyBackends, Player, Services,
    VIRTUAL_MIC_LABEL,
};
use soundboars::audio::{
    Destination, EngineEvent, Peak, PlayRequest, Playing, RecordingApp, Retrigger, Trim, Waveform,
};
use soundboars::color::{Preset, TileColor};
use soundboars::config::{
    ClipConfig, ClipKey, Config, ConfigStore, Injection, SavedTarget, SoundConfig, TabConfig, Theme,
};
use soundboars::hotkeys::{
    BackendKind, BindHint, Binding, Capabilities, EventSink, Hotkey, HotkeyBackend, HotkeyEvent,
};
use soundboars::import::{DownloadEvent, Downloader, Stage};
use soundboars::library::watch::{FolderEvent, FolderWatcher};
use soundboars::midi::{
    Connection, MidiBinding, MidiControl, MidiDevice, MidiError, MidiInput, MidiPort, OnMessage,
};

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Play(PlayRequest),
    StopAll,
    StopPreview,
    LoadWaveform(PathBuf),
    SetCallVolume(f32),
    SetLocalVolume(f32),
    SetLocalEnabled(bool),
    SetRetrigger(Retrigger),
    Invalidate(PathBuf),
    LinkTo(u32),
    UseVirtualMic,
    Unlink,
}

/// What the fakes saw and what they report; shared between the test and the app.
#[derive(Default)]
struct World {
    calls: Vec<Call>,
    engine_events: Vec<EngineEvent>,
    playing: Vec<Playing>,
    /// The sounds last asked to be kept decoded.
    preloaded: Vec<PathBuf>,
    watched: Vec<PathBuf>,
    folder_events: Vec<FolderEvent>,
    folder_to_pick: Option<PathBuf>,
    /// The hotkey backends started, in order.
    hotkey_backends: Vec<BackendKind>,
    /// Every set of bindings registered with the running backend.
    registered: Vec<Vec<Binding>>,
    /// Where the running hotkey backend sends its events.
    hotkey_events: Option<EventSink>,
    hotkey_capabilities: Capabilities,
    /// Starting a hotkey backend fails with this.
    hotkey_error: Option<String>,
    /// What the fake downloader reports as missing.
    downloader_unavailable: Option<String>,
    /// The `(url, folder)` of every started download.
    downloads: Vec<(String, PathBuf)>,
    cancelled_downloads: usize,
    download_events: Vec<DownloadEvent>,
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<World>>);

impl Shared {
    fn with<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn call(&self, call: Call) {
        self.with(|world| world.calls.push(call));
    }

    fn calls(&self) -> Vec<Call> {
        self.with(|world| world.calls.clone())
    }

    fn clear_calls(&self) {
        self.with(|world| world.calls.clear());
    }
}

struct FakeAudio(Shared);

impl AudioBackend for FakeAudio {
    fn play(&self, request: PlayRequest) {
        self.0.call(Call::Play(request));
    }
    fn stop_all(&self) {
        self.0.call(Call::StopAll);
    }
    fn stop_preview(&self) {
        self.0.call(Call::StopPreview);
    }
    fn load_waveform(&self, path: &Path) {
        self.0.call(Call::LoadWaveform(path.to_owned()));
    }
    fn preload(&self, paths: Vec<PathBuf>) {
        self.0.with(|world| world.preloaded = paths);
    }
    fn set_call_volume(&self, volume: f32) {
        self.0.call(Call::SetCallVolume(volume));
    }
    fn set_local_volume(&self, volume: f32) {
        self.0.call(Call::SetLocalVolume(volume));
    }
    fn set_local_enabled(&self, enabled: bool) {
        self.0.call(Call::SetLocalEnabled(enabled));
    }
    fn set_retrigger_policy(&self, retrigger: Retrigger) {
        self.0.call(Call::SetRetrigger(retrigger));
    }
    fn invalidate(&self, path: &Path) {
        self.0.call(Call::Invalidate(path.to_owned()));
    }
    fn link_to(&self, node_id: u32) {
        self.0.call(Call::LinkTo(node_id));
    }
    fn use_virtual_mic(&self) {
        self.0.call(Call::UseVirtualMic);
    }
    fn unlink(&self) {
        self.0.call(Call::Unlink);
    }
    fn poll_events(&self) -> Vec<EngineEvent> {
        self.0
            .with(|world| std::mem::take(&mut world.engine_events))
    }
    fn playing(&self) -> Vec<Playing> {
        self.0.with(|world| world.playing.clone())
    }
    fn player(&self) -> Box<dyn Player> {
        Box::new(FakeAudio(self.0.clone()))
    }
}

impl Player for FakeAudio {
    fn play(&self, request: PlayRequest) {
        self.0.call(Call::Play(request));
    }
    fn stop_all(&self) {
        self.0.call(Call::StopAll);
    }
}

struct FakeWatcher(Shared);

impl FolderWatcher for FakeWatcher {
    fn watch(&mut self, dir: &Path) -> Result<(), notify::Error> {
        self.0.with(|world| world.watched.push(dir.to_owned()));
        Ok(())
    }
    fn unwatch(&mut self, dir: &Path) {
        self.0
            .with(|world| world.watched.retain(|watched| watched != dir));
    }
    fn poll(&mut self) -> Vec<FolderEvent> {
        self.0
            .with(|world| std::mem::take(&mut world.folder_events))
    }
}

struct FakePicker(Shared);

impl FolderPicker for FakePicker {
    fn pick_folder(&self, picked: mpsc::Sender<PathBuf>) {
        if let Some(folder) = self.0.with(|world| world.folder_to_pick.take()) {
            picked.send(folder).expect("app is alive");
        }
    }
}

struct FakeHotkeys(Shared);

impl HotkeyBackends for FakeHotkeys {
    fn start(
        &self,
        kind: BackendKind,
        events: EventSink,
    ) -> Result<Option<Box<dyn HotkeyBackend>>, String> {
        self.0.with(|world| {
            world.hotkey_backends.push(kind);
            world.hotkey_events = Some(events);
            match world.hotkey_error.clone() {
                Some(error) => Err(error),
                None if kind == BackendKind::Off => Ok(None),
                None => Ok(Some(
                    Box::new(FakeHotkeyBackend(self.0.clone())) as Box<dyn HotkeyBackend>
                )),
            }
        })
    }
}

struct FakeHotkeyBackend(Shared);

impl HotkeyBackend for FakeHotkeyBackend {
    fn register(&mut self, bindings: &[Binding]) {
        self.0
            .with(|world| world.registered.push(bindings.to_vec()));
    }
    fn capabilities(&self) -> Capabilities {
        self.0.with(|world| world.hotkey_capabilities)
    }
}

/// MIDI input ports; a connected port's callback receives what the test sends.
#[derive(Default)]
struct MidiWorld {
    ports: Vec<MidiPort>,
    /// The callbacks of the connected ports, by port id.
    listeners: Vec<(String, OnMessage)>,
}

#[derive(Clone, Default)]
struct FakeMidi(Arc<Mutex<MidiWorld>>);

impl FakeMidi {
    fn with<R>(&self, f: impl FnOnce(&mut MidiWorld) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl MidiInput for FakeMidi {
    fn ports(&mut self) -> Result<Vec<MidiPort>, MidiError> {
        Ok(self.with(|world| world.ports.clone()))
    }

    fn connect(&mut self, port: &MidiPort, on_message: OnMessage) -> Result<Connection, MidiError> {
        self.with(|world| world.listeners.push((port.id.clone(), on_message)));
        Ok(Box::new(()))
    }
}

const MPD: &str = "MPD218:MPD218 MIDI 1";
const MPD_ID: &str = "20:0";

struct FakeDownloader(Shared);

impl Downloader for FakeDownloader {
    fn unavailable(&self) -> Option<String> {
        self.0.with(|world| world.downloader_unavailable.clone())
    }
    fn start(&mut self, url: &str, folder: &Path) {
        self.0
            .with(|world| world.downloads.push((url.to_owned(), folder.to_owned())));
    }
    fn cancel(&mut self) {
        self.0.with(|world| world.cancelled_downloads += 1);
    }
    fn poll(&mut self) -> Vec<DownloadEvent> {
        self.0
            .with(|world| std::mem::take(&mut world.download_events))
    }
}

/// Two tabs: `Memes` (Airhorn, Bruh) and `SFX` (Applause, Horn). Airhorn has a volume and a trim.
/// One MIDI port, the MPD218.
struct Setup {
    dir: tempfile::TempDir,
    shared: Shared,
    midi: FakeMidi,
    store: ConfigStore,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let memes = dir.path().join("Memes");
        let sfx = dir.path().join("SFX");
        for (folder, name) in [
            (&memes, "Airhorn.wav"),
            (&memes, "Bruh.wav"),
            (&sfx, "Applause.wav"),
            (&sfx, "Horn.wav"),
        ] {
            fs::create_dir_all(folder).expect("mkdir");
            fs::write(folder.join(name), b"").expect("write");
        }
        let mut memes_tab = TabConfig::for_folder(memes);
        memes_tab.sounds.insert(
            "Airhorn.wav".to_owned(),
            SoundConfig {
                volume: 0.5,
                start_ms: 100,
                end_ms: Some(900),
                ..SoundConfig::default()
            },
        );
        let store = ConfigStore::new(dir.path().join("config/config.toml"));
        let mut config = Config {
            tabs: vec![memes_tab, TabConfig::for_folder(sfx)],
            ..Config::default()
        };
        // Not the session's default, which depends on where the tests run.
        config.settings.hotkey_backend = Some(BackendKind::X11);
        store.save(&config).expect("save config");
        let midi = FakeMidi::default();
        midi.with(|world| {
            world.ports.push(MidiPort {
                id: MPD_ID.to_owned(),
                name: MPD.to_owned(),
            })
        });
        Self {
            dir,
            shared: Shared::default(),
            midi,
            store,
        }
    }

    fn with_config(self, change: impl FnOnce(&mut Config)) -> Self {
        let mut config = self.config();
        change(&mut config);
        self.store.save(&config).expect("save config");
        self
    }

    fn folder(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn config(&self) -> Config {
        let (config, error) = self.store.load();
        assert!(error.is_none(), "{error:?}");
        config
    }

    fn start(&self) -> Harness<'static, AppWindows> {
        self.start_with_audio(Ok(Box::new(FakeAudio(self.shared.clone()))))
    }

    fn start_with_audio(
        &self,
        audio: Result<Box<dyn AudioBackend>, String>,
    ) -> Harness<'static, AppWindows> {
        let services = Services {
            audio,
            watcher: Box::new(FakeWatcher(self.shared.clone())),
            folder_picker: Box::new(FakePicker(self.shared.clone())),
            config: self.store.clone(),
            hotkeys: Box::new(FakeHotkeys(self.shared.clone())),
            midi: Box::new(self.midi.clone()),
            downloader: Box::new(FakeDownloader(self.shared.clone())),
        };
        let mut harness = Harness::builder()
            .with_size([1000.0, 700.0])
            .build_eframe(|cc| AppWindows::new(App::new(services, &cc.egui_ctx)));
        harness.run();
        harness
    }
}

fn discord(node_id: u32) -> RecordingApp {
    RecordingApp {
        node_id,
        app_name: "WEBRTC VoiceEngine".to_owned(),
        process_binary: Some("Discord".to_owned()),
        media_name: None,
        ports: Vec::new(),
    }
}

/// What the target picker shows as selected.
fn target_picker_value(harness: &Harness<'_, AppWindows>) -> String {
    harness
        .get_by_label("Inject into:")
        .value()
        .unwrap_or_default()
}

fn tile(name: &str) -> String {
    format!("{name}\nWAV")
}

#[test]
fn startup_applies_the_saved_settings() {
    let setup = Setup::new().with_config(|config| {
        config.settings.call_volume = 0.7;
        config.settings.local_enabled = false;
        config.settings.retrigger = Retrigger::Stop;
    });
    let _harness = setup.start();

    let calls = setup.shared.calls();
    assert!(calls.contains(&Call::SetCallVolume(0.7)), "{calls:?}");
    assert!(calls.contains(&Call::SetLocalEnabled(false)), "{calls:?}");
    assert!(
        calls.contains(&Call::SetRetrigger(Retrigger::Stop)),
        "{calls:?}"
    );
    assert_eq!(
        setup.shared.with(|world| world.watched.clone()),
        [setup.folder("Memes"), setup.folder("SFX")]
    );
}

#[test]
fn clicking_a_tile_plays_it_with_its_volume_and_trim() {
    let setup = Setup::new();
    let mut harness = setup.start();
    setup.shared.clear_calls();

    harness.get_by_label(&tile("Airhorn")).click();
    harness.run();

    assert_eq!(
        setup.shared.calls(),
        [Call::Play(PlayRequest {
            path: setup.folder("Memes").join("Airhorn.wav"),
            clip: 0,
            trim: Some(Trim {
                start_ms: 100,
                end_ms: Some(900)
            }),
            volume: 0.5,
            destination: Destination::Everywhere,
        })]
    );
}

#[test]
fn tabs_switch_the_shown_sounds() {
    let setup = Setup::new();
    let mut harness = setup.start();
    assert!(harness.query_by_label(&tile("Bruh")).is_some());
    assert!(harness.query_by_label(&tile("Applause")).is_none());

    harness.get_by_label("SFX").click();
    harness.run();

    assert!(harness.query_by_label(&tile("Bruh")).is_none());
    assert!(harness.query_by_label(&tile("Applause")).is_some());
}

#[test]
fn search_filters_tiles_across_all_tabs() {
    let setup = Setup::new();
    let mut harness = setup.start();

    let search = harness.get_by_role(Role::TextInput);
    search.click();
    search.type_text("horn");
    harness.run();

    assert!(harness.query_by_label(&tile("Airhorn")).is_some());
    assert!(harness.query_by_label(&tile("Horn")).is_some());
    assert!(harness.query_by_label(&tile("Bruh")).is_none());
    assert!(harness.query_by_label(&tile("Applause")).is_none());
}

#[test]
fn picking_a_target_links_to_it_and_remembers_it() {
    let setup = Setup::new();
    let mut harness = setup.start();
    setup.shared.with(|world| {
        world
            .engine_events
            .push(EngineEvent::RecordingApps(vec![discord(41)]))
    });
    harness.run();
    setup.shared.clear_calls();

    harness.get_by_label("Inject into:").click();
    harness.run();
    harness.get_by_label("WEBRTC VoiceEngine (Discord)").click();
    harness.run();

    assert_eq!(setup.shared.calls(), [Call::LinkTo(41)]);
    assert_eq!(
        setup.config().target,
        Some(SavedTarget {
            process_binary: Some("Discord".to_owned()),
            app_name: "WEBRTC VoiceEngine".to_owned(),
        })
    );

    harness.get_by_label("Inject into:").click();
    harness.run();
    harness.get_by_label("None").click();
    harness.run();

    assert_eq!(setup.shared.calls(), [Call::LinkTo(41), Call::Unlink]);
    assert_eq!(setup.config().target, None);
}

#[test]
fn an_empty_target_list_explains_when_apps_appear() {
    let setup = Setup::new();
    let mut harness = setup.start();

    harness.get_by_label("Inject into:").click();
    harness.run();

    assert!(
        harness
            .query_by_label_contains("Apps only appear while they record audio")
            .is_some()
    );
}

#[test]
fn the_remembered_target_is_relinked_when_it_appears() {
    let setup = Setup::new().with_config(|config| {
        config.injection = Injection::App;
        config.target = Some(SavedTarget::of(&discord(0)));
    });
    let mut harness = setup.start();
    assert_eq!(
        target_picker_value(&harness),
        "WEBRTC VoiceEngine (Discord) (not running)"
    );
    setup.shared.clear_calls();

    setup.shared.with(|world| {
        world
            .engine_events
            .push(EngineEvent::RecordingApps(vec![discord(57)]))
    });
    harness.run();

    assert_eq!(setup.shared.calls(), [Call::LinkTo(57)]);
    assert_eq!(
        target_picker_value(&harness),
        "WEBRTC VoiceEngine (Discord)"
    );
}

#[test]
fn the_virtual_mic_is_the_default_target_and_shows_the_mixed_in_microphone() {
    let setup = Setup::new();
    let mut harness = setup.start();

    assert!(setup.shared.calls().contains(&Call::UseVirtualMic));
    assert_eq!(target_picker_value(&harness), VIRTUAL_MIC_LABEL);
    assert!(harness.query_by_label("no microphone found").is_some());

    setup.shared.with(|world| {
        world
            .engine_events
            .push(EngineEvent::Microphone(Some("USB Mic".to_owned())))
    });
    harness.run();

    assert!(harness.query_by_label("with USB Mic").is_some());
    assert!(harness.query_by_label("⟳").is_none());
}

#[test]
fn switching_between_virtual_mic_and_app_keeps_the_remembered_app() {
    let setup = Setup::new().with_config(|config| {
        config.target = Some(SavedTarget::of(&discord(0)));
    });
    let mut harness = setup.start();
    setup.shared.with(|world| {
        world
            .engine_events
            .push(EngineEvent::RecordingApps(vec![discord(41)]))
    });
    harness.run();
    // The virtual mic is the saved mode, so the remembered app isn't linked.
    assert!(!setup.shared.calls().contains(&Call::LinkTo(41)));
    setup.shared.clear_calls();

    harness.get_by_label("Inject into:").click();
    harness.run();
    harness.get_by_label("WEBRTC VoiceEngine (Discord)").click();
    harness.run();

    assert_eq!(setup.shared.calls(), [Call::LinkTo(41)]);
    assert_eq!(setup.config().injection, Injection::App);

    harness.get_by_label("Inject into:").click();
    harness.run();
    harness.get_by_label(VIRTUAL_MIC_LABEL).click();
    harness.run();

    assert_eq!(
        setup.shared.calls(),
        [Call::LinkTo(41), Call::UseVirtualMic]
    );
    let config = setup.config();
    assert_eq!(config.injection, Injection::VirtualMic);
    assert_eq!(config.target, Some(SavedTarget::of(&discord(0))));
    assert_eq!(target_picker_value(&harness), VIRTUAL_MIC_LABEL);
}

#[test]
fn app_mode_starts_without_the_virtual_mic() {
    let setup = Setup::new().with_config(|config| {
        config.injection = Injection::App;
    });
    let harness = setup.start();

    assert!(!setup.shared.calls().contains(&Call::UseVirtualMic));
    assert_eq!(target_picker_value(&harness), "None");
}

#[test]
fn stop_all_stops_and_playing_sounds_are_shown() {
    let setup = Setup::new();
    let path = setup.folder("Memes").join("Bruh.wav");
    setup.shared.with(|world| {
        world.playing.push(Playing {
            voice: 0,
            path,
            clip: 0,
            started_at: Instant::now(),
            duration: Duration::from_secs(60),
            destination: Destination::Everywhere,
        })
    });
    let mut harness = setup.start();
    assert!(harness.query_by_label("▶ Bruh").is_some());
    setup.shared.clear_calls();

    harness.get_by_label("⏹ Stop all").click();
    harness.run();

    assert_eq!(setup.shared.calls(), [Call::StopAll]);
}

#[test]
fn volume_changes_reach_the_engine_and_the_config() {
    let setup = Setup::new();
    let mut harness = setup.start();
    setup.shared.clear_calls();

    harness
        .get_by_label("Call volume")
        .set_value_accesskit(0.25);
    harness.run();
    harness.get_by_label("🔊 Local").click();
    harness.run();

    assert_eq!(
        setup.shared.calls(),
        [Call::SetCallVolume(0.25), Call::SetLocalEnabled(false)]
    );
    let settings = setup.config().settings;
    assert_eq!(settings.call_volume, 0.25);
    assert!(!settings.local_enabled);
}

#[test]
fn settings_change_the_retrigger_policy_and_theme() {
    let setup = Setup::new();
    let mut harness = setup.start();

    harness.get_by_label("⚙ Settings").click();
    harness.run();
    harness.get_by_label("Restart").click();
    harness.run();

    assert!(
        setup
            .shared
            .calls()
            .contains(&Call::SetRetrigger(Retrigger::Restart))
    );
    assert_eq!(setup.config().settings.retrigger, Retrigger::Restart);

    harness.get_by_label("Light").click();
    harness.run();
    assert_eq!(harness.ctx.theme(), eframe::egui::Theme::Light);
    assert_eq!(setup.config().settings.theme, Theme::Light);
}

#[test]
fn engine_errors_are_shown_until_dismissed() {
    let setup = Setup::new();
    let mut harness = setup.start();

    setup.shared.with(|world| {
        world
            .engine_events
            .push(EngineEvent::Error("PipeWire exploded".to_owned()))
    });
    harness.run();
    assert!(harness.query_by_label("PipeWire exploded").is_some());

    harness.get_by_label("✖").click();
    harness.run();
    assert!(harness.query_by_label("PipeWire exploded").is_none());
}

#[test]
fn without_audio_the_app_starts_and_says_why() {
    let setup = Setup::new();
    let harness = setup.start_with_audio(Err("PipeWire is not running".to_owned()));

    assert!(
        harness
            .query_by_label_contains("PipeWire is not running")
            .is_some()
    );
    assert!(harness.query_by_label(&tile("Airhorn")).is_some());
}

#[test]
fn folders_can_be_added_and_removed_as_tabs() {
    let setup = Setup::new();
    let music = setup.folder("Music");
    fs::create_dir(&music).expect("mkdir");
    fs::write(music.join("Intro.wav"), b"").expect("write");
    setup
        .shared
        .with(|world| world.folder_to_pick = Some(music.clone()));
    let mut harness = setup.start();

    harness.get_by_label("➕ Folder").click();
    harness.run();

    assert!(harness.query_by_label(&tile("Intro")).is_some());
    assert!(setup.shared.with(|world| world.watched.contains(&music)));
    let tabs: Vec<String> = setup
        .config()
        .tabs
        .into_iter()
        .map(|tab| tab.name)
        .collect();
    assert_eq!(tabs, ["Memes", "SFX", "Music"]);

    harness.get_by_label("SFX").click_secondary();
    harness.run();
    harness.get_by_label("Remove tab").click();
    harness.run();

    assert!(harness.query_by_label("SFX").is_none());
    assert!(
        !setup
            .shared
            .with(|world| world.watched.contains(&setup.folder("SFX")))
    );
    let tabs: Vec<String> = setup
        .config()
        .tabs
        .into_iter()
        .map(|tab| tab.name)
        .collect();
    assert_eq!(tabs, ["Memes", "Music"]);
    assert!(setup.folder("SFX").join("Horn.wav").exists());
}

#[test]
fn folder_changes_update_tiles_and_invalidate_sounds() {
    let setup = Setup::new();
    let mut harness = setup.start();
    setup.shared.clear_calls();

    let path = setup.folder("Memes").join("Vine Boom.wav");
    fs::write(&path, b"").expect("write");
    setup.shared.with(|world| {
        world.folder_events.push(FolderEvent::Listing(path.clone()));
        world
            .folder_events
            .push(FolderEvent::Contents(path.clone()));
    });
    harness.run();

    assert!(harness.query_by_label(&tile("Vine Boom")).is_some());
    assert_eq!(setup.shared.calls(), [Call::Invalidate(path)]);
}

#[test]
fn listed_sounds_are_kept_decoded() {
    let setup = Setup::new();
    let mut harness = setup.start();
    let preloaded = || {
        let mut paths = setup.shared.with(|world| world.preloaded.clone());
        paths.sort();
        paths
    };
    assert!(preloaded().contains(&airhorn(&setup)));
    let sfx = setup.folder("SFX");
    assert!(preloaded().iter().any(|path| path.starts_with(&sfx)));

    harness.get_by_label("SFX").click_secondary();
    harness.run();
    harness.get_by_label("Remove tab").click();
    harness.run();

    assert!(preloaded().contains(&airhorn(&setup)));
    assert!(!preloaded().iter().any(|path| path.starts_with(&sfx)));
}

#[test]
fn tiles_show_the_trimmed_length_without_playing() {
    let setup = Setup::new().with_config(|config| {
        config.tabs[0].sounds.insert(
            "Tone.flac".to_owned(),
            SoundConfig {
                start_ms: 100,
                ..SoundConfig::default()
            },
        );
    });
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.flac");
    fs::copy(fixture, setup.folder("Memes").join("Tone.flac")).expect("copy");
    let mut harness = setup.start();

    // The fixture is 0.5 s long; it's measured in the background.
    let deadline = Instant::now() + Duration::from_secs(10);
    while harness.query_by_label("Tone\n0:00.40").is_none() {
        assert!(Instant::now() < deadline, "the length never appeared");
        std::thread::sleep(Duration::from_millis(20));
        harness.run();
    }
    assert!(
        setup
            .shared
            .calls()
            .iter()
            .all(|call| !matches!(call, Call::Play(_)))
    );
    // Files that can't be measured keep showing their format.
    assert!(harness.query_by_label(&tile("Airhorn")).is_some());
}

// The "Edit sound" window. Airhorn is trimmed to 0.1–0.9 s at volume 0.5; its fake waveform says
// the file is 2 s long.

const AIRHORN_MS: u64 = 2_000;
const EDITOR: &str = "Edit sound – Airhorn.wav";

fn airhorn(setup: &Setup) -> PathBuf {
    setup.folder("Memes").join("Airhorn.wav")
}

/// Opens the editor for Airhorn through the tile's context menu.
fn open_editor(harness: &mut Harness<'_, AppWindows>) {
    // The tile's subtitle may go on with the MIDI binding and the hotkey.
    harness
        .get_by_label_contains(&tile("Airhorn"))
        .click_secondary();
    harness.run();
    harness.get_by_label("✏ Edit…").click();
    harness.run();
}

/// Opens the editor for Airhorn and delivers its waveform.
fn open_editor_with_waveform(setup: &Setup, harness: &mut Harness<'_, AppWindows>) {
    open_editor(harness);
    deliver_waveform(setup, harness);
}

/// The engine reports Airhorn's waveform.
fn deliver_waveform(setup: &Setup, harness: &mut Harness<'_, AppWindows>) {
    let waveform = Waveform {
        peaks: vec![
            Peak {
                min: -0.5,
                max: 0.5
            };
            200
        ],
        duration: Duration::from_millis(AIRHORN_MS),
    };
    setup.shared.with(|world| {
        world.engine_events.push(EngineEvent::Waveform {
            path: airhorn(setup),
            waveform: Ok(waveform),
        })
    });
    harness.run();
    setup.shared.clear_calls();
}

fn saved_airhorn(setup: &Setup) -> SoundConfig {
    setup.config().tabs[0].sounds["Airhorn.wav"].clone()
}

/// The point on the waveform at `ms`.
fn waveform_point(harness: &Harness<'_, AppWindows>, ms: u64) -> Pos2 {
    let rect = harness.get_by_label("Waveform").rect();
    let x = rect.left() + rect.width() * ms as f32 / AIRHORN_MS as f32;
    Pos2::new(x, rect.center().y)
}

fn drag(harness: &mut Harness<'_, AppWindows>, from_ms: u64, to_ms: u64) {
    let from = waveform_point(harness, from_ms);
    let to = waveform_point(harness, to_ms);
    harness.hover_at(from);
    harness.run();
    harness.drag_at(from);
    harness.run();
    harness.hover_at(to);
    harness.run();
    harness.drop_at(to);
    harness.run();
}

fn click_editor_button(harness: &mut Harness<'_, AppWindows>, label: &str) {
    harness.get_by_label(label).click();
    harness.run();
}

#[test]
fn the_editor_opens_from_the_tile_menu_and_shows_the_waveform() {
    let setup = Setup::new();
    let mut harness = setup.start();
    setup.shared.clear_calls();

    open_editor(&mut harness);

    assert!(harness.query_by_label(EDITOR).is_some());
    assert!(harness.query_by_label("File: Airhorn.wav").is_some());
    assert!(harness.query_by_label("Loading waveform…").is_some());
    assert!(harness.query_by_label("Waveform").is_none());
    assert_eq!(setup.shared.calls(), [Call::LoadWaveform(airhorn(&setup))]);

    deliver_waveform(&setup, &mut harness);

    assert!(harness.query_by_label("Loading waveform…").is_none());
    assert!(harness.query_by_label("Waveform").is_some());
    assert_eq!(
        harness.get_by_label("Start").value().as_deref(),
        Some("0.10 s")
    );
    assert_eq!(
        harness.get_by_label("End").value().as_deref(),
        Some("0.90 s")
    );
    assert!(harness.query_by_label("Length 0:00.80").is_some());
}

#[test]
fn renaming_shows_the_new_name_on_the_tile_and_keeps_the_file() {
    let setup = Setup::new();
    let mut harness = setup.start();

    harness
        .get_by_label_contains(&tile("Airhorn"))
        .click_secondary();
    harness.run();
    harness.get_by_label("Rename…").click();
    harness.run();

    assert!(harness.query_by_label(RENAME).is_some());
    assert!(
        harness.query_by_label(EDITOR).is_none(),
        "not the big editor"
    );
    assert!(harness.query_by_label("File: Airhorn.wav").is_some());
    let field = harness.get_by_label("Name:");
    assert!(field.is_focused());
    assert_eq!(field.value().as_deref(), Some("Airhorn"));
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label("Name:").type_text("Horn of doom");
    harness.run();
    click_editor_button(&mut harness, "Save");

    assert!(harness.query_by_label(RENAME).is_none());
    assert_eq!(saved_airhorn(&setup).name.as_deref(), Some("Horn of doom"));
    assert!(airhorn(&setup).exists());
    assert!(harness.query_by_label_contains("Horn of doom\n").is_some());
    assert!(harness.query_by_label_contains("Airhorn\n").is_none());
}

/// Right-clicks the tile labelled `name` and picks `item` from its menu.
fn tile_menu(harness: &mut Harness<'_, AppWindows>, name: &str, item: &str) {
    harness.get_by_label_contains(&tile(name)).click_secondary();
    harness.run();
    harness.get_by_label(item).click();
    harness.run();
}

/// Clicks the tile labelled `name` and returns what it played.
fn click_tile(setup: &Setup, harness: &mut Harness<'_, AppWindows>, name: &str) -> Vec<Call> {
    setup.shared.clear_calls();
    harness.get_by_label(&tile(name)).click();
    harness.run();
    setup.shared.calls()
}

fn airhorn_clip(setup: &Setup, clip: u32, start_ms: u64) -> Call {
    Call::Play(PlayRequest {
        path: airhorn(setup),
        clip,
        trim: Some(Trim {
            start_ms,
            end_ms: Some(900),
        }),
        volume: 0.5,
        destination: Destination::Everywhere,
    })
}

#[test]
fn a_duplicated_clip_is_trimmed_on_its_own_and_plays_its_own_part() {
    let setup = Setup::new();
    let mut harness = setup.start();
    let original = saved_airhorn(&setup);

    tile_menu(&mut harness, "Airhorn", "Duplicate");

    assert!(harness.query_by_label(EDITOR).is_some());
    let field = harness.get_by_label("Name:");
    assert!(field.is_focused());
    assert_eq!(field.value().as_deref(), Some("Airhorn (2)"));
    deliver_waveform(&setup, &mut harness);
    harness.get_by_label("Start").set_value_accesskit(0.5);
    harness.run();
    click_editor_button(&mut harness, "Save");

    // Its name is the default name of the file's second clip, so it isn't stored.
    assert!(harness.query_by_label(&tile("Airhorn (2)")).is_some());
    let saved = saved_airhorn(&setup);
    assert_eq!(
        saved.clips,
        [ClipConfig {
            id: 1,
            config: SoundConfig {
                name: None,
                volume: 0.5,
                start_ms: 500,
                end_ms: Some(900),
                ..SoundConfig::default()
            },
        }]
    );
    assert_eq!(
        SoundConfig {
            clips: Vec::new(),
            ..saved
        },
        original
    );
    assert!(airhorn(&setup).exists());
    assert_eq!(
        click_tile(&setup, &mut harness, "Airhorn"),
        [airhorn_clip(&setup, 0, 100)]
    );
    assert_eq!(
        click_tile(&setup, &mut harness, "Airhorn (2)"),
        [airhorn_clip(&setup, 1, 500)]
    );
}

#[test]
fn deleting_a_clip_removes_its_tile_and_its_global_shortcut() {
    let setup = Setup::new().with_config(|config| {
        let airhorn = config.tabs[0].sounds.get_mut("Airhorn.wav");
        airhorn.expect("configured").clips.push(ClipConfig {
            id: 1,
            config: SoundConfig {
                name: Some("Intro".to_owned()),
                shortcut_id: Some("play-airhorn-2".to_owned()),
                ..SoundConfig::default()
            },
        });
    });
    let mut harness = setup.start();
    let original = saved_airhorn(&setup);
    assert_eq!(
        registered(&setup),
        [
            binding("stop-all", "Stop all sounds", None),
            binding("play-airhorn-2", "Play Intro", None),
        ]
    );
    harness
        .get_by_label_contains(&tile("Airhorn"))
        .click_secondary();
    harness.run();
    assert!(
        harness.query_by_label("Delete clip").is_none(),
        "the file's own clip can't be deleted"
    );
    harness.key_press(Key::Escape);
    harness.run();

    tile_menu(&mut harness, "Intro", "Delete clip");

    assert!(harness.query_by_label_contains("Intro\n").is_none());
    assert!(harness.query_by_label(&tile("Airhorn")).is_some());
    assert_eq!(
        saved_airhorn(&setup),
        SoundConfig {
            clips: Vec::new(),
            ..original
        }
    );
    assert_eq!(
        registered(&setup),
        [binding("stop-all", "Stop all sounds", None)]
    );
}

#[test]
fn deleting_the_clip_open_in_the_editor_closes_it() {
    // Four more clips of Airhorn: "D" is the fifth tile, right of the editor window (kittest
    // embeds it in the main window, over the first tiles).
    let setup = Setup::new().with_config(|config| {
        let airhorn = config.tabs[0].sounds.get_mut("Airhorn.wav");
        let airhorn = airhorn.expect("configured");
        for (id, name) in (1..).zip(["A", "B", "C", "D"]) {
            airhorn.clips.push(ClipConfig {
                id,
                config: SoundConfig {
                    name: Some(name.to_owned()),
                    ..SoundConfig::default()
                },
            });
        }
    });
    let mut harness = setup.start();
    tile_menu(&mut harness, "D", "✏ Edit…");
    assert!(harness.query_by_label(EDITOR).is_some());
    assert_eq!(harness.get_by_label("Name:").value().as_deref(), Some("D"));

    tile_menu(&mut harness, "D", "Delete clip");

    assert!(harness.query_by_label(EDITOR).is_none());
    let names: Vec<Option<String>> = (saved_airhorn(&setup).clips.into_iter())
        .map(|clip| clip.config.name)
        .collect();
    assert_eq!(
        names,
        [Some("A"), Some("B"), Some("C")].map(|n| n.map(str::to_owned))
    );
}

#[test]
fn a_waveform_that_cannot_be_decoded_is_explained() {
    let setup = Setup::new();
    let mut harness = setup.start();
    open_editor(&mut harness);

    setup.shared.with(|world| {
        world.engine_events.push(EngineEvent::Waveform {
            path: airhorn(&setup),
            waveform: Err("the file contains no audio".to_owned()),
        })
    });
    harness.run();

    assert!(
        harness
            .query_by_label("No waveform: the file contains no audio")
            .is_some()
    );
}

#[test]
fn dragging_moves_the_nearer_handle() {
    let setup = Setup::new();
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);

    drag(&mut harness, 150, 500);
    drag(&mut harness, 1_000, 1_500);
    click_editor_button(&mut harness, "Save");

    let saved = saved_airhorn(&setup);
    assert!(saved.start_ms.abs_diff(500) <= 10, "{saved:?}");
    assert!(
        saved.end_ms.is_some_and(|end| end.abs_diff(1_500) <= 10),
        "{saved:?}"
    );
}

#[test]
fn trim_can_be_typed_in_and_saving_to_the_end_stores_no_end() {
    let setup = Setup::new();
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);

    harness.get_by_label("Start").set_value_accesskit(0.25);
    harness.run();
    harness.get_by_label("End").set_value_accesskit(2.0);
    harness.run();
    assert!(harness.query_by_label("Length 0:01.75").is_some());
    click_editor_button(&mut harness, "Save");

    assert_eq!(
        saved_airhorn(&setup),
        SoundConfig {
            volume: 0.5,
            start_ms: 250,
            end_ms: None,
            ..SoundConfig::default()
        }
    );
}

#[test]
fn reset_plays_the_whole_sound() {
    let setup = Setup::new();
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);

    click_editor_button(&mut harness, "↺ Reset");
    click_editor_button(&mut harness, "Save");

    assert_eq!(saved_airhorn(&setup).trim(), None);
}

#[test]
fn saved_changes_are_played() {
    let setup = Setup::new();
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);

    harness.get_by_label("Start").set_value_accesskit(0.5);
    harness.run();
    harness
        .get_by_role_and_label(Role::Slider, "Volume:")
        .set_value_accesskit(80.0);
    harness.run();
    click_editor_button(&mut harness, "Save");

    assert!(harness.query_by_label(EDITOR).is_none());
    assert_eq!(
        saved_airhorn(&setup),
        SoundConfig {
            volume: 0.8,
            start_ms: 500,
            end_ms: Some(900),
            ..SoundConfig::default()
        }
    );
    setup.shared.clear_calls();
    harness.get_by_label(&tile("Airhorn")).click();
    harness.run();
    assert_eq!(
        setup.shared.calls(),
        [Call::Play(PlayRequest {
            path: airhorn(&setup),
            clip: 0,
            trim: Some(Trim {
                start_ms: 500,
                end_ms: Some(900),
            }),
            volume: 0.8,
            destination: Destination::Everywhere,
        })]
    );
}

#[test]
fn cancel_discards_the_changes() {
    let setup = Setup::new();
    let mut harness = setup.start();
    let before = saved_airhorn(&setup);
    open_editor_with_waveform(&setup, &mut harness);

    click_editor_button(&mut harness, "↺ Reset");
    click_editor_button(&mut harness, "Cancel");

    assert!(harness.query_by_label(EDITOR).is_none());
    assert_eq!(saved_airhorn(&setup), before);
    open_editor_with_waveform(&setup, &mut harness);
    assert_eq!(
        harness.get_by_label("Start").value().as_deref(),
        Some("0.10 s")
    );
}

#[test]
fn preview_plays_the_edited_sound_locally_and_stops_on_close() {
    let setup = Setup::new().with_config(|config| config.settings.local_enabled = false);
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);

    harness.get_by_label("Start").set_value_accesskit(0.3);
    harness.run();
    click_editor_button(&mut harness, "▶ Preview");

    assert_eq!(
        setup.shared.calls(),
        [Call::Play(PlayRequest {
            path: airhorn(&setup),
            clip: 0,
            trim: Some(Trim {
                start_ms: 300,
                end_ms: Some(900),
            }),
            volume: 0.5,
            destination: Destination::Preview,
        })]
    );

    // While the preview plays, the button stops it; it's not shown as playing on the board.
    let preview = Playing {
        voice: 7,
        path: airhorn(&setup),
        clip: 0,
        started_at: Instant::now(),
        duration: Duration::from_millis(600),
        destination: Destination::Preview,
    };
    setup.shared.with(|world| world.playing.push(preview));
    harness.run();
    assert!(harness.query_by_label("Nothing playing").is_some());
    setup.shared.clear_calls();
    click_editor_button(&mut harness, "⏹ Stop");
    assert_eq!(setup.shared.calls(), [Call::StopPreview]);

    setup.shared.clear_calls();
    // Closing the dialog's window. Kittest embeds dialogs in the main window, where its close
    // request is the one of the main viewport.
    harness
        .input_mut()
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .events
        .push(ViewportEvent::Close);
    harness.run();
    assert!(harness.query_by_label(EDITOR).is_none());
    assert_eq!(setup.shared.calls(), [Call::StopPreview]);
    assert_eq!(saved_airhorn(&setup).start_ms, 100);
}
// Global hotkeys, with a fake backend. The settings choose the X11 backend (see `Setup::new`).

fn hotkey(text: &str) -> Hotkey {
    text.parse().expect("valid hotkey")
}

fn binding(id: &str, description: &str, hotkey: Option<Hotkey>) -> Binding {
    Binding {
        id: id.to_owned(),
        description: description.to_owned(),
        hotkey,
    }
}

/// The bindings registered last.
fn registered(setup: &Setup) -> Vec<Binding> {
    setup
        .shared
        .with(|world| world.registered.last().cloned().unwrap_or_default())
}

/// The backend reports a hotkey event, and the app runs a frame.
fn hotkey_event(setup: &Setup, harness: &mut Harness<'_, AppWindows>, event: HotkeyEvent) {
    report_hotkey(setup, event);
    harness.run();
}

/// The backend reports a hotkey event (from its own thread, in the real app).
fn report_hotkey(setup: &Setup, event: HotkeyEvent) {
    let events = setup
        .shared
        .with(|world| world.hotkey_events.clone())
        .expect("a hotkey backend runs");
    events.send(event);
}

/// Waits until `done`, without running frames.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Runs frames until `done`. Hotkeys and MIDI triggers are handled on their own thread, so what
/// they do shows up a moment later.
fn run_until(
    harness: &mut Harness<'_, AppWindows>,
    what: &str,
    done: impl Fn(&Harness<'_, AppWindows>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    harness.run();
    while !done(harness) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
        harness.run();
    }
}

fn airhorn_request(setup: &Setup) -> PlayRequest {
    PlayRequest {
        path: airhorn(setup),
        clip: 0,
        trim: Some(Trim {
            start_ms: 100,
            end_ms: Some(900),
        }),
        volume: 0.5,
        destination: Destination::Everywhere,
    }
}

/// Airhorn has the hotkey Ctrl+F13 and the shortcut id `play-airhorn`.
fn setup_with_airhorn_hotkey() -> Setup {
    Setup::new().with_config(|config| {
        let airhorn = config.tabs[0]
            .sounds
            .get_mut("Airhorn.wav")
            .expect("airhorn");
        airhorn.hotkey = Some(hotkey("Ctrl+F13"));
        airhorn.shortcut_id = Some("play-airhorn".to_owned());
    })
}

fn hyprland() -> Capabilities {
    Capabilities {
        assign_in_app: false,
        bind_hint: Some(BindHint::Hyprland),
    }
}

#[test]
fn startup_registers_the_hotkeys_of_the_config() {
    let setup = setup_with_airhorn_hotkey();
    let harness = setup.start();

    assert_eq!(
        setup.shared.with(|world| world.hotkey_backends.clone()),
        [BackendKind::X11]
    );
    assert_eq!(
        registered(&setup),
        [
            binding("stop-all", "Stop all sounds", None),
            binding("play-airhorn", "Play Airhorn", Some(hotkey("Ctrl+F13"))),
        ]
    );
    assert!(harness.query_by_label("Airhorn\nWAV · Ctrl+F13").is_some());
}

#[test]
fn a_hotkey_recorded_in_the_editor_is_saved_and_registered() {
    let setup = Setup::new();
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);

    click_editor_button(&mut harness, "Record");
    assert!(
        harness
            .query_by_label("Press the keys… (Esc cancels)")
            .is_some()
    );
    // Modifier keys alone don't count.
    harness.key_press_modifiers(Modifiers::CTRL, Key::ShiftLeft);
    harness.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::F13);
    harness.run();
    assert!(harness.query_by_label("Ctrl+Shift+F13").is_some());
    assert!(
        registered(&setup).len() == 1,
        "registered only once saved: {:?}",
        registered(&setup)
    );
    click_editor_button(&mut harness, "Save");

    let saved = saved_airhorn(&setup);
    assert_eq!(saved.hotkey, Some(hotkey("Ctrl+Shift+F13")));
    assert_eq!(saved.shortcut_id.as_deref(), Some("play-airhorn"));
    assert_eq!(
        registered(&setup),
        [
            binding("stop-all", "Stop all sounds", None),
            binding(
                "play-airhorn",
                "Play Airhorn",
                Some(hotkey("Ctrl+Shift+F13"))
            ),
        ]
    );
    assert!(
        harness
            .query_by_label("Airhorn\nWAV · Ctrl+Shift+F13")
            .is_some()
    );
}

#[test]
fn escape_cancels_recording() {
    let setup = setup_with_airhorn_hotkey();
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);

    click_editor_button(&mut harness, "Record");
    harness.key_press(Key::Escape);
    harness.run();

    assert!(
        harness
            .query_by_label("Press the keys… (Esc cancels)")
            .is_none()
    );
    assert!(harness.query_by_label("Ctrl+F13").is_some());
}

#[test]
fn clearing_the_hotkey_removes_the_global_shortcut() {
    let setup = setup_with_airhorn_hotkey();
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);

    click_editor_button(&mut harness, "Clear hotkey");
    click_editor_button(&mut harness, "Save");

    let saved = saved_airhorn(&setup);
    assert_eq!((saved.hotkey, saved.shortcut_id), (None, None));
    assert_eq!(
        registered(&setup),
        [binding("stop-all", "Stop all sounds", None)]
    );
    assert!(harness.query_by_label(&tile("Airhorn")).is_some());
}

#[test]
fn hyprland_shows_the_shortcut_id_and_bind_line_instead_of_a_recorder() {
    let setup = Setup::new()
        .with_config(|config| config.settings.hotkey_backend = Some(BackendKind::Portal));
    setup
        .shared
        .with(|world| world.hotkey_capabilities = hyprland());
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);
    assert!(harness.query_by_label("Record").is_none());

    click_editor_button(&mut harness, "Add global shortcut");

    assert!(harness.query_by_label("play-airhorn").is_some());
    let bind_line =
        r#"hl.bind("CTRL + F13", hl.dsp.global("me.maxjoehnk.soundboars:play-airhorn"))"#;
    assert!(harness.query_by_label(bind_line).is_some());
    click_editor_button(&mut harness, "Copy");
    click_editor_button(&mut harness, "Save");

    let saved = saved_airhorn(&setup);
    assert_eq!(saved.shortcut_id.as_deref(), Some("play-airhorn"));
    assert_eq!(saved.hotkey, None);
    assert_eq!(
        registered(&setup)[1],
        binding("play-airhorn", "Play Airhorn", None)
    );

    // Removing and adding it again keeps the id users bound in their config.
    open_editor_with_waveform(&setup, &mut harness);
    click_editor_button(&mut harness, "Remove");
    click_editor_button(&mut harness, "Add global shortcut");
    assert!(harness.query_by_label("play-airhorn").is_some());
}

#[test]
fn hotkey_presses_play_sounds_and_stop_all() {
    let setup = setup_with_airhorn_hotkey();
    let mut harness = setup.start();
    setup.shared.clear_calls();

    hotkey_event(
        &setup,
        &mut harness,
        HotkeyEvent::Pressed("play-airhorn".into()),
    );
    hotkey_event(
        &setup,
        &mut harness,
        HotkeyEvent::Released("play-airhorn".into()),
    );
    hotkey_event(&setup, &mut harness, HotkeyEvent::Pressed("unknown".into()));
    hotkey_event(
        &setup,
        &mut harness,
        HotkeyEvent::Pressed("stop-all".into()),
    );

    // Presses are handled in order, so once "Stop all" arrived, the others were handled too.
    run_until(&mut harness, "Stop all", |_| {
        setup.shared.calls().contains(&Call::StopAll)
    });
    assert_eq!(
        setup.shared.calls(),
        [Call::Play(airhorn_request(&setup)), Call::StopAll]
    );
}

#[test]
fn hotkeys_play_while_the_window_gets_no_frames() {
    let setup = setup_with_airhorn_hotkey();
    let _harness = setup.start();
    setup.shared.clear_calls();

    // No `harness.run()` from here on: a hidden window on Wayland may get no frames.
    report_hotkey(&setup, HotkeyEvent::Pressed("play-airhorn".into()));
    wait_for("the play", || !setup.shared.calls().is_empty());
    assert_eq!(setup.shared.calls(), [Call::Play(airhorn_request(&setup))]);

    report_hotkey(&setup, HotkeyEvent::Pressed("stop-all".into()));
    wait_for("Stop all", || setup.shared.calls().len() == 2);
    assert_eq!(setup.shared.calls()[1], Call::StopAll);
}

#[test]
fn hotkey_errors_are_shown() {
    let setup = Setup::new();
    let mut harness = setup.start();

    hotkey_event(
        &setup,
        &mut harness,
        HotkeyEvent::Error("Global shortcuts are not available".into()),
    );

    assert!(
        harness
            .query_by_label("Global shortcuts are not available")
            .is_some()
    );
}

#[test]
fn a_backend_that_cannot_start_is_explained() {
    let setup = Setup::new();
    setup
        .shared
        .with(|world| world.hotkey_error = Some("no X11 display".into()));
    let harness = setup.start();

    assert!(
        harness
            .query_by_label("Hotkeys are not available: no X11 display")
            .is_some()
    );
    assert!(harness.query_by_label(&tile("Airhorn")).is_some());
}

#[test]
fn the_stop_all_hotkey_is_recorded_in_the_settings() {
    let setup = Setup::new();
    let mut harness = setup.start();
    harness.get_by_label("⚙ Settings").click();
    harness.run();

    harness.get_by_label("Record").click();
    harness.run();
    harness.key_press_modifiers(Modifiers::CTRL, Key::Pause);
    harness.run();

    assert_eq!(
        setup.config().settings.stop_all_hotkey,
        Some(hotkey("Ctrl+Pause"))
    );
    assert_eq!(
        registered(&setup),
        [binding(
            "stop-all",
            "Stop all sounds",
            Some(hotkey("Ctrl+Pause"))
        )]
    );
    assert!(harness.query_by_label("Ctrl+Pause").is_some());

    harness.get_by_label("Clear hotkey").click();
    harness.run();
    assert_eq!(setup.config().settings.stop_all_hotkey, None);
}

#[test]
fn switching_the_hotkey_backend_starts_it_and_registers_again() {
    let setup = setup_with_airhorn_hotkey();
    let mut harness = setup.start();
    harness.get_by_label("⚙ Settings").click();
    harness.run();
    assert!(
        harness
            .query_by_label_contains("X11 hotkeys only work in X11 sessions")
            .is_some()
    );

    harness.get_by_label("Off").click();
    harness.run();
    assert_eq!(
        setup.config().settings.hotkey_backend,
        Some(BackendKind::Off)
    );

    setup
        .shared
        .with(|world| world.hotkey_capabilities = hyprland());
    harness.get_by_label("XDG portal").click();
    harness.run();

    assert_eq!(
        setup.shared.with(|world| world.hotkey_backends.clone()),
        [BackendKind::X11, BackendKind::Off, BackendKind::Portal]
    );
    let registrations = setup.shared.with(|world| world.registered.clone());
    assert_eq!(registrations.len(), 2, "{registrations:?}");
    assert_eq!(registrations[0], registrations[1]);
    // Hyprland: the stop-all shortcut is bound in its config, not recorded.
    assert!(harness.query_by_label("Record").is_none());
    assert!(
        harness
            .query_by_label(
                r#"hl.bind("CTRL + F13", hl.dsp.global("me.maxjoehnk.soundboars:stop-all"))"#
            )
            .is_some()
    );
}

// MIDI. The fake MPD218 is the only port; tests that need it connected select it in the config.

fn listening_to_mpd(config: &mut Config) {
    config.settings.midi_input = MidiDevice::Port(MPD.to_owned());
}

fn note_36(port: Option<&str>) -> MidiBinding {
    MidiBinding {
        port: port.map(str::to_owned),
        channel: 1,
        control: MidiControl::Note(36),
    }
}

/// Sends a message from the MPD218 once the app listens to it, and runs a frame. What the message
/// triggers happens on the `triggers` thread, so check its effects with [`run_until`].
fn send_midi(setup: &Setup, harness: &mut Harness<'_, AppWindows>, message: &[u8]) {
    midi_message(setup, message);
    harness.run();
}

/// Sends a message from the MPD218 once the app listens to it. The MIDI thread connects on its
/// own, so this doesn't run frames.
fn midi_message(setup: &Setup, message: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let sent = setup.midi.with(|world| {
            let listener = world.listeners.iter_mut().find(|(id, _)| id == MPD_ID);
            listener
                .map(|(_, on_message)| on_message(message))
                .is_some()
        });
        if sent {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the app never connected to the MPD218"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn plays(setup: &Setup) -> Vec<PathBuf> {
    let calls = setup.shared.calls();
    calls
        .into_iter()
        .filter_map(|call| match call {
            Call::Play(request) => Some(request.path),
            _ => None,
        })
        .collect()
}

#[test]
fn a_note_learned_in_the_editor_is_saved_and_shown_on_the_tile() {
    let setup = Setup::new().with_config(listening_to_mpd);
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);
    assert!(harness.query_by_label("—").is_some(), "not bound");

    click_editor_button(&mut harness, "Learn");
    assert!(harness.query_by_label("Cancel learning").is_some());
    send_midi(&setup, &mut harness, &[0x90, 36, 100]);
    run_until(&mut harness, "the learned note", |harness| {
        harness.query_by_label("Cancel learning").is_none()
    });

    assert!(plays(&setup).is_empty(), "learning doesn't play");
    assert!(harness.query_by_label("Note 36 · ch 1 · MPD218").is_some());
    click_editor_button(&mut harness, "Save");

    assert_eq!(saved_airhorn(&setup).midi, Some(note_36(Some(MPD))));
    assert!(harness.query_by_label("Airhorn\nWAV · Note 36").is_some());
}

#[test]
fn a_bound_trigger_plays_the_sound_like_a_click() {
    let setup = Setup::new().with_config(|config| {
        listening_to_mpd(config);
        let airhorn = config.tabs[0].sounds.get_mut("Airhorn.wav").expect("bound");
        airhorn.midi = Some(note_36(Some(MPD)));
    });
    let mut harness = setup.start();
    setup.shared.clear_calls();

    send_midi(&setup, &mut harness, &[0x80, 36, 0]);
    send_midi(&setup, &mut harness, &[0x90, 37, 100]);
    send_midi(&setup, &mut harness, &[0x90, 36, 100]);

    // Triggers are handled in order: once note 36 played, the others did nothing.
    run_until(&mut harness, "the play", |_| !plays(&setup).is_empty());
    assert_eq!(setup.shared.calls(), [Call::Play(airhorn_request(&setup))]);
}

#[test]
fn midi_triggers_play_while_the_window_gets_no_frames() {
    let setup = Setup::new().with_config(|config| {
        listening_to_mpd(config);
        let airhorn = config.tabs[0].sounds.get_mut("Airhorn.wav").expect("bound");
        airhorn.midi = Some(note_36(Some(MPD)));
    });
    let _harness = setup.start();
    setup.shared.clear_calls();

    // No `harness.run()` from here on: a hidden window on Wayland may get no frames.
    midi_message(&setup, &[0x90, 36, 100]);
    wait_for("the play", || !setup.shared.calls().is_empty());
    assert_eq!(setup.shared.calls(), [Call::Play(airhorn_request(&setup))]);
}

#[test]
fn learning_can_be_cancelled_with_escape() {
    let setup = Setup::new().with_config(|config| {
        listening_to_mpd(config);
        config.tabs[0].sounds.insert(
            "Bruh.wav".to_owned(),
            SoundConfig {
                midi: Some(note_36(None)),
                ..SoundConfig::default()
            },
        );
    });
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);

    click_editor_button(&mut harness, "Learn");
    harness.key_press(eframe::egui::Key::Escape);
    harness.run();
    assert!(harness.query_by_label("Cancel learning").is_none());

    send_midi(&setup, &mut harness, &[0x90, 36, 100]);
    run_until(&mut harness, "the play", |_| !plays(&setup).is_empty());
    assert_eq!(plays(&setup), [setup.folder("Memes").join("Bruh.wav")]);
    assert!(harness.query_by_label("—").is_some(), "not bound");
}

#[test]
fn a_binding_can_be_cleared() {
    let setup = Setup::new().with_config(|config| {
        let airhorn = config.tabs[0].sounds.get_mut("Airhorn.wav").expect("bound");
        airhorn.midi = Some(note_36(Some(MPD)));
    });
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);
    assert!(harness.query_by_label("Note 36 · ch 1 · MPD218").is_some());

    click_editor_button(&mut harness, "Clear");
    click_editor_button(&mut harness, "Save");

    assert_eq!(saved_airhorn(&setup).midi, None);
}

#[test]
fn a_binding_can_react_to_any_device() {
    let setup = Setup::new().with_config(|config| {
        let airhorn = config.tabs[0].sounds.get_mut("Airhorn.wav").expect("bound");
        airhorn.midi = Some(note_36(Some(MPD)));
    });
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);

    click_editor_button(&mut harness, "Any device");
    assert!(harness.query_by_label("Note 36 · ch 1").is_some());
    click_editor_button(&mut harness, "Save");

    assert_eq!(saved_airhorn(&setup).midi, Some(note_36(None)));
}

#[test]
fn learning_a_binding_another_sound_uses_warns() {
    let setup = Setup::new().with_config(|config| {
        listening_to_mpd(config);
        config.tabs[1].sounds.insert(
            "Horn.wav".to_owned(),
            SoundConfig {
                midi: Some(note_36(None)),
                ..SoundConfig::default()
            },
        );
    });
    let mut harness = setup.start();
    open_editor_with_waveform(&setup, &mut harness);
    assert!(harness.query_by_label_contains("Also triggers").is_none());

    click_editor_button(&mut harness, "Learn");
    send_midi(&setup, &mut harness, &[0x90, 36, 100]);

    run_until(&mut harness, "the warning", |harness| {
        harness.query_by_label("⚠ Also triggers: Horn").is_some()
    });
    click_editor_button(&mut harness, "Save");
    assert_eq!(saved_airhorn(&setup).midi, Some(note_36(Some(MPD))));
}

#[test]
fn the_midi_input_is_chosen_in_the_settings_and_remembered() {
    let setup = Setup::new();
    let mut harness = setup.start();
    harness.get_by_label("⚙ Settings").click();
    harness.run();
    assert_eq!(
        harness.get_by_label("MIDI input:").value().as_deref(),
        Some("(none)")
    );
    assert!(harness.query_by_label("nothing").is_some());

    harness.get_by_label("MIDI input:").click();
    harness.run();
    harness.get_by_label(MPD).click();
    harness.run();

    assert_eq!(
        setup.config().settings.midi_input,
        MidiDevice::Port(MPD.to_owned())
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while harness.query_by_label("nothing").is_some() {
        assert!(Instant::now() < deadline, "never connected");
        std::thread::sleep(Duration::from_millis(10));
        harness.run();
    }
    assert!(harness.query_by_label(MPD).is_some());
}

#[test]
fn stop_all_can_be_learned_in_the_settings() {
    let setup = Setup::new().with_config(listening_to_mpd);
    let mut harness = setup.start();
    harness.get_by_label("⚙ Settings").click();
    harness.run();
    setup.shared.clear_calls();

    harness.get_by_label("Learn").click();
    harness.run();
    send_midi(&setup, &mut harness, &[0xB0, 20, 127]);
    run_until(&mut harness, "the learned CC", |_| {
        setup.config().settings.midi_stop_all.is_some()
    });

    assert!(setup.shared.calls().is_empty(), "learning doesn't stop");
    let learned = MidiBinding {
        port: Some(MPD.to_owned()),
        channel: 1,
        control: MidiControl::ControlChange(20),
    };
    assert_eq!(setup.config().settings.midi_stop_all, Some(learned));
    assert!(harness.query_by_label("CC 20 · ch 1 · MPD218").is_some());

    send_midi(&setup, &mut harness, &[0xB0, 20, 0]);
    send_midi(&setup, &mut harness, &[0xB0, 20, 127]);
    run_until(&mut harness, "Stop all", |_| {
        !setup.shared.calls().is_empty()
    });
    assert_eq!(setup.shared.calls(), [Call::StopAll]);

    harness.get_by_label("Clear").click();
    harness.run();
    assert_eq!(setup.config().settings.midi_stop_all, None);
}

// "Add sound from URL", with a fake downloader.

const URL_DIALOG: &str = "Add sound from URL";
const URL_FIELD: &str = "Paste a YouTube (or any yt-dlp supported) URL:";
const ADD_FROM_URL: &str = "🔗 Add from URL";
const DOWNLOAD: &str = "⬇ Download";
/// Frames to run after each input: the progress bar animates while downloading, so the UI never
/// settles for `Harness::run`.
const STEPS: usize = 4;

fn report_download(setup: &Setup, harness: &mut Harness<'_, AppWindows>, event: DownloadEvent) {
    setup.shared.with(|world| world.download_events.push(event));
    harness.run_steps(STEPS);
}

fn downloads(setup: &Setup) -> Vec<(String, PathBuf)> {
    setup.shared.with(|world| world.downloads.clone())
}

fn cancelled_downloads(setup: &Setup) -> usize {
    setup.shared.with(|world| world.cancelled_downloads)
}

fn type_url(harness: &mut Harness<'_, AppWindows>, url: &str) {
    let field = harness.get_by_label(URL_FIELD);
    field.click();
    field.type_text(url);
    harness.run_steps(STEPS);
}

/// Opens the dialog and types `url`.
fn enter_url(harness: &mut Harness<'_, AppWindows>, url: &str) {
    harness.get_by_label(ADD_FROM_URL).click();
    harness.run_steps(STEPS);
    type_url(harness, url);
}

#[test]
fn add_from_url_is_disabled_without_the_tools() {
    let setup = Setup::new();
    setup.shared.with(|world| {
        world.downloader_unavailable = Some("Install yt-dlp (not found: yt-dlp).".to_owned())
    });
    let mut harness = setup.start();

    let button = harness.get_by_label(ADD_FROM_URL);
    assert!(button.accesskit_node().is_disabled());
    button.hover();
    harness.run_steps(STEPS);
    assert!(
        harness
            .query_by_label_contains("not found: yt-dlp")
            .is_some()
    );
    harness.get_by_label(ADD_FROM_URL).click();
    harness.run_steps(STEPS);
    assert!(harness.query_by_label(URL_DIALOG).is_none());
}

#[test]
fn a_downloaded_sound_opens_in_the_editor_in_its_tab() {
    let setup = Setup::new();
    let mut harness = setup.start();
    enter_url(&mut harness, "youtube.com/watch?v=1");
    assert!(harness.query_by_label(URL_DIALOG).is_some());
    assert!(
        harness
            .get_by_label(DOWNLOAD)
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        harness
            .query_by_label_contains("http:// or https://")
            .is_some()
    );

    harness
        .get_by_label(URL_FIELD)
        .set_text_value_accesskit("https://example.com/clip");
    harness.run_steps(STEPS);
    harness.get_by_label("Save to tab:").click();
    harness.run_steps(STEPS);
    let tabs = harness.query_all_by_label("SFX").collect::<Vec<_>>();
    tabs.last().expect("the picker lists SFX").click();
    harness.run_steps(STEPS);
    harness.get_by_label(DOWNLOAD).click();
    harness.run_steps(STEPS);

    let sfx = setup.folder("SFX");
    assert_eq!(
        downloads(&setup),
        [("https://example.com/clip".to_owned(), sfx.clone())]
    );
    let half = DownloadEvent::Progress(Stage::Downloading(Some(0.5)));
    report_download(&setup, &mut harness, half);
    assert!(
        harness
            .query_by_label_contains("Downloading… 50%")
            .is_some()
    );
    assert!(
        harness
            .get_by_label(DOWNLOAD)
            .accesskit_node()
            .is_disabled()
    );
    let converting = DownloadEvent::Progress(Stage::Converting);
    report_download(&setup, &mut harness, converting);
    assert!(
        harness
            .query_by_label_contains("Converting to MP3…")
            .is_some()
    );

    // The folder watcher hasn't reported the new file yet.
    let clip = sfx.join("Clip.mp3");
    fs::write(&clip, b"").expect("write");
    setup.shared.clear_calls();
    report_download(&setup, &mut harness, DownloadEvent::Done(clip.clone()));

    assert!(harness.query_by_label(URL_DIALOG).is_none());
    assert!(harness.query_by_label("Edit sound – Clip.mp3").is_some());
    assert_eq!(setup.shared.calls(), [Call::LoadWaveform(clip)]);
    assert!(
        harness.query_by_label(&tile("Applause")).is_some(),
        "SFX is shown"
    );
    assert!(harness.query_by_label(&tile("Bruh")).is_none());
    assert_eq!(cancelled_downloads(&setup), 0);
}

#[test]
fn a_failed_download_shows_why_and_can_be_retried() {
    let setup = Setup::new();
    let mut harness = setup.start();
    enter_url(&mut harness, "https://example.com/gone");
    harness.get_by_label(DOWNLOAD).click();
    harness.run_steps(STEPS);

    let failed = DownloadEvent::Failed("HTTP Error 404: File not found".to_owned());
    report_download(&setup, &mut harness, failed);
    assert!(
        harness
            .query_by_label("Failed: HTTP Error 404: File not found")
            .is_some()
    );
    harness.get_by_label("⟳ Retry").click();
    harness.run_steps(STEPS);

    let download = ("https://example.com/gone".to_owned(), setup.folder("Memes"));
    assert_eq!(
        downloads(&setup),
        [download.clone(), download],
        "into the current tab by default"
    );
    assert!(harness.query_by_label_contains("Failed:").is_none());
    assert!(harness.query_by_label_contains("Starting…").is_some());
}

#[test]
fn closing_the_dialog_cancels_the_download() {
    let setup = Setup::new();
    let mut harness = setup.start();
    enter_url(&mut harness, "https://example.com/long");
    harness.get_by_label(DOWNLOAD).click();
    harness.run_steps(STEPS);

    harness.get_by_label("Cancel").click();
    harness.run_steps(STEPS);

    assert!(harness.query_by_label(URL_DIALOG).is_none());
    assert_eq!(cancelled_downloads(&setup), 1);

    enter_url(&mut harness, "https://example.com/long");
    harness.get_by_label(DOWNLOAD).click();
    harness.run_steps(STEPS);
    // Kittest embeds dialogs in the main window, where its close request is the main viewport's.
    harness
        .input_mut()
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .events
        .push(ViewportEvent::Close);
    harness.run_steps(STEPS);

    assert!(harness.query_by_label(URL_DIALOG).is_none());
    assert_eq!(cancelled_downloads(&setup), 2);
}

#[test]
fn without_tabs_the_dialog_asks_for_a_folder_first() {
    let setup = Setup::new().with_config(|config| config.tabs.clear());
    let music = setup.folder("Music");
    fs::create_dir(&music).expect("mkdir");
    setup
        .shared
        .with(|world| world.folder_to_pick = Some(music.clone()));
    let mut harness = setup.start();

    harness.get_by_label(ADD_FROM_URL).click();
    harness.run_steps(STEPS);
    assert!(
        harness
            .query_by_label_contains("Add a folder as a tab first")
            .is_some()
    );
    assert!(harness.query_by_label(URL_FIELD).is_none());

    harness.get_by_label("➕ Add folder").click();
    harness.run_steps(STEPS);
    type_url(&mut harness, "https://example.com/a");
    harness.get_by_label(DOWNLOAD).click();
    harness.run_steps(STEPS);

    assert_eq!(
        downloads(&setup),
        [("https://example.com/a".to_owned(), music)]
    );
}

// Rename dialog, tile layout, colors and order.

const RENAME: &str = "Rename sound";

#[test]
fn enter_saves_the_rename_dialog_and_escape_cancels_it() {
    let setup = Setup::new();
    let mut harness = setup.start();

    tile_menu(&mut harness, "Airhorn", "Rename…");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label("Name:").type_text("Not saved");
    harness.run();
    harness.key_press(Key::Escape);
    harness.run();
    assert!(harness.query_by_label(RENAME).is_none());
    assert_eq!(saved_airhorn(&setup).name, None);
    assert!(harness.query_by_label(&tile("Airhorn")).is_some());

    tile_menu(&mut harness, "Airhorn", "Rename…");
    assert_eq!(
        harness.get_by_label("Name:").value().as_deref(),
        Some("Airhorn")
    );
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label("Name:").type_text("Horn of doom");
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();

    assert!(harness.query_by_label(RENAME).is_none());
    assert_eq!(saved_airhorn(&setup).name.as_deref(), Some("Horn of doom"));
    assert!(harness.query_by_label(&tile("Horn of doom")).is_some());
}

#[test]
fn deleting_the_clip_being_renamed_closes_the_rename_dialog() {
    // Four more clips of Airhorn: "C" and "D" are right of the rename window (kittest embeds it in
    // the main window, over the first tiles).
    let setup = Setup::new().with_config(|config| {
        let airhorn = config.tabs[0].sounds.get_mut("Airhorn.wav");
        let airhorn = airhorn.expect("configured");
        for (id, name) in (1..).zip(["A", "B", "C", "D"]) {
            airhorn.clips.push(ClipConfig {
                id,
                config: SoundConfig {
                    name: Some(name.to_owned()),
                    ..SoundConfig::default()
                },
            });
        }
    });
    let mut harness = setup.start();

    tile_menu(&mut harness, "D", "Rename…");
    tile_menu(&mut harness, "C", "Rename…");
    assert_eq!(
        harness.get_by_label("Name:").value().as_deref(),
        Some("C"),
        "replaces the one for D"
    );
    tile_menu(&mut harness, "C", "Delete clip");

    assert!(harness.query_by_label(RENAME).is_none());
}

/// Long real-world names (one with an emoji) next to short ones.
const LONG_NAMES: [&str; 4] = [
    "Scheiße die Bullen - Haben wir noch Pepps?",
    "HUH❓ Cat meme - Sound Button",
    "Der is bisschen ein Otto geworden",
    "Parkour",
];

fn setup_with_long_names() -> Setup {
    let setup = Setup::new();
    for name in LONG_NAMES {
        fs::write(setup.folder("Memes").join(format!("{name}.wav")), b"").expect("write");
    }
    setup
}

#[test]
fn tiles_all_have_the_same_size_in_equal_columns() {
    let setup = setup_with_long_names();
    let harness = setup.start();

    let names = ["Airhorn", "Bruh"].into_iter().chain(LONG_NAMES);
    let rects: Vec<_> = names
        .map(|name| harness.get_by_label(&tile(name)).rect())
        .collect();
    for rect in &rects {
        assert_eq!(rect.size(), Vec2::new(180.0, 90.0), "{rects:?}");
    }
    let mut lefts: Vec<f32> = rects.iter().map(|rect| rect.left()).collect();
    lefts.sort_by(f32::total_cmp);
    lefts.dedup();
    for pair in lefts.windows(2) {
        assert_eq!(pair[1] - pair[0], 188.0, "{lefts:?}");
    }
}

#[test]
fn the_tile_menu_colors_a_tile_and_it_is_saved() {
    let setup = Setup::new();
    let mut harness = setup.start();

    tile_menu(&mut harness, "Airhorn", "Red");
    assert_eq!(
        saved_airhorn(&setup).color,
        Some(TileColor::Preset(Preset::Red))
    );
    let text = fs::read_to_string(setup.store.path()).expect("read");
    assert!(text.contains("color = \"red\""), "{text}");

    tile_menu(&mut harness, "Airhorn", "None");
    assert_eq!(saved_airhorn(&setup).color, None);
}

#[test]
fn the_editor_color_section_is_saved_with_the_sound() {
    let setup = Setup::new();
    let mut harness = setup.start();

    open_editor(&mut harness);
    harness.get_by_label("Teal").click();
    harness.run();
    assert_eq!(saved_airhorn(&setup).color, None, "not before saving");
    click_editor_button(&mut harness, "Save");

    assert_eq!(
        saved_airhorn(&setup).color,
        Some(TileColor::Preset(Preset::Teal))
    );
}

/// Drags the tile `from` onto the left (or right, `after`) half of the tile `to` and drops it.
fn drag_tile(harness: &mut Harness<'_, AppWindows>, from: &str, to: &str, after: bool) {
    let start = harness.get_by_label_contains(&tile(from)).rect().center();
    let target = harness.get_by_label_contains(&tile(to)).rect().center();
    let end = target + Vec2::new(if after { 40.0 } else { -40.0 }, 0.0);
    harness.hover_at(start);
    harness.run();
    harness.drag_at(start);
    harness.run();
    harness.hover_at(start + Vec2::new(20.0, 0.0));
    harness.run();
    harness.hover_at(end);
    harness.run();
    // Not `drop_at`: the pointer stays where it was released, like a real mouse.
    harness.event(eframe::egui::Event::PointerButton {
        pos: end,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run();
}

fn left_of(harness: &Harness<'_, AppWindows>, name: &str) -> f32 {
    harness.get_by_label_contains(&tile(name)).rect().left()
}

#[test]
fn dragging_a_tile_onto_another_reorders_the_tab_and_a_click_still_plays() {
    let setup = Setup::new();
    let mut harness = setup.start();
    setup.shared.clear_calls();

    drag_tile(&mut harness, "Bruh", "Airhorn", false);

    assert!(left_of(&harness, "Bruh") < left_of(&harness, "Airhorn"));
    assert_eq!(
        setup.config().tabs[0].order,
        [ClipKey::new("Bruh.wav", 0), ClipKey::new("Airhorn.wav", 0)]
    );
    assert!(
        setup.shared.calls().is_empty(),
        "dragging doesn't play: {:?}",
        setup.shared.calls()
    );

    drag_tile(&mut harness, "Bruh", "Airhorn", true);
    assert!(left_of(&harness, "Airhorn") < left_of(&harness, "Bruh"));

    assert_eq!(
        click_tile(&setup, &mut harness, "Bruh"),
        [Call::Play(PlayRequest {
            path: setup.folder("Memes").join("Bruh.wav"),
            clip: 0,
            trim: None,
            volume: 1.0,
            destination: Destination::Everywhere,
        })]
    );
}

#[test]
fn sort_by_name_forgets_the_dragged_order() {
    let setup = Setup::new().with_config(|config| {
        config.tabs[0].order = vec![ClipKey::new("Bruh.wav", 0), ClipKey::new("Gone.wav", 0)];
    });
    let mut harness = setup.start();
    assert!(left_of(&harness, "Bruh") < left_of(&harness, "Airhorn"));

    tile_menu(&mut harness, "Airhorn", "Sort by name");

    assert!(left_of(&harness, "Airhorn") < left_of(&harness, "Bruh"));
    assert!(setup.config().tabs[0].order.is_empty());
}

#[test]
fn tiles_cannot_be_dragged_while_searching() {
    let setup = Setup::new();
    let mut harness = setup.start();
    let search = harness.get_by_role(Role::TextInput);
    search.click();
    search.type_text("r");
    harness.run();

    drag_tile(&mut harness, "Bruh", "Airhorn", false);

    assert!(left_of(&harness, "Airhorn") < left_of(&harness, "Bruh"));
    assert!(setup.config().tabs[0].order.is_empty());
}
