//! End-to-end test of injection against a real PipeWire daemon: a fake voice app (`pw-record`)
//! must receive our 440 Hz sine, and our links must go away again. Runs only with
//! `SOUNDBOARS_REQUIRE_PIPEWIRE=1` (see `common::pipewire_required`).

mod common;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::{FAKE_BINARY, Recorder, TIMEOUT, assert_tone, same_channel, temp_wav, wait_for_links};
use soundboars::audio::sine::Sine;
use soundboars::audio::{Event, INJECT_NODE_NAME, PipeWireService, RecordingApp, Renders};

#[test]
fn injects_a_sine_into_a_recording_app_and_unlinks_again() {
    if !common::pipewire_required() {
        return;
    }
    let mut sine = Sine::new(440.0, 0.5);
    let renders = Renders {
        inject: Box::new(move |frames: &mut [f32]| sine.render(frames)),
        local: Box::new(|frames: &mut [f32]| frames.fill(0.0)),
    };
    let (event_tx, events) = mpsc::channel();
    let service = PipeWireService::start(renders, move |event| {
        let _ = event_tx.send(event);
    })
    .expect("PipeWire must be reachable when SOUNDBOARS_REQUIRE_PIPEWIRE=1");
    let wav = temp_wav("inject");
    let recorder = Recorder::fake_voice_app(&wav);

    let app = wait_for_app(&events, FAKE_BINARY);
    assert_eq!(app.app_name, "FakeVoiceApp");
    assert_eq!(app.ports.len(), 2);

    service.link_to(app.node_id);
    let links = wait_for_links(INJECT_NODE_NAME, |links| links.len() == 2);
    assert!(
        links.iter().all(|(out, input)| same_channel(out, input)),
        "{links:?}"
    );

    std::thread::sleep(Duration::from_secs(1));
    service.unlink();
    wait_for_links(INJECT_NODE_NAME, |links| links.is_empty());
    assert_tone("app", &recorder.stop_and_read(), 440.0, 0.5);

    // Dropping the service must take its links with it.
    let recorder = Recorder::fake_voice_app(&wav);
    let app = wait_for_app(&events, FAKE_BINARY);
    service.link_to(app.node_id);
    wait_for_links(INJECT_NODE_NAME, |links| links.len() == 2);
    drop(service);
    wait_for_links(INJECT_NODE_NAME, |links| links.is_empty());
    recorder.stop_and_read();
}

fn wait_for_app(events: &mpsc::Receiver<Event>, binary: &str) -> RecordingApp {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(remaining) {
            Ok(Event::RecordingApps(apps)) => {
                let found = apps.into_iter().find(|app| {
                    app.process_binary.as_deref() == Some(binary) && app.ports.len() == 2
                });
                if let Some(app) = found {
                    return app;
                }
            }
            Ok(Event::Error(message)) => panic!("PipeWire error: {message}"),
            Err(_) => panic!("{binary} did not show up as a recording app"),
        }
    }
}
