//! The XDG GlobalShortcuts portal backend (`ashpd`), running on its own thread with an async
//! executor. A portal session can bind shortcuts only once, so every new set of bindings gets a
//! new session.

use std::future::ready;
use std::thread;

use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use ashpd::desktop::{CreateSessionOptions, Session};
use futures_channel::mpsc;
use futures_util::stream::{self, StreamExt, select};

use super::{BindHint, Binding, Capabilities, EventSink, HotkeyBackend, HotkeyEvent};
use crate::app::APP_ID;

pub struct PortalBackend {
    bindings: mpsc::UnboundedSender<Vec<Binding>>,
    capabilities: Capabilities,
}

impl PortalBackend {
    /// Connects to the portal on a new thread; failures arrive as [`HotkeyEvent::Error`].
    /// `desktop` is `$XDG_CURRENT_DESKTOP`.
    pub fn start(events: EventSink, desktop: &str) -> Result<Self, String> {
        let (bindings, receiver) = mpsc::unbounded();
        thread::Builder::new()
            .name("hotkey-portal".to_owned())
            .spawn(move || ashpd::zbus::block_on(run(receiver, events)))
            .map_err(|error| format!("could not start the portal thread: {error}"))?;
        Ok(Self {
            bindings,
            capabilities: capabilities_for(desktop),
        })
    }
}

impl HotkeyBackend for PortalBackend {
    fn register(&mut self, bindings: &[Binding]) {
        // Fails only if the portal thread ended, which it reported already.
        let _ = self.bindings.unbounded_send(bindings.to_vec());
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }
}

/// Hyprland's portal ignores `preferred_trigger`: keys are bound in its config instead. Other
/// desktops (KDE, GNOME) take the suggested keys, or let the user pick them in their own dialog.
pub fn capabilities_for(desktop: &str) -> Capabilities {
    let hyprland = desktop
        .split(':')
        .any(|name| name.trim().eq_ignore_ascii_case("hyprland"));
    Capabilities {
        assign_in_app: !hyprland,
        bind_hint: hyprland.then_some(BindHint::Hyprland),
    }
}

enum Input {
    Bind(Vec<Binding>),
    Activated(String),
    Deactivated(String),
    /// The backend was dropped.
    Stop,
}

async fn run(bindings: mpsc::UnboundedReceiver<Vec<Binding>>, events: EventSink) {
    if let Err(error) = serve(bindings, &events).await {
        let message = format!("Global shortcuts are not available (XDG portal): {error}");
        events.send(HotkeyEvent::Error(message));
    }
}

async fn serve(
    bindings: mpsc::UnboundedReceiver<Vec<Binding>>,
    events: &EventSink,
) -> Result<(), ashpd::Error> {
    register_app_id().await;
    let portal = GlobalShortcuts::new().await?;
    let activated = (portal.receive_activated().await?)
        .map(|event| Input::Activated(event.shortcut_id().to_owned()));
    let deactivated = (portal.receive_deactivated().await?)
        .map(|event| Input::Deactivated(event.shortcut_id().to_owned()));
    let bindings = bindings
        .map(Input::Bind)
        .chain(stream::once(ready(Input::Stop)));
    let mut inputs = select(bindings, select(activated, deactivated));

    let mut session = None;
    while let Some(input) = inputs.next().await {
        match input {
            Input::Bind(bindings) => {
                close(session.take()).await;
                match bind(&portal, &bindings).await {
                    Ok(bound) => session = Some(bound),
                    Err(error) => {
                        let message = format!("Could not register the global shortcuts: {error}");
                        events.send(HotkeyEvent::Error(message));
                    }
                }
            }
            Input::Activated(id) => events.send(HotkeyEvent::Pressed(id)),
            Input::Deactivated(id) => events.send(HotkeyEvent::Released(id)),
            Input::Stop => break,
        }
    }
    close(session).await;
    Ok(())
}

/// Tells the portal our app ID (we're not sandboxed, so it can't know). Portals before
/// xdg-desktop-portal 1.19 don't have the registry; they work without.
async fn register_app_id() {
    let registered = match APP_ID.parse() {
        Ok(app_id) => ashpd::register_host_app(app_id).await,
        Err(error) => Err(error),
    };
    if let Err(error) = registered {
        tracing::warn!("could not register the app ID with the portal: {error}");
    }
}

async fn bind(
    portal: &GlobalShortcuts,
    bindings: &[Binding],
) -> Result<Session<GlobalShortcuts>, ashpd::Error> {
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;
    let triggers: Vec<Option<String>> = bindings
        .iter()
        .map(|binding| binding.hotkey.map(|hotkey| hotkey.xdg_trigger()))
        .collect();
    let shortcuts: Vec<NewShortcut> = bindings
        .iter()
        .zip(&triggers)
        .map(|(binding, trigger)| {
            NewShortcut::new(&binding.id, &binding.description)
                .preferred_trigger(trigger.as_deref())
        })
        .collect();
    let bound = portal
        .bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default())
        .await?
        .response()?;
    for shortcut in bound.shortcuts() {
        tracing::info!(
            "global shortcut {} bound to “{}”",
            shortcut.id(),
            shortcut.trigger_description()
        );
    }
    Ok(session)
}

async fn close(session: Option<Session<GlobalShortcuts>>) {
    if let Some(session) = session
        && let Err(error) = session.close().await
    {
        tracing::debug!("could not close the shortcuts session: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_hyprland_needs_bind_hints() {
        for desktop in ["Hyprland", "hyprland", "Hyprland:wlroots"] {
            let capabilities = capabilities_for(desktop);
            assert!(!capabilities.assign_in_app, "{desktop}");
            assert_eq!(capabilities.bind_hint, Some(BindHint::Hyprland));
        }
        for desktop in ["KDE", "GNOME", "", "sway"] {
            assert_eq!(
                capabilities_for(desktop),
                Capabilities::default(),
                "{desktop}"
            );
        }
    }
}
