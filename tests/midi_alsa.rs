//! The MIDI service against the real ALSA sequencer: a virtual output port made with `midir` plays
//! the controller. Runs only inside the test container (`scripts/test-headless.sh`) and only when
//! it has `/dev/snd/seq` (`scripts/in-container.sh` passes it through if the host has one), so a
//! plain `cargo test` never creates ports on the desktop's sequencer, and CI runners without the
//! device skip it.

use std::path::Path;
use std::time::{Duration, Instant};

use midir::os::unix::VirtualOutput;
use midir::{MidiOutput, MidiOutputConnection};
use soundboars::midi::{MidiControl, MidiDevice, MidiEvent, MidiService, MidiTrigger, MidirInput};

const TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

fn sequencer_available() -> bool {
    if std::env::var("SOUNDBOARS_REQUIRE_PIPEWIRE").as_deref() != Ok("1") {
        eprintln!("skipping: runs only in the test container (scripts/test-headless.sh)");
        return false;
    }
    if !Path::new("/dev/snd/seq").exists() {
        eprintln!("skipping: no ALSA sequencer (/dev/snd/seq) in this container");
        return false;
    }
    true
}

/// A virtual MIDI controller: an output port other clients can listen to.
fn controller(client: &str) -> MidiOutputConnection {
    MidiOutput::new(client)
        .expect("the sequencer opens")
        .create_virtual("out")
        .expect("a virtual port can be created")
}

/// Collects the service's events until one satisfies `done`; returns all of them.
fn events_until(service: &MidiService, done: impl Fn(&MidiEvent) -> bool) -> Vec<MidiEvent> {
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = Vec::new();
    loop {
        let events = service.poll();
        let finished = events.iter().any(&done);
        seen.extend(events);
        if finished {
            return seen;
        }
        assert!(Instant::now() < deadline, "only got {seen:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn triggers(events: &[MidiEvent]) -> Vec<MidiTrigger> {
    events
        .iter()
        .filter_map(|event| match event {
            MidiEvent::Trigger(trigger) => Some(trigger.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn triggers_arrive_from_a_port_that_comes_and_goes() {
    if !sequencer_available() {
        return;
    }
    let client = format!("soundboars-test-{}", std::process::id());
    let port = format!("{client}:out");
    let connected = MidiEvent::Connected(vec![port.clone()]);
    let mut out = controller(&client);
    let service =
        MidiService::start(Box::new(MidirInput::default()), POLL, || {}).expect("thread starts");
    // Only ever the test's own port: the sequencer may be the host's, with real devices on it.
    service.select(MidiDevice::Port(port.clone()));
    events_until(&service, |event| *event == connected);

    for message in [
        [0x80, 36, 0],
        [0x99, 36, 100],
        [0xB0, 20, 0],
        [0xB0, 20, 127],
    ] {
        out.send(&message).expect("send");
    }
    let events = events_until(
        &service,
        |event| matches!(event, MidiEvent::Trigger(trigger) if trigger.control == MidiControl::ControlChange(20)),
    );
    let trigger = |channel, control| MidiTrigger {
        port: port.clone(),
        channel,
        control,
    };
    assert_eq!(
        triggers(&events),
        [
            trigger(10, MidiControl::Note(36)),
            trigger(1, MidiControl::ControlChange(20))
        ]
    );

    // Unplugged…
    drop(out);
    let events = events_until(&service, |event| *event == MidiEvent::Connected(vec![]));
    let still_listed =
        |event: &MidiEvent| matches!(event, MidiEvent::Ports(ports) if ports.contains(&port));
    assert!(!events.iter().any(still_listed), "{events:?}");

    // …and plugged in again.
    let mut out = controller(&client);
    events_until(&service, |event| *event == connected);
    out.send(&[0x90, 40, 1]).expect("send");
    let events = events_until(&service, |event| matches!(event, MidiEvent::Trigger(_)));
    assert_eq!(triggers(&events), [trigger(1, MidiControl::Note(40))]);
}
