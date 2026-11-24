//! End-to-end tests of the UI with `egui_kittest`: the real [`App`] with a fake audio backend,
//! folder watcher and folder picker, temp folders and a temp config. Headless, no PipeWire.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::accesskit::Role;
use eframe::egui::{Pos2, ViewportEvent, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundboars::app::{App, AudioBackend, FolderPicker, Services};
use soundboars::audio::{
    Destination, EngineEvent, Peak, PlayRequest, Playing, RecordingApp, Retrigger, Trim, Waveform,
};
use soundboars::config::{Config, ConfigStore, SavedTarget, SoundConfig, TabConfig, Theme};
use soundboars::library::watch::{FolderEvent, FolderWatcher};
use soundboars::midi::{
    Connection, HOTPLUG_POLL, MidiBinding, MidiControl, MidiDevice, MidiError, MidiInput, MidiPort,
    MidiService, OnMessage,
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
        let config = Config {
            tabs: vec![memes_tab, TabConfig::for_folder(sfx)],
            ..Config::default()
        };
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

    fn start(&self) -> Harness<'static, App> {
        self.start_with_audio(Ok(Box::new(FakeAudio(self.shared.clone()))))
    }

    fn start_with_audio(
        &self,
        audio: Result<Box<dyn AudioBackend>, String>,
    ) -> Harness<'static, App> {
        let mut services = Services {
            audio,
            watcher: Box::new(FakeWatcher(self.shared.clone())),
            folder_picker: Box::new(FakePicker(self.shared.clone())),
            config: self.store.clone(),
            midi: None,
        };
        let midi = Box::new(self.midi.clone());
        let mut harness = Harness::builder()
            .with_size([1000.0, 700.0])
            .build_eframe(|cc| {
                let ctx = cc.egui_ctx.clone();
                let wake = move || ctx.request_repaint();
                services.midi = Some(MidiService::start(midi, HOTPLUG_POLL, wake).expect("thread"));
                App::new(services)
            });
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
    // The tile's subtitle may go on with the MIDI binding.
    harness
        .get_by_label_contains(&tile("Airhorn"))
        .click_secondary();
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

/// Sends a message from the MPD218 once the app listens to it, and lets the app handle it.
fn send_midi(setup: &Setup, harness: &mut Harness<'_, App>, message: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let sent = setup.midi.with(|world| {
            let listener = world.listeners.iter_mut().find(|(id, _)| id == MPD_ID);
            listener
                .map(|(_, on_message)| on_message(message))
                .is_some()
        });
        if sent {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the app never connected to the MPD218"
        );
        std::thread::sleep(Duration::from_millis(10));
        harness.run();
    }
    harness.run();
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

    assert!(plays(&setup).is_empty(), "learning doesn't play");
    assert!(harness.query_by_label("Cancel learning").is_none());
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
    assert!(setup.shared.calls().is_empty());
    send_midi(&setup, &mut harness, &[0x90, 36, 100]);

    assert_eq!(
        setup.shared.calls(),
        [Call::Play(PlayRequest {
            path: airhorn(&setup),
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

    assert!(harness.query_by_label("⚠ Also triggers: Horn").is_some());
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
    assert_eq!(setup.shared.calls(), [Call::StopAll]);

    harness.get_by_label("Clear").click();
    harness.run();
    assert_eq!(setup.config().settings.midi_stop_all, None);
}
