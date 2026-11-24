//! The `soundboars` binary: wires the config, the audio engine, the folder watcher and the UI.

use anyhow::Context as _;
use eframe::egui;
use soundboars::app::{APP_ID, App, AudioBackend, ICON_PNG, PortalFolderPicker, Services};
use soundboars::audio::AudioEngine;
use soundboars::config::ConfigStore;
use soundboars::import::ytdlp::YtDlp;
use soundboars::library::watch::NotifyWatcher;
use soundboars::midi::{HOTPLUG_POLL, MidiService, MidirInput};
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = ConfigStore::default_location()?;
    tracing::info!("config: {}", config.path().display());
    let mut services = Services {
        audio: start_audio(),
        watcher: Box::new(NotifyWatcher::new().context("could not watch folders")?),
        folder_picker: Box::new(PortalFolderPicker),
        config,
        midi: None,
        downloader: Box::new(YtDlp::new()),
    };
    let icon = eframe::icon_data::from_png_bytes(ICON_PNG).context("invalid window icon")?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(APP_ID)
            .with_title("SoundBoars")
            .with_icon(icon)
            .with_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SoundBoars",
        options,
        Box::new(|cc| {
            services.midi = start_midi(&cc.egui_ctx);
            Ok(Box::new(App::new(services)))
        }),
    )
    .map_err(|error| anyhow::anyhow!("could not open the window: {error}"))
}

/// The audio engine, or why there is none; the app still starts without it.
fn start_audio() -> Result<Box<dyn AudioBackend>, String> {
    match AudioEngine::start() {
        Ok(engine) => Ok(Box::new(engine)),
        Err(error) => {
            tracing::error!("could not start the audio engine: {error}");
            Err(error.to_string())
        }
    }
}

/// The MIDI thread, waking the UI for every trigger; `None` if it couldn't start.
fn start_midi(ctx: &egui::Context) -> Option<MidiService> {
    let ctx = ctx.clone();
    let wake = move || ctx.request_repaint();
    MidiService::start(Box::new(MidirInput::default()), HOTPLUG_POLL, wake)
        .inspect_err(|error| tracing::error!("could not start MIDI: {error}"))
        .ok()
}
