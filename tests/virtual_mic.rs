//! End-to-end test of the virtual microphone against a real PipeWire daemon: "SoundBoars
//! Microphone" must carry our 440 Hz sine plus the default source (a fake 1 kHz microphone) on both
//! channels, link the microphone only while someone records it, follow default changes, never feed
//! itself, and go away in app mode. Runs only with `SOUNDBOARS_REQUIRE_PIPEWIRE=1` (see
//! `common::pipewire_required`).

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::{
    FAKE_BINARY, Recorder, TIMEOUT, links_from, node_listing, port_names, same_channel, temp_wav,
    tone_amplitude, wait_for_app, wait_for_links, wait_for_node, wait_for_node_gone,
};
use soundboars::audio::sine::Sine;
use soundboars::audio::{
    Event, INJECT_NODE_NAME, PipeWireService, Renders, SAMPLE_RATE, VIRTUAL_MIC_NODE_NAME,
};

const MIC_A: &str = "soundboars-test-mic-a";
const MIC_B: &str = "soundboars-test-mic-b";

#[test]
fn the_virtual_mic_carries_sounds_and_the_default_microphone() {
    if !common::pipewire_required() {
        return;
    }
    let mic_wav = sine_wav("mic", 1000.0);
    let mic_a = FakeMic::start(MIC_A, "Fake Mic A", &mic_wav);
    wait_for_node(MIC_A);
    set_default_source(MIC_A);
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

    service.use_virtual_mic();
    wait_for_node(VIRTUAL_MIC_NODE_NAME);
    assert_virtual_mic_node();
    wait_for_microphone(&events, "Fake Mic A");
    let links = wait_for_links(INJECT_NODE_NAME, |links| links.len() == 2);
    assert!(
        links.iter().all(|(out, input)| same_channel(out, input)
            && input.starts_with(&format!("{VIRTUAL_MIC_NODE_NAME}:input_"))),
        "{links:?}"
    );

    // Nobody records the virtual mic, so the real one stays closed.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(links_from(MIC_A), []);

    let wav = temp_wav("virtual-mic");
    let recorder = Recorder::recording(&wav, VIRTUAL_MIC_NODE_NAME);
    let mic_links = wait_for_links(MIC_A, |links| links.len() == 2);
    let inputs: Vec<&str> = mic_links.iter().map(|(_, input)| input.as_str()).collect();
    assert!(inputs.contains(&"soundboars-mic:input_FL"), "{mic_links:?}");
    assert!(inputs.contains(&"soundboars-mic:input_FR"), "{mic_links:?}");
    std::thread::sleep(Duration::from_secs(2));
    let (left, right) = recorder.stop_and_read_stereo();
    for (channel, samples) in [("left", &left), ("right", &right)] {
        let last_second = &samples[samples.len().saturating_sub(SAMPLE_RATE as usize)..];
        let sound = tone_amplitude(last_second, 440.0);
        let mic = tone_amplitude(last_second, 1000.0);
        eprintln!("{channel}: 440 Hz at {sound:.3}, 1 kHz at {mic:.3}");
        assert!(sound > 0.3, "{channel}: sound at {sound}");
        assert!(mic > 0.05, "{channel}: microphone at {mic}");
    }
    wait_for_links(MIC_A, |links| links.is_empty());

    // A new default source replaces the microphone.
    let mic_b = FakeMic::start(MIC_B, "Fake Mic B", &mic_wav);
    wait_for_node(MIC_B);
    set_default_source(MIC_B);
    wait_for_microphone(&events, "Fake Mic B");
    let recorder = Recorder::recording(&wav, VIRTUAL_MIC_NODE_NAME);
    wait_for_links(MIC_B, |links| links.len() == 2);
    assert_eq!(links_from(MIC_A), []);

    // Our own virtual mic as default falls back to another source instead of feeding itself.
    set_default_source(VIRTUAL_MIC_NODE_NAME);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let fake_mic_links = links_from(MIC_A).len() + links_from(MIC_B).len();
        let loops = links_from(VIRTUAL_MIC_NODE_NAME)
            .into_iter()
            .filter(|(_, input)| input.starts_with(&format!("{VIRTUAL_MIC_NODE_NAME}:")))
            .count();
        assert_eq!(loops, 0, "the virtual mic feeds itself");
        if fake_mic_links == 2 && is_default_source(VIRTUAL_MIC_NODE_NAME) {
            break;
        }
        assert!(Instant::now() < deadline, "no fallback microphone linked");
        std::thread::sleep(Duration::from_millis(50));
    }
    recorder.stop_and_read();

    // App mode removes the virtual mic.
    let app_wav = temp_wav("virtual-mic-app");
    let voice_app = Recorder::fake_voice_app(&app_wav);
    let app = wait_for_app(&events, FAKE_BINARY);
    service.link_to(app.node_id);
    wait_for_node_gone(VIRTUAL_MIC_NODE_NAME);
    wait_for_links(INJECT_NODE_NAME, |links| {
        links.len() == 2
            && links
                .iter()
                .all(|(_, input)| !input.starts_with(VIRTUAL_MIC_NODE_NAME))
    });
    drop(service);
    wait_for_links(INJECT_NODE_NAME, |links| links.is_empty());
    voice_app.stop_and_read();
    drop((mic_a, mic_b));
    let _ = std::fs::remove_file(mic_wav);
}

/// The virtual mic is a stereo virtual source with FL/FR inputs and capture ports.
fn assert_virtual_mic_node() {
    let listing = node_listing(VIRTUAL_MIC_NODE_NAME).expect("the virtual mic exists");
    assert!(
        listing.contains("media.class = \"Audio/Source/Virtual\""),
        "{listing}"
    );
    assert!(
        listing.contains("node.description = \"SoundBoars Microphone\""),
        "{listing}"
    );
    let deadline = Instant::now() + TIMEOUT;
    let ports = |flag| -> Vec<String> {
        port_names(flag)
            .into_iter()
            .filter(|port| port.starts_with(&format!("{VIRTUAL_MIC_NODE_NAME}:")))
            .collect()
    };
    loop {
        let (inputs, outputs) = (ports("-i"), ports("-o"));
        if inputs == ["soundboars-mic:input_FL", "soundboars-mic:input_FR"]
            && outputs == ["soundboars-mic:capture_FL", "soundboars-mic:capture_FR"]
        {
            return;
        }
        assert!(Instant::now() < deadline, "ports: {inputs:?} {outputs:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_microphone(events: &mpsc::Receiver<Event>, description: &str) {
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(remaining) {
            Ok(Event::Microphone(Some(microphone))) if microphone == description => return,
            Ok(Event::Error(message)) => panic!("PipeWire error: {message}"),
            Ok(Event::Microphone(other)) => seen.push(format!("Microphone({other:?})")),
            Ok(_) => {}
            Err(_) => panic!(
                "{description} did not become the microphone; microphone events: {seen:?}\n{}",
                diagnostics()
            ),
        }
    }
}

/// The default-node metadata and the sources, for failure messages.
fn diagnostics() -> String {
    let run = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_else(|error| format!("{program}: {error}"))
    };
    format!(
        "pw-metadata:\n{}\nwpctl status:\n{}",
        run("pw-metadata", &["0"]),
        run("wpctl", &["status"])
    )
}

/// Makes the node named `name` the configured default source, like a desktop's sound settings do
/// (`wpctl set-default` refuses virtual sources); WirePlumber then makes it the default source.
fn set_default_source(name: &str) {
    let status = Command::new("pw-metadata")
        .args([
            "0",
            "default.configured.audio.source",
            &format!("{{\"name\":\"{name}\"}}"),
            "Spa:String:JSON",
        ])
        .stdout(std::process::Stdio::null())
        .status()
        .expect("pw-metadata is installed");
    assert!(status.success());
}

/// Whether WirePlumber reports the node named `name` as the default source.
fn is_default_source(name: &str) -> bool {
    let output = Command::new("pw-metadata")
        .args(["0", "default.audio.source"])
        .output()
        .expect("pw-metadata is installed");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"name\":\"{name}\""))
}

/// A microphone made of `pw-cat` playing a WAV file as an `Audio/Source` node (mono, so its one
/// port goes to both channels of the virtual mic).
struct FakeMic(Child);

impl FakeMic {
    fn start(name: &str, description: &str, wav: &Path) -> Self {
        let properties = format!(
            "{{ media.class = Audio/Source node.name = {name} \
               node.description = \"{description}\" node.autoconnect = false }}"
        );
        let child = Command::new("pw-cat")
            .args(["--playback", "--channels", "1", "-P", &properties])
            .arg(wav)
            .spawn()
            .expect("pw-cat is installed");
        Self(child)
    }
}

impl Drop for FakeMic {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A 16-bit mono WAV file with a minute of a `frequency` Hz sine at a quarter of full scale.
fn sine_wav(name: &str, frequency: f32) -> PathBuf {
    let frames = SAMPLE_RATE as usize * 60;
    let data: Vec<u8> = (0..frames)
        .flat_map(|frame| {
            let phase = 2.0 * std::f32::consts::PI * frequency * frame as f32 / SAMPLE_RATE as f32;
            ((phase.sin() * 0.25 * f32::from(i16::MAX)) as i16).to_le_bytes()
        })
        .collect();
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    let path = temp_wav(name);
    std::fs::write(&path, wav).expect("write the microphone's WAV file");
    path
}
