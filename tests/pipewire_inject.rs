//! End-to-end test of injection against a real PipeWire daemon: a fake voice app (`pw-record`)
//! must receive our 440 Hz sine, and our links must go away again.
//!
//! It creates nodes and links, so it only runs when `SOUNDBOARS_REQUIRE_PIPEWIRE=1` is set, as
//! `scripts/test-headless.sh` does inside the container with a private PipeWire. Elsewhere it
//! skips, so a plain `cargo test` never touches the desktop's audio graph.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use soundboars::audio::sine::Sine;
use soundboars::audio::{Event, INJECT_NODE_NAME, PipeWireService, RecordingApp, SAMPLE_RATE};

const FAKE_BINARY: &str = "fakevoice";
const TIMEOUT: Duration = Duration::from_secs(10);

#[test]
fn injects_a_sine_into_a_recording_app_and_unlinks_again() {
    if std::env::var("SOUNDBOARS_REQUIRE_PIPEWIRE").as_deref() != Ok("1") {
        eprintln!("skipping: set SOUNDBOARS_REQUIRE_PIPEWIRE=1 (scripts/test-headless.sh) to run");
        return;
    }
    let mut sine = Sine::new(440.0, 0.5);
    let service = PipeWireService::start(move |frames| sine.render(frames))
        .expect("PipeWire must be reachable when SOUNDBOARS_REQUIRE_PIPEWIRE=1");
    let wav = std::env::temp_dir().join(format!("soundboars-inject-{}.wav", std::process::id()));
    let recorder = FakeVoiceApp::start(&wav);

    let app = wait_for_app(&service, FAKE_BINARY);
    assert_eq!(app.app_name, "FakeVoiceApp");
    assert_eq!(app.ports.len(), 2);

    service.link_to(app.node_id);
    let links = wait_for_links(|links| links.len() == 2);
    assert!(
        links.iter().all(|(out, input)| same_channel(out, input)),
        "{links:?}"
    );

    std::thread::sleep(Duration::from_secs(1));
    service.unlink();
    wait_for_links(|links| links.is_empty());
    let samples = recorder.stop_and_read();

    let tone = loud_part(&samples);
    assert!(
        tone.len() > SAMPLE_RATE as usize / 2,
        "only {} loud samples",
        tone.len()
    );
    let frequency = dominant_frequency(tone);
    eprintln!(
        "recorded {} samples of tone at {frequency:.1} Hz",
        tone.len()
    );
    assert!(
        (430.0..450.0).contains(&frequency),
        "recorded {frequency} Hz"
    );

    // Dropping the service must take its links with it.
    let recorder = FakeVoiceApp::start(&wav);
    let app = wait_for_app(&service, FAKE_BINARY);
    service.link_to(app.node_id);
    wait_for_links(|links| links.len() == 2);
    drop(service);
    wait_for_links(|links| links.is_empty());
    recorder.stop_and_read();
    let _ = std::fs::remove_file(&wav);
}

/// `pw-record` dressed up as a voice app. It isn't linked to any microphone, so all it records is
/// what we inject.
struct FakeVoiceApp {
    child: Child,
    wav: PathBuf,
}

impl FakeVoiceApp {
    fn start(wav: &Path) -> Self {
        let child = Command::new("pw-record")
            .args(["--target", "0", "--rate", "48000", "--channels", "2"])
            .args(["--format", "f32"])
            .arg("-P")
            .arg(format!(
                "{{ application.name = FakeVoiceApp application.process.binary = {FAKE_BINARY} \
                    node.autoconnect = false }}"
            ))
            .arg(wav)
            .spawn()
            .expect("pw-record is installed");
        Self {
            child,
            wav: wav.to_owned(),
        }
    }

    /// Stops recording cleanly (SIGINT, so the WAV gets flushed) and returns the left channel.
    fn stop_and_read(mut self) -> Vec<f32> {
        let status = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .expect("kill is installed");
        assert!(status.success());
        self.child.wait().expect("pw-record exits");
        let bytes = std::fs::read(&self.wav).expect("pw-record wrote a WAV file");
        wav_f32_samples(&bytes).step_by(2).collect()
    }
}

impl Drop for FakeVoiceApp {
    /// Makes sure a failing test doesn't leave `pw-record` running.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for_app(service: &PipeWireService, binary: &str) -> RecordingApp {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match service.events().recv_timeout(remaining) {
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

/// Polls `pw-link` until our links satisfy `done`; returns them as (output port, input port).
fn wait_for_links(done: impl Fn(&[(String, String)]) -> bool) -> Vec<(String, String)> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let links = inject_links();
        if done(&links) {
            return links;
        }
        assert!(Instant::now() < deadline, "unexpected links: {links:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Our links according to `pw-link -l`, which prints each output port followed by its links as
/// `  |-> node:port` lines.
fn inject_links() -> Vec<(String, String)> {
    let output = Command::new("pw-link")
        .arg("-l")
        .output()
        .expect("pw-link is installed");
    let listing = String::from_utf8_lossy(&output.stdout);
    let mut links = Vec::new();
    let mut port = "";
    for line in listing.lines() {
        match line.trim_start().strip_prefix("|-> ") {
            Some(target) if port.starts_with(INJECT_NODE_NAME) => {
                links.push((port.to_owned(), target.to_owned()));
            }
            Some(_) => {}
            None => port = line.trim(),
        }
    }
    links
}

fn same_channel(output_port: &str, input_port: &str) -> bool {
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
fn loud_part(samples: &[f32]) -> &[f32] {
    let is_loud = |s: &f32| s.abs() > 0.05;
    match (
        samples.iter().position(is_loud),
        samples.iter().rposition(is_loud),
    ) {
        (Some(first), Some(last)) => &samples[first..=last],
        _ => &[],
    }
}

fn dominant_frequency(samples: &[f32]) -> f32 {
    let rising = samples
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    rising as f32 * SAMPLE_RATE as f32 / samples.len() as f32
}
