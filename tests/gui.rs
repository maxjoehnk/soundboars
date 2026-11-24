//! End-to-end tests of the UI with `egui_kittest`: the real [`App`] with a fake audio backend,
//! folder watcher and folder picker, temp folders and a temp config. Headless, no PipeWire.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::accesskit::Role;
use eframe::egui::{Key, Modifiers, Pos2, ViewportEvent, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundboars::app::{App, AudioBackend, FolderPicker, HotkeyBackends, Services};
use soundboars::audio::{
    Destination, EngineEvent, Peak, PlayRequest, Playing, RecordingApp, Retrigger, Trim, Waveform,
};
use soundboars::config::{Config, ConfigStore, SavedTarget, SoundConfig, TabConfig, Theme};
use soundboars::hotkeys::{
    BackendKind, BindHint, Binding, Capabilities, EventSink, Hotkey, HotkeyBackend, HotkeyEvent,
};
use soundboars::library::watch::{FolderEvent, FolderWatcher};

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
    Unlink,
}

/// What the fakes saw and what they report; shared between the test and the app.
#[derive(Default)]
struct World {
    calls: Vec<Call>,
    engine_events: Vec<EngineEvent>,
    playing: Vec<Playing>,
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

/// Two tabs: `Memes` (Airhorn, Bruh) and `SFX` (Applause, Horn). Airhorn has a volume and a trim.
struct Setup {
    dir: tempfile::TempDir,
    shared: Shared,
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
        Self {
            dir,
            shared: Shared::default(),
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

    fn start(&self) -> Harness<'static, App> {
        self.start_with_audio(Ok(Box::new(FakeAudio(self.shared.clone()))))
    }

    fn start_with_audio(
        &self,
        audio: Result<Box<dyn AudioBackend>, String>,
    ) -> Harness<'static, App> {
        let services = Services {
            audio,
            watcher: Box::new(FakeWatcher(self.shared.clone())),
            folder_picker: Box::new(FakePicker(self.shared.clone())),
            config: self.store.clone(),
            hotkeys: Box::new(FakeHotkeys(self.shared.clone())),
        };
        let mut harness = Harness::builder()
            .with_size([1000.0, 700.0])
            .build_eframe(|_cc| App::new(services));
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
fn target_picker_value(harness: &Harness<'_, App>) -> String {
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
fn stop_all_stops_and_playing_sounds_are_shown() {
    let setup = Setup::new();
    let path = setup.folder("Memes").join("Bruh.wav");
    setup.shared.with(|world| {
        world.playing.push(Playing {
            voice: 0,
            path,
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
fn open_editor(harness: &mut Harness<'_, App>) {
    // The tile, whatever its subtitle.
    harness.get_by_label_contains("Airhorn\n").click_secondary();
    harness.run();
    harness.get_by_label("✏ Edit…").click();
    harness.run();
}

/// Opens the editor for Airhorn and delivers its waveform.
fn open_editor_with_waveform(setup: &Setup, harness: &mut Harness<'_, App>) {
    open_editor(harness);
    deliver_waveform(setup, harness);
}

/// The engine reports Airhorn's waveform.
fn deliver_waveform(setup: &Setup, harness: &mut Harness<'_, App>) {
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
fn waveform_point(harness: &Harness<'_, App>, ms: u64) -> Pos2 {
    let rect = harness.get_by_label("Waveform").rect();
    let x = rect.left() + rect.width() * ms as f32 / AIRHORN_MS as f32;
    Pos2::new(x, rect.center().y)
}

fn drag(harness: &mut Harness<'_, App>, from_ms: u64, to_ms: u64) {
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

fn click_editor_button(harness: &mut Harness<'_, App>, label: &str) {
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
    assert!(harness.query_by_label("Airhorn.wav").is_some());
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

/// The backend reports a hotkey event.
fn hotkey_event(setup: &Setup, harness: &mut Harness<'_, App>, event: HotkeyEvent) {
    let events = setup
        .shared
        .with(|world| world.hotkey_events.clone())
        .expect("a hotkey backend runs");
    events.send(event);
    harness.run();
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

    click_editor_button(&mut harness, "Clear");
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

    assert_eq!(
        setup.shared.calls(),
        [
            Call::Play(PlayRequest {
                path: airhorn(&setup),
                trim: Some(Trim {
                    start_ms: 100,
                    end_ms: Some(900),
                }),
                volume: 0.5,
                destination: Destination::Everywhere,
            }),
            Call::StopAll,
        ]
    );
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

    harness.get_by_label("Clear").click();
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
