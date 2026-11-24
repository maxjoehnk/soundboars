//! Smoke test of the real `soundboars` binary: it opens its window on the container's Xvfb, starts
//! the audio engine against the headless PipeWire, and relinks to the remembered target app from
//! its config. Runs only with `SOUNDBOARS_REQUIRE_PIPEWIRE=1` (see `common::pipewire_required`),
//! i.e. inside the container, never on the desktop.

mod common;

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{FAKE_BINARY, Recorder, TIMEOUT, temp_wav, wait_for_links};
use soundboars::audio::INJECT_NODE_NAME;

/// Kills the app when the test ends, also when it fails.
struct RunningApp(Child);

impl Drop for RunningApp {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn the_app_starts_and_relinks_to_the_remembered_target() {
    if !common::pipewire_required() {
        return;
    }
    let display = std::env::var("DISPLAY").expect("test-headless.sh sets DISPLAY to its Xvfb");
    let dir = tempfile::tempdir().expect("temp dir");
    let config = write_config(dir.path());
    let log_path = dir.path().join("app.log");
    let log = fs::File::create(&log_path).expect("log file");

    let recorder = Recorder::fake_voice_app(&temp_wav("smoke"));
    let mut app = RunningApp(
        Command::new(env!("CARGO_BIN_EXE_soundboars"))
            .env("SOUNDBOARS_CONFIG", &config)
            .env("DISPLAY", display)
            .env_remove("WAYLAND_DISPLAY")
            .env("RUST_LOG", "info")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("start soundboars"),
    );

    // Linking happens in the UI's frame loop, so this also shows that the window renders.
    let links = wait_for_links(INJECT_NODE_NAME, |links| links.len() == 2);
    assert!(
        links
            .iter()
            .all(|(_, input)| input.starts_with("pw-record:")),
        "{links:?}"
    );
    wait_for_window("SoundBoars");
    std::thread::sleep(Duration::from_secs(2));

    let status = app.0.try_wait().expect("query the app");
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    assert!(status.is_none(), "the app exited ({status:?}):\n{log}");
    assert!(!log.contains("panicked"), "{log}");
    drop(app);
    recorder.stop_and_read();
}

/// A config with one tab and the fake voice app as the remembered target.
fn write_config(dir: &Path) -> std::path::PathBuf {
    let sounds = dir.join("Sounds");
    fs::create_dir(&sounds).expect("mkdir");
    fs::copy(common::fixture("tone.flac"), sounds.join("Tone.flac")).expect("copy fixture");
    let config = dir.join("config.toml");
    let text = format!(
        "[target]\napp_name = \"FakeVoiceApp\"\nprocess_binary = \"{FAKE_BINARY}\"\n\n\
         [[tabs]]\nname = \"Sounds\"\npath = \"{}\"\n",
        sounds.display()
    );
    fs::write(&config, text).expect("write config");
    config
}

fn wait_for_window(title: &str) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let found = Command::new("xdotool")
            .args(["search", "--name", &format!("^{title}$")])
            .output()
            .expect("xdotool is installed");
        if found.status.success() && !found.stdout.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "no window titled {title}");
        std::thread::sleep(Duration::from_millis(100));
    }
}
