//! Lists apps that record audio, or plays a sound file into one of them (and locally).
//!
//! ```text
//! cargo run --example play                                           # list recording apps
//! cargo run --example play -- --target Discord --file ~/airhorn.mp3  # play into Discord
//! ```
//!
//! `--target` matches the process binary or the app name, case-insensitively.
#![expect(
    clippy::print_stdout,
    reason = "a CLI example whose output is the point"
)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use soundboars::audio::{AudioEngine, Destination, EngineEvent, PlayRequest, RecordingApp};

/// How long to wait for the graph to settle (and for a target to appear).
const DISCOVERY: Duration = Duration::from_secs(2);

struct Args {
    target: Option<String>,
    file: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let engine = AudioEngine::start()?;
    match (args.target, args.file) {
        (None, _) => list_apps(&engine),
        (Some(target), Some(file)) => play_into(&engine, &target, file),
        (Some(_), None) => bail!("--target needs --file"),
    }
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        target: None,
        file: None,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(flag) = raw.next() {
        let value = raw
            .next()
            .with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--target" => args.target = Some(value),
            "--file" => args.file = Some(value.into()),
            _ => bail!("unknown argument {flag}; use [--target NAME --file PATH]"),
        }
    }
    Ok(args)
}

fn list_apps(engine: &AudioEngine) -> anyhow::Result<()> {
    let apps = collect_apps(engine, |_| false);
    if apps.is_empty() {
        println!("No app is recording audio right now.");
    }
    for app in apps {
        println!(
            "node {:>4}  binary={:<20} app={:<25} media={:<25} ports={}",
            app.node_id,
            app.process_binary.as_deref().unwrap_or("?"),
            app.app_name,
            app.media_name.as_deref().unwrap_or("?"),
            app.ports
                .iter()
                .map(|port| port.channel.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    Ok(())
}

fn play_into(engine: &AudioEngine, target: &str, file: PathBuf) -> anyhow::Result<()> {
    let apps = collect_apps(engine, |apps| apps.iter().any(|app| matches(app, target)));
    let app = apps
        .into_iter()
        .find(|app| matches(app, target))
        .with_context(|| format!("no recording app matches {target:?}"))?;
    println!(
        "Playing {} into node {} ({})…",
        file.display(),
        app.node_id,
        app.app_name
    );
    engine.link_to(app.node_id);
    engine.play(PlayRequest {
        path: file,
        trim: None,
        volume: 1.0,
        destination: Destination::Everywhere,
    });
    let result = wait_until_finished(engine);
    engine.unlink();
    result
}

/// Collects the recording apps for [`DISCOVERY`], or until `enough` says so.
fn collect_apps(
    engine: &AudioEngine,
    enough: impl Fn(&[RecordingApp]) -> bool,
) -> Vec<RecordingApp> {
    let deadline = Instant::now() + DISCOVERY;
    let mut apps = Vec::new();
    while let Ok(event) = engine
        .events()
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        match event {
            EngineEvent::RecordingApps(latest) => apps = latest,
            EngineEvent::Error(message) => println!("Error: {message}"),
            _ => {}
        }
        if enough(&apps) {
            break;
        }
    }
    apps
}

fn wait_until_finished(engine: &AudioEngine) -> anyhow::Result<()> {
    for event in engine.events() {
        match event {
            EngineEvent::Started(playing) => println!("Started, {:?} long.", playing.duration),
            EngineEvent::Finished { .. } => {
                println!("Done.");
                return Ok(());
            }
            EngineEvent::DecodeFailed { path, error } => {
                bail!("could not decode {}: {error}", path.display())
            }
            EngineEvent::Error(message) => println!("Error: {message}"),
            EngineEvent::RecordingApps(_)
            | EngineEvent::Microphone(_)
            | EngineEvent::Waveform { .. } => {}
        }
    }
    bail!("the audio engine stopped")
}

fn matches(app: &RecordingApp, target: &str) -> bool {
    let target = target.to_lowercase();
    [app.process_binary.as_deref(), Some(app.app_name.as_str())]
        .into_iter()
        .flatten()
        .any(|name| name.to_lowercase() == target)
}
