//! End-to-end test of injection against a real PipeWire daemon: a fake voice app (`pw-record`)
//! must receive our 440 Hz sine, and our links must go away again. Runs only with
//! `SOUNDBOARS_REQUIRE_PIPEWIRE=1` (see `common::pipewire_required`).

mod common;

use std::sync::mpsc;
use std::time::Duration;

use common::{
    FAKE_BINARY, Recorder, assert_tone, same_channel, temp_wav, wait_for_app, wait_for_links,
};
use soundboars::audio::sine::Sine;
use soundboars::audio::{INJECT_NODE_NAME, PipeWireService, Renders};

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
