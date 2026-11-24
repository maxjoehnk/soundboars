//! Lists apps that record audio, or plays a 440 Hz sine into one of them.
//!
//! ```text
//! cargo run --example inject_spike                          # list recording apps
//! cargo run --example inject_spike -- --target Discord      # play into Discord for 5 s
//! cargo run --example inject_spike -- --target Discord --seconds 10
//! ```
//!
//! `--target` matches the process binary or the app name, case-insensitively.
#![expect(
    clippy::print_stdout,
    reason = "a CLI example whose output is the point"
)]

use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use soundboars::audio::sine::Sine;
use soundboars::audio::{Event, PipeWireService, RecordingApp};

/// How long to wait for the graph to settle (and for a target to appear).
const DISCOVERY: Duration = Duration::from_secs(2);

struct Args {
    target: Option<String>,
    seconds: u64,
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let mut sine = Sine::new(440.0, 0.3);
    let service = PipeWireService::start(move |frames| sine.render(frames))?;
    match args.target {
        None => list_apps(&service),
        Some(target) => play_into(&service, &target, Duration::from_secs(args.seconds)),
    }
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        target: None,
        seconds: 5,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(flag) = raw.next() {
        let value = raw
            .next()
            .with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--target" => args.target = Some(value),
            "--seconds" => args.seconds = value.parse().context("--seconds")?,
            _ => bail!("unknown argument {flag}; use --target NAME [--seconds N]"),
        }
    }
    Ok(args)
}

fn list_apps(service: &PipeWireService) -> anyhow::Result<()> {
    let apps = collect_apps(service, |_| false);
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

fn play_into(service: &PipeWireService, target: &str, duration: Duration) -> anyhow::Result<()> {
    let apps = collect_apps(service, |apps| apps.iter().any(|app| matches(app, target)));
    let app = apps
        .into_iter()
        .find(|app| matches(app, target))
        .with_context(|| format!("no recording app matches {target:?}"))?;
    println!(
        "Playing 440 Hz into node {} ({}) for {duration:?}…",
        app.node_id, app.app_name
    );
    service.link_to(app.node_id);
    report_errors_for(service, duration);
    service.unlink();
    println!("Done.");
    Ok(())
}

/// Collects the recording apps for [`DISCOVERY`], or until `enough` says so.
fn collect_apps(
    service: &PipeWireService,
    enough: impl Fn(&[RecordingApp]) -> bool,
) -> Vec<RecordingApp> {
    let deadline = Instant::now() + DISCOVERY;
    let mut apps = Vec::new();
    while let Ok(event) = service
        .events()
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        match event {
            Event::RecordingApps(latest) => apps = latest,
            Event::Error(message) => println!("PipeWire error: {message}"),
        }
        if enough(&apps) {
            break;
        }
    }
    apps
}

fn report_errors_for(service: &PipeWireService, duration: Duration) {
    let deadline = Instant::now() + duration;
    while let Ok(event) = service
        .events()
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        if let Event::Error(message) = event {
            println!("PipeWire error: {message}");
        }
    }
}

fn matches(app: &RecordingApp, target: &str) -> bool {
    let target = target.to_lowercase();
    [app.process_binary.as_deref(), Some(app.app_name.as_str())]
        .into_iter()
        .flatten()
        .any(|name| name.to_lowercase() == target)
}
