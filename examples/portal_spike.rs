//! Registers one global shortcut (`play-test`) through the XDG GlobalShortcuts portal and prints
//! its activations until Ctrl+C.
//!
//! ```text
//! cargo run --example portal_spike
//! ```
//!
//! Hyprland's portal ignores `preferred_trigger`; bind it in `hyprland.conf` instead, e.g.
//! `bind = CTRL, F13, global, me.maxjoehnk.soundboars:play-test`.
#![expect(
    clippy::print_stdout,
    reason = "a CLI example whose output is the point"
)]

use anyhow::Context;
use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use futures_util::stream::select;

const APP_ID: &str = "me.maxjoehnk.soundboars";

fn main() -> anyhow::Result<()> {
    ashpd::zbus::block_on(run())
}

async fn run() -> anyhow::Result<()> {
    // Host (non-Flatpak) apps must tell the portal who they are before opening sessions.
    ashpd::register_host_app(APP_ID.parse().context("app id")?)
        .await
        .context("registering the host app id")?;

    let portal = GlobalShortcuts::new().await?;
    println!("GlobalShortcuts portal version {}", portal.version());
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;

    let shortcut =
        NewShortcut::new("play-test", "Play the test sound").preferred_trigger("CTRL+F13");
    let bound = portal
        .bind_shortcuts(&session, &[shortcut], None, BindShortcutsOptions::default())
        .await?
        .response()?;
    for shortcut in bound.shortcuts() {
        println!(
            "Bound {} ({}) to {:?}",
            shortcut.id(),
            shortcut.description(),
            shortcut.trigger_description()
        );
    }

    let activated = portal
        .receive_activated()
        .await?
        .map(|event| format!("Activated   {}", event.shortcut_id()));
    let deactivated = portal
        .receive_deactivated()
        .await?
        .map(|event| format!("Deactivated {}", event.shortcut_id()));
    let mut events = select(activated, deactivated);
    println!("Waiting for shortcut events (Ctrl+C to quit)…");
    while let Some(line) = events.next().await {
        println!("{line}");
    }
    Ok(())
}
