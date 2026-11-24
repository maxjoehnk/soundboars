//! End-to-end tests of the UI with `egui_kittest`: the real [`App`] with a fake audio backend,
//! folder watcher and folder picker, temp folders and a temp config. Headless, no PipeWire.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundboars::app::{App, AudioBackend, FolderPicker, Services};
use soundboars::audio::{EngineEvent, PlayRequest, Playing, RecordingApp, Retrigger, Trim};
use soundboars::config::{Config, ConfigStore, SavedTarget, SoundConfig, TabConfig, Theme};
use soundboars::library::watch::{FolderEvent, FolderWatcher};

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Play(PlayRequest),
    StopAll,
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
            },
        );
        let store = ConfigStore::new(dir.path().join("config/config.toml"));
        let config = Config {
            tabs: vec![memes_tab, TabConfig::for_folder(sfx)],
            ..Config::default()
        };
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
fn tiles_show_the_length_once_a_sound_played() {
    let setup = Setup::new();
    let mut harness = setup.start();

    let started = Playing {
        voice: 3,
        path: setup.folder("Memes").join("Bruh.wav"),
        started_at: Instant::now(),
        duration: Duration::from_millis(2500),
    };
    setup
        .shared
        .with(|world| world.engine_events.push(EngineEvent::Started(started)));
    harness.run();

    assert!(harness.query_by_label("Bruh\n0:02.50").is_some());
    assert!(harness.query_by_label(&tile("Airhorn")).is_some());
}
