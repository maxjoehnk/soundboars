//! Global hotkeys: [`HotkeyBackend`] registers the app's [`Binding`]s with the desktop and reports
//! presses as [`HotkeyEvent`]s. Two backends: the XDG GlobalShortcuts portal ([`portal`]) and X11
//! key grabs ([`x11`]).

pub mod hotkey;
pub mod portal;
pub mod x11;

use std::sync::{Arc, mpsc};

use serde::{Deserialize, Serialize};

pub use hotkey::{Hotkey, Modifiers, ParseHotkeyError};

use crate::app::APP_ID;

/// Something the user can trigger with a global hotkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Stable across restarts and readable: users type it into their compositor's config.
    pub id: String,
    /// Shown by the desktop's shortcut settings.
    pub description: String,
    /// The keys to grab (X11) or to suggest to the desktop (portal); X11 skips bindings without.
    pub hotkey: Option<Hotkey>,
}

/// What a backend reports, from its own thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// The binding with this id was pressed.
    Pressed(String),
    Released(String),
    /// Something went wrong (e.g. no portal); the app keeps working without (some) hotkeys.
    Error(String),
}

/// Where backends send their events (from their own threads), e.g. presses to the app's trigger
/// thread and errors to the UI.
#[derive(Clone)]
pub struct EventSink(Arc<dyn Fn(HotkeyEvent) + Send + Sync>);

impl EventSink {
    pub fn new(on_event: impl Fn(HotkeyEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(on_event))
    }

    /// A sink that sends every event to `sender`.
    pub fn channel(sender: mpsc::Sender<HotkeyEvent>) -> Self {
        // Fails only when the receiver is gone, and then nobody is interested anymore.
        Self::new(move |event| {
            let _ = sender.send(event);
        })
    }

    pub fn send(&self, event: HotkeyEvent) {
        (self.0)(event);
    }
}

/// What a backend can do, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// The backend takes the keys from the app (X11 grabs, the portal's `preferred_trigger`).
    /// If not, keys are bound outside the app, see [`Self::bind_hint`].
    pub assign_in_app: bool,
    /// How to bind a shortcut outside the app.
    pub bind_hint: Option<BindHint>,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            assign_in_app: true,
            bind_hint: None,
        }
    }
}

/// Generates the config line that binds a shortcut in a desktop that ignores the app's keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindHint {
    /// Hyprland ≥ 0.55 (Lua config), whose portal ignores `preferred_trigger`.
    Hyprland,
}

/// The keys suggested in a bind hint while the binding has no hotkey.
const PLACEHOLDER_KEYS: &str = "CTRL + F13";

impl BindHint {
    /// Tells the user what to do with [`Self::snippet`].
    pub fn instructions(self) -> &'static str {
        match self {
            Self::Hyprland => "Bind it in your Hyprland config (adjust the keys):",
        }
    }

    /// The line to paste into the desktop's config, with `hotkey` (or placeholder keys).
    pub fn snippet(self, shortcut_id: &str, hotkey: Option<Hotkey>) -> String {
        match self {
            Self::Hyprland => {
                let keys =
                    hotkey.map_or_else(|| PLACEHOLDER_KEYS.to_owned(), Hotkey::hyprland_keys);
                format!(r#"hl.bind("{keys}", hl.dsp.global("{APP_ID}:{shortcut_id}"))"#)
            }
        }
    }
}

/// Registers bindings with the desktop. Events go to the [`EventSink`] it was started with.
pub trait HotkeyBackend {
    /// Replaces all registered bindings with `bindings`.
    fn register(&mut self, bindings: &[Binding]);
    fn capabilities(&self) -> Capabilities;
}

/// Which backend to use (a setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    /// The XDG GlobalShortcuts portal.
    Portal,
    /// X11 key grabs with `global-hotkey`; only fire while an X11 window has focus on Wayland.
    X11,
    Off,
}

impl BackendKind {
    /// The portal on Wayland, X11 otherwise.
    pub fn default_for(wayland: bool) -> Self {
        if wayland { Self::Portal } else { Self::X11 }
    }

    /// [`Self::default_for`] the current session (`$WAYLAND_DISPLAY`).
    pub fn default_for_session() -> Self {
        Self::default_for(std::env::var_os("WAYLAND_DISPLAY").is_some())
    }
}

/// Starts the backend of `kind`; `None` for [`BackendKind::Off`].
pub fn start(
    kind: BackendKind,
    events: EventSink,
) -> Result<Option<Box<dyn HotkeyBackend>>, String> {
    Ok(match kind {
        BackendKind::Portal => {
            let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
            Some(Box::new(portal::PortalBackend::start(events, &desktop)?))
        }
        BackendKind::X11 => Some(Box::new(x11::X11Backend::start(events)?)),
        BackendKind::Off => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_backend_depends_on_the_session() {
        assert_eq!(BackendKind::default_for(true), BackendKind::Portal);
        assert_eq!(BackendKind::default_for(false), BackendKind::X11);
    }

    #[test]
    fn hyprland_hints_bind_the_global_shortcut() {
        let hotkey = "Ctrl+Shift+F13".parse().ok();
        assert_eq!(
            BindHint::Hyprland.snippet("play-airhorn", hotkey),
            r#"hl.bind("CTRL + SHIFT + F13", hl.dsp.global("me.maxjoehnk.soundboars:play-airhorn"))"#
        );
        assert_eq!(
            BindHint::Hyprland.snippet("stop-all", None),
            r#"hl.bind("CTRL + F13", hl.dsp.global("me.maxjoehnk.soundboars:stop-all"))"#
        );
    }

    #[test]
    fn the_channel_sink_sends_every_event() {
        let (sender, events) = mpsc::channel();
        let sink = EventSink::channel(sender);
        sink.send(HotkeyEvent::Pressed("a".to_owned()));
        sink.send(HotkeyEvent::Released("a".to_owned()));
        assert_eq!(
            events.try_iter().collect::<Vec<_>>(),
            [
                HotkeyEvent::Pressed("a".to_owned()),
                HotkeyEvent::Released("a".to_owned())
            ]
        );
    }
}
