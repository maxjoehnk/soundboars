//! Smoke tests of the real `soundboars` binary: it opens its window on the container's Xvfb, starts
//! the audio engine against the headless PipeWire, and either relinks to the remembered target app
//! from its config or (by default) creates the virtual microphone. Runs only with
//! `SOUNDBOARS_REQUIRE_PIPEWIRE=1` (see `common::pipewire_required`), i.e. inside the container,
//! never on the desktop.

mod common;

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use common::{FAKE_BINARY, Recorder, TIMEOUT, temp_wav, wait_for_links, wait_for_node};
use soundboars::audio::{INJECT_NODE_NAME, VIRTUAL_MIC_NODE_NAME};

/// Both apps would have a `soundboars-inject` node, so the tests must not run concurrently.
static GRAPH: Mutex<()> = Mutex::new(());

/// Kills the app when the test ends, also when it fails.
struct RunningApp(Child);

impl RunningApp {
    fn start(config: &Path, log_path: &Path) -> Self {
        let display = std::env::var("DISPLAY").expect("test-headless.sh sets DISPLAY to its Xvfb");
        let log = fs::File::create(log_path).expect("log file");
        Self(
            Command::new(env!("CARGO_BIN_EXE_soundboars"))
                .env("SOUNDBOARS_CONFIG", config)
                .env("DISPLAY", display)
                .env_remove("WAYLAND_DISPLAY")
                .env("RUST_LOG", "info")
                .stdout(Stdio::null())
                .stderr(log)
                .spawn()
                .expect("start soundboars"),
        )
    }

    /// Asserts that the app still runs and logged no panic.
    fn assert_alive(&mut self, log_path: &Path) {
        let status = self.0.try_wait().expect("query the app");
        let log = fs::read_to_string(log_path).unwrap_or_default();
        assert!(status.is_none(), "the app exited ({status:?}):\n{log}");
        assert!(!log.contains("panicked"), "{log}");
    }
}

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
    let _graph = GRAPH.lock().unwrap_or_else(PoisonError::into_inner);
    let dir = tempfile::tempdir().expect("temp dir");
    let config = write_config(dir.path(), "injection = \"app\"\n");
    let log_path = dir.path().join("app.log");

    let recorder = Recorder::fake_voice_app(&temp_wav("smoke"));
    let mut app = RunningApp::start(&config, &log_path);

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

    app.assert_alive(&log_path);
    drop(app);
    recorder.stop_and_read();
}

#[test]
fn the_app_creates_the_virtual_mic_by_default() {
    if !common::pipewire_required() {
        return;
    }
    let _graph = GRAPH.lock().unwrap_or_else(PoisonError::into_inner);
    let dir = tempfile::tempdir().expect("temp dir");
    let config = write_config(dir.path(), "");
    let log_path = dir.path().join("app.log");

    let mut app = RunningApp::start(&config, &log_path);

    wait_for_node(VIRTUAL_MIC_NODE_NAME);
    let links = wait_for_links(INJECT_NODE_NAME, |links| links.len() == 2);
    assert!(
        links
            .iter()
            .all(|(_, input)| input.starts_with(&format!("{VIRTUAL_MIC_NODE_NAME}:"))),
        "{links:?}"
    );
    wait_for_window("SoundBoars");
    app.assert_alive(&log_path);
}

/// A config with one tab and the fake voice app as the remembered target, after the top-level
/// keys in `head`.
fn write_config(dir: &Path, head: &str) -> std::path::PathBuf {
    let sounds = dir.join("Sounds");
    fs::create_dir(&sounds).expect("mkdir");
    fs::copy(common::fixture("tone.flac"), sounds.join("Tone.flac")).expect("copy fixture");
    let config = dir.join("config.toml");
    let text = format!(
        "{head}[target]\napp_name = \"FakeVoiceApp\"\nprocess_binary = \"{FAKE_BINARY}\"\n\n\
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
