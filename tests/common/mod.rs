//! Helpers for the PipeWire integration tests: recording with `pw-record`, inspecting links with
//! `pw-link` and analysing what was recorded.
#![allow(
    dead_code,
    reason = "every test binary compiles this module but uses a different subset of it"
)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use soundboars::audio::{Event, RecordingApp, SAMPLE_RATE};

pub const FAKE_BINARY: &str = "fakevoice";
/// The null sink `scripts/test-headless.sh` sets up as the default sink.
pub const HEADLESS_SINK: &str = "headless-sink";
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Tests that create PipeWire nodes and links only run with `SOUNDBOARS_REQUIRE_PIPEWIRE=1`, as
/// `scripts/test-headless.sh` sets inside the container. Elsewhere they skip, so a plain
/// `cargo test` never touches the desktop's audio graph.
pub fn pipewire_required() -> bool {
    let required = std::env::var("SOUNDBOARS_REQUIRE_PIPEWIRE").as_deref() == Ok("1");
    if !required {
        eprintln!("skipping: set SOUNDBOARS_REQUIRE_PIPEWIRE=1 (scripts/test-headless.sh) to run");
    }
    required
}

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A WAV path in the temp dir that is unique to this process.
pub fn temp_wav(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("soundboars-{name}-{}.wav", std::process::id()))
}

/// `pw-record` writing stereo f32 at 48 kHz to a WAV file.
pub struct Recorder {
    child: Child,
    wav: PathBuf,
}

impl Recorder {
    /// `pw-record` dressed up as a voice app. It isn't linked to any microphone, so all it records
    /// is what we inject.
    pub fn fake_voice_app(wav: &Path) -> Self {
        Self::start(
            wav,
            &[],
            &format!(
                "{{ application.name = FakeVoiceApp application.process.binary = {FAKE_BINARY} \
                    node.autoconnect = false }}"
            ),
        )
    }

    /// Records the node named `target`, linked by the session manager like any recording app.
    pub fn recording(wav: &Path, target: &str) -> Self {
        Self::start(
            wav,
            &["--target", target],
            "{ application.name = TestRecorder }",
        )
    }

    /// Records what is played on the headless null sink (its monitor).
    pub fn sink_monitor(wav: &Path) -> Self {
        Self::start(
            wav,
            &["--target", HEADLESS_SINK],
            "{ application.name = SinkMonitor stream.capture.sink = true }",
        )
    }

    fn start(wav: &Path, args: &[&str], properties: &str) -> Self {
        let child = Command::new("pw-record")
            .args(args)
            .args(["--rate", "48000", "--channels", "2", "--format", "f32"])
            .args(["-P", properties])
            .arg(wav)
            .spawn()
            .expect("pw-record is installed");
        Self {
            child,
            wav: wav.to_owned(),
        }
    }

    /// Stops recording cleanly (SIGINT, so the WAV gets flushed) and returns the left channel.
    pub fn stop_and_read(self) -> Vec<f32> {
        self.stop_and_read_stereo().0
    }

    /// Like [`Self::stop_and_read`], but returns both channels.
    pub fn stop_and_read_stereo(mut self) -> (Vec<f32>, Vec<f32>) {
        let status = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .expect("kill is installed");
        assert!(status.success());
        self.child.wait().expect("pw-record exits");
        let bytes = std::fs::read(&self.wav).expect("pw-record wrote a WAV file");
        let _ = std::fs::remove_file(&self.wav);
        let samples: Vec<f32> = wav_f32_samples(&bytes).collect();
        let channel = |offset| samples.iter().skip(offset).step_by(2).copied().collect();
        (channel(0), channel(1))
    }
}

impl Drop for Recorder {
    /// Makes sure a failing test doesn't leave `pw-record` running.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Waits for the recording app whose process binary is `binary` to show up with two ports.
pub fn wait_for_app(events: &mpsc::Receiver<Event>, binary: &str) -> RecordingApp {
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
            Ok(Event::Microphone(_)) => {}
            Ok(Event::Error(message)) => panic!("PipeWire error: {message}"),
            Err(_) => panic!("{binary} did not show up as a recording app"),
        }
    }
}

/// The registry properties of the node named `name` as `pw-cli ls Node` prints them, or `None`
/// if there is no such node.
pub fn node_listing(name: &str) -> Option<String> {
    let output = Command::new("pw-cli")
        .args(["ls", "Node"])
        .output()
        .expect("pw-cli is installed");
    let listing = String::from_utf8_lossy(&output.stdout);
    let wanted = format!("node.name = \"{name}\"");
    listing
        .split("\tid ")
        .find(|block| block.lines().any(|line| line.trim() == wanted))
        .map(|block| format!("id {block}"))
}

/// The global id of the node named `name`, waiting for it to appear.
pub fn wait_for_node(name: &str) -> u32 {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(block) = node_listing(name) {
            let id = block
                .strip_prefix("id ")
                .and_then(|rest| rest.split(',').next())
                .and_then(|id| id.trim().parse().ok());
            return id.expect("pw-cli prints `id N, type …`");
        }
        assert!(Instant::now() < deadline, "node {name} did not appear");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits until there is no node named `name`.
pub fn wait_for_node_gone(name: &str) {
    let deadline = Instant::now() + TIMEOUT;
    while node_listing(name).is_some() {
        assert!(Instant::now() < deadline, "node {name} is still there");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The ports of all nodes in one direction as `node:port`, from `pw-link -i` / `pw-link -o`.
pub fn port_names(direction_flag: &str) -> Vec<String> {
    let output = Command::new("pw-link")
        .arg(direction_flag)
        .output()
        .expect("pw-link is installed");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().to_owned())
        .collect()
}

/// Polls `pw-link` until the links from `node`'s output ports satisfy `done`; returns them as
/// (output port, input port).
pub fn wait_for_links(
    node: &str,
    done: impl Fn(&[(String, String)]) -> bool,
) -> Vec<(String, String)> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let links = links_from(node);
        if done(&links) {
            return links;
        }
        assert!(Instant::now() < deadline, "unexpected links: {links:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The links from `node`'s output ports according to `pw-link -l`, which prints each output port
/// followed by its links as `  |-> node:port` lines.
pub fn links_from(node: &str) -> Vec<(String, String)> {
    let output = Command::new("pw-link")
        .arg("-l")
        .output()
        .expect("pw-link is installed");
    let listing = String::from_utf8_lossy(&output.stdout);
    let prefix = format!("{node}:");
    let mut links = Vec::new();
    let mut port = "";
    for line in listing.lines() {
        match line.trim_start().strip_prefix("|-> ") {
            Some(target) if port.starts_with(&prefix) => {
                links.push((port.to_owned(), target.to_owned()));
            }
            Some(_) => {}
            None => port = line.trim(),
        }
    }
    links
}

pub fn same_channel(output_port: &str, input_port: &str) -> bool {
    ["_FL", "_FR"]
        .iter()
        .any(|channel| output_port.ends_with(channel) && input_port.ends_with(channel))
}

/// The samples of the `data` chunk of a 32-bit float WAV file.
fn wav_f32_samples(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
    let mut offset = 12; // "RIFF", size, "WAVE"
    while &bytes[offset..offset + 4] != b"data" {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().expect("4 bytes"));
        offset += 8 + size as usize;
    }
    // The data chunk's size may be stale; everything after its header is audio.
    bytes[offset + 8..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
}

/// The samples from the first to the last one that isn't (near) silent.
pub fn loud_part(samples: &[f32]) -> &[f32] {
    let is_loud = |s: &f32| s.abs() > 0.05;
    match (
        samples.iter().position(is_loud),
        samples.iter().rposition(is_loud),
    ) {
        (Some(first), Some(last)) => &samples[first..=last],
        _ => &[],
    }
}

pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |peak, s| peak.max(s.abs()))
}

/// The amplitude of the `frequency` Hz component of `samples` (Goertzel algorithm).
pub fn tone_amplitude(samples: &[f32], frequency: f32) -> f32 {
    let omega = 2.0 * std::f32::consts::PI * frequency / SAMPLE_RATE as f32;
    let coefficient = 2.0 * omega.cos();
    let (mut previous, mut before) = (0.0_f32, 0.0_f32);
    for &sample in samples {
        let current = sample + coefficient * previous - before;
        before = previous;
        previous = current;
    }
    let power = previous * previous + before * before - coefficient * previous * before;
    2.0 * power.max(0.0).sqrt() / samples.len().max(1) as f32
}

pub fn dominant_frequency(samples: &[f32]) -> f32 {
    let rising = samples
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    rising as f32 * SAMPLE_RATE as f32 / samples.len() as f32
}

/// Asserts that `samples` contain a tone of about `frequency` Hz lasting at least `min_seconds`.
pub fn assert_tone(what: &str, samples: &[f32], frequency: f32, min_seconds: f32) {
    let tone = loud_part(samples);
    let seconds = tone.len() as f32 / SAMPLE_RATE as f32;
    assert!(seconds >= min_seconds, "{what}: only {seconds} s of tone");
    let measured = dominant_frequency(tone);
    eprintln!(
        "{what}: {seconds:.2} s of tone at {measured:.1} Hz, peak {:.2}",
        peak(tone)
    );
    assert!(
        (frequency - 10.0..frequency + 10.0).contains(&measured),
        "{what}: recorded {measured} Hz"
    );
}
