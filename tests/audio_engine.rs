//! End-to-end tests of [`AudioEngine`] against a real PipeWire daemon: sound files must arrive in a
//! fake voice app (`pw-record`) and on the default (null) sink. Runs only with
//! `SOUNDBOARS_REQUIRE_PIPEWIRE=1` (see `common::pipewire_required`).

mod common;

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use common::{
    FAKE_BINARY, HEADLESS_SINK, Recorder, TIMEOUT, assert_tone, fixture, peak, temp_wav,
    wait_for_links,
};
use soundboars::audio::{
    AudioEngine, Destination, EngineEvent, INJECT_NODE_NAME, PlayRequest, RecordingApp, waveform,
};

/// The tests watch the whole graph (links, the sink's output), so they must not run concurrently.
static GRAPH: Mutex<()> = Mutex::new(());

/// The fixtures are 0.5 s of 440 Hz at half amplitude.
const TONE_HZ: f32 = 440.0;
const MIN_TONE_SECONDS: f32 = 0.4;

#[test]
fn plays_a_file_into_the_app_and_on_the_default_sink() {
    if !common::pipewire_required() {
        return;
    }
    let _graph = GRAPH.lock().unwrap_or_else(PoisonError::into_inner);
    let engine = start_engine();
    let recorders = Recorders::start_and_link(&engine, "plays");

    play_and_wait(&engine, request("tone.flac"));

    let (app, local) = recorders.stop_and_read();
    assert_tone("app", &app, TONE_HZ, MIN_TONE_SECONDS);
    assert_tone("local", &local, TONE_HZ, MIN_TONE_SECONDS);
    // A stereo target gets the sound at its own level (the limiter leaves 0.5 alone).
    let app_peak = peak(&app);
    assert!((0.4..0.6).contains(&app_peak), "app peak {app_peak}");
}

#[test]
fn call_volume_zero_silences_the_app_but_not_local_playback() {
    if !common::pipewire_required() {
        return;
    }
    let _graph = GRAPH.lock().unwrap_or_else(PoisonError::into_inner);
    let engine = start_engine();
    engine.set_call_volume(0.0);
    let recorders = Recorders::start_and_link(&engine, "volume");

    play_and_wait(&engine, request("tone.mp3"));

    let (app, local) = recorders.stop_and_read();
    assert!(peak(&app) < 0.01, "app peak {}", peak(&app));
    assert_tone("local", &local, TONE_HZ, MIN_TONE_SECONDS);
}

#[test]
fn sounds_finish_without_a_linked_app() {
    if !common::pipewire_required() {
        return;
    }
    let _graph = GRAPH.lock().unwrap_or_else(PoisonError::into_inner);
    let engine = start_engine();
    engine.set_local_enabled(false);
    // The inject stream isn't linked anywhere, yet it must keep rendering so the sound finishes.
    play_and_wait(&engine, request("tone.ogg"));
    assert!(engine.playing().is_empty());
}

#[test]
fn previews_play_locally_but_never_into_the_app() {
    if !common::pipewire_required() {
        return;
    }
    let _graph = GRAPH.lock().unwrap_or_else(PoisonError::into_inner);
    let engine = start_engine();
    // Previews ignore whether local playback is enabled.
    engine.set_local_enabled(false);
    let recorders = Recorders::start_and_link(&engine, "preview");

    play_and_wait(
        &engine,
        PlayRequest {
            destination: Destination::Preview,
            ..request("tone.flac")
        },
    );

    let (app, local) = recorders.stop_and_read();
    assert!(peak(&app) < 0.01, "app peak {}", peak(&app));
    assert_tone("local", &local, TONE_HZ, MIN_TONE_SECONDS);
}

#[test]
fn waveforms_are_computed_from_the_file() {
    if !common::pipewire_required() {
        return;
    }
    let _graph = GRAPH.lock().unwrap_or_else(PoisonError::into_inner);
    let engine = start_engine();
    engine.load_waveform(&fixture("tone.flac"));
    let event = wait_for(&engine, |event| {
        matches!(event, EngineEvent::Waveform { .. })
    });
    let EngineEvent::Waveform { waveform, .. } = event else {
        unreachable!("wait_for returned a Waveform event");
    };
    let waveform = waveform.expect("decodes");
    assert_eq!(waveform.peaks.len(), waveform::BUCKETS);
    assert!((0.45..0.55).contains(&waveform.duration.as_secs_f32()));
    let loudest = waveform
        .peaks
        .iter()
        .fold(0.0f32, |loudest, peak| loudest.max(peak.max));
    assert!((0.4..0.6).contains(&loudest), "peak {loudest}");
}

#[test]
fn undecodable_files_are_reported() {
    if !common::pipewire_required() {
        return;
    }
    let _graph = GRAPH.lock().unwrap_or_else(PoisonError::into_inner);
    let engine = start_engine();
    engine.play(request("not-audio.mp3"));
    wait_for(
        &engine,
        |event| matches!(event, EngineEvent::DecodeFailed { path, .. } if path.ends_with("not-audio.mp3")),
    );
}

fn start_engine() -> AudioEngine {
    AudioEngine::start().expect("PipeWire must be reachable when SOUNDBOARS_REQUIRE_PIPEWIRE=1")
}

fn request(name: &str) -> PlayRequest {
    PlayRequest {
        path: fixture(name),
        clip: 0,
        trim: None,
        volume: 1.0,
        destination: Destination::Everywhere,
    }
}

/// Plays a sound and waits until it has started and finished.
fn play_and_wait(engine: &AudioEngine, request: PlayRequest) {
    engine.play(request);
    let started = wait_for(engine, |event| matches!(event, EngineEvent::Started(_)));
    let EngineEvent::Started(playing) = started else {
        unreachable!("wait_for returned a Started event");
    };
    assert_eq!(engine.playing(), std::slice::from_ref(&playing));
    wait_for(
        engine,
        |event| matches!(event, EngineEvent::Finished { voice, .. } if *voice == playing.voice),
    );
}

/// The fake voice app (linked to the engine) and a recorder of the default sink.
struct Recorders {
    app: Recorder,
    local: Recorder,
}

impl Recorders {
    fn start_and_link(engine: &AudioEngine, name: &str) -> Self {
        let app = Recorder::fake_voice_app(&temp_wav(&format!("{name}-app")));
        let local = Recorder::sink_monitor(&temp_wav(&format!("{name}-local")));
        let target = wait_for_app(engine);
        engine.link_to(target.node_id);
        wait_for_links(INJECT_NODE_NAME, |links| links.len() == 2);
        wait_for_links(HEADLESS_SINK, |links| links.len() >= 2);
        Self { app, local }
    }

    /// Lets the tail of the sound reach the recorders, then stops them.
    fn stop_and_read(self) -> (Vec<f32>, Vec<f32>) {
        std::thread::sleep(Duration::from_millis(300));
        (self.app.stop_and_read(), self.local.stop_and_read())
    }
}

fn wait_for_app(engine: &AudioEngine) -> RecordingApp {
    let event = wait_for(
        engine,
        |event| matches!(event, EngineEvent::RecordingApps(apps) if find_fake_app(apps).is_some()),
    );
    match event {
        EngineEvent::RecordingApps(apps) => find_fake_app(&apps).cloned(),
        _ => None,
    }
    .expect("wait_for returned the app list with the fake app")
}

fn find_fake_app(apps: &[RecordingApp]) -> Option<&RecordingApp> {
    apps.iter()
        .find(|app| app.process_binary.as_deref() == Some(FAKE_BINARY) && app.ports.len() == 2)
}

/// Waits for the first event matching `wanted`; fails on errors and on timeout.
fn wait_for(engine: &AudioEngine, wanted: impl Fn(&EngineEvent) -> bool) -> EngineEvent {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = engine
            .events()
            .recv_timeout(remaining)
            .expect("the expected event arrives in time");
        if wanted(&event) {
            return event;
        }
        if let EngineEvent::Error(message) = &event {
            panic!("engine error: {message}");
        }
    }
}
