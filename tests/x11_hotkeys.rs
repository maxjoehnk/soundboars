//! The X11 hotkey backend on the container's Xvfb: grabs key combinations and sees them pressed
//! with `xdotool` (Xvfb's keymap has no F13 and up). Runs only with
//! `SOUNDBOARS_REQUIRE_PIPEWIRE=1` (`scripts/test-headless.sh`), so it never grabs keys on the
//! desktop's display.

mod common;

use std::process::Command;
use std::sync::mpsc;

use soundboars::hotkeys::x11::X11Backend;
use soundboars::hotkeys::{Binding, EventSink, HotkeyBackend, HotkeyEvent};

fn binding(id: &str, hotkey: Option<&str>) -> Binding {
    Binding {
        id: id.to_owned(),
        description: id.to_owned(),
        hotkey: hotkey.map(|hotkey| hotkey.parse().expect("valid hotkey")),
    }
}

fn press(keys: &str) {
    let status = Command::new("xdotool")
        .args(["key", keys])
        .status()
        .expect("xdotool runs");
    assert!(status.success(), "xdotool key {keys}");
}

fn next_event(events: &mpsc::Receiver<HotkeyEvent>) -> HotkeyEvent {
    events
        .recv_timeout(common::TIMEOUT)
        .expect("a hotkey event")
}

#[test]
fn grabbed_keys_are_reported_as_presses_and_releases() {
    if !common::pipewire_required() {
        return;
    }
    let (sender, events) = mpsc::channel();
    let mut backend = X11Backend::start(EventSink::new(sender, || {})).expect("X11 backend");
    backend.register(&[
        binding("stop-all", None),
        binding("play-airhorn", Some("Ctrl+Shift+F12")),
        binding("play-bruh", Some("Alt+B")),
    ]);

    press("ctrl+shift+F12");
    assert_eq!(
        next_event(&events),
        HotkeyEvent::Pressed("play-airhorn".into())
    );
    assert_eq!(
        next_event(&events),
        HotkeyEvent::Released("play-airhorn".into())
    );
    press("alt+b");
    assert_eq!(
        next_event(&events),
        HotkeyEvent::Pressed("play-bruh".into())
    );
    assert_eq!(
        next_event(&events),
        HotkeyEvent::Released("play-bruh".into())
    );

    // Registering again replaces the grabs.
    backend.register(&[binding("play-bruh", Some("Ctrl+Alt+B"))]);
    press("alt+b");
    press("ctrl+alt+b");
    assert_eq!(
        next_event(&events),
        HotkeyEvent::Pressed("play-bruh".into())
    );
    assert_eq!(
        next_event(&events),
        HotkeyEvent::Released("play-bruh".into())
    );

    // The same keys twice: the second binding is reported, the first still works.
    backend.register(&[
        binding("play-airhorn", Some("Ctrl+F11")),
        binding("play-bruh", Some("Ctrl+F11")),
    ]);
    match next_event(&events) {
        HotkeyEvent::Error(error) => assert!(error.contains("Ctrl+F11 is used twice"), "{error}"),
        event => panic!("expected an error, got {event:?}"),
    }
    press("ctrl+F11");
    assert_eq!(
        next_event(&events),
        HotkeyEvent::Pressed("play-airhorn".into())
    );
    assert_eq!(
        next_event(&events),
        HotkeyEvent::Released("play-airhorn".into())
    );

    drop(backend);
    press("ctrl+F11");
    assert!(
        events
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_err(),
        "no presses once dropped"
    );
}
