//! The X11 backend: grabs keys on the X11 root window with `global-hotkey`. Only works in X11
//! sessions; on Wayland, XWayland only sees keys while one of its windows has focus.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use eframe::egui::Key;
use global_hotkey::hotkey::{Code, HotKey, Modifiers as HotKeyModifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use super::{Binding, Capabilities, EventSink, Hotkey, HotkeyBackend, HotkeyEvent};

/// `global-hotkey` hotkey id → binding id.
type Ids = Arc<Mutex<HashMap<u32, String>>>;

pub struct X11Backend {
    manager: GlobalHotKeyManager,
    registered: Vec<HotKey>,
    ids: Ids,
    events: EventSink,
}

impl X11Backend {
    /// Fails without an X11 display. `global-hotkey` reports events through one process-wide
    /// handler, so only one `X11Backend` may exist at a time.
    pub fn start(events: EventSink) -> Result<Self, String> {
        if std::env::var_os("DISPLAY").is_none() {
            return Err("no X11 display ($DISPLAY is not set)".to_owned());
        }
        let manager = GlobalHotKeyManager::new().map_err(|error| error.to_string())?;
        let ids = Ids::default();
        let handler_ids = ids.clone();
        let handler_events = events.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            let ids = handler_ids.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(id) = ids.get(&event.id()) {
                handler_events.send(match event.state() {
                    HotKeyState::Pressed => HotkeyEvent::Pressed(id.clone()),
                    HotKeyState::Released => HotkeyEvent::Released(id.clone()),
                });
            }
        }));
        Ok(Self {
            manager,
            registered: Vec::new(),
            ids,
            events,
        })
    }

    fn unregister_all(&mut self) {
        if let Err(error) = self.manager.unregister_all(&self.registered) {
            tracing::warn!("could not release the X11 hotkeys: {error}");
        }
        self.registered.clear();
        self.ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    fn register_one(&mut self, binding: &Binding, hotkey: Hotkey) -> Result<(), String> {
        let x11_hotkey = to_x11(hotkey).ok_or_else(|| format!("{hotkey} is not supported"))?;
        let mut ids = self.ids.lock().unwrap_or_else(PoisonError::into_inner);
        if ids.contains_key(&x11_hotkey.id()) {
            return Err(format!("{hotkey} is used twice"));
        }
        self.manager
            .register(x11_hotkey)
            .map_err(|error| format!("{hotkey}: {error}"))?;
        ids.insert(x11_hotkey.id(), binding.id.clone());
        self.registered.push(x11_hotkey);
        Ok(())
    }
}

impl HotkeyBackend for X11Backend {
    fn register(&mut self, bindings: &[Binding]) {
        self.unregister_all();
        for binding in bindings {
            let Some(hotkey) = binding.hotkey else {
                continue;
            };
            if let Err(error) = self.register_one(binding, hotkey) {
                let message = format!("Could not register the hotkey {error}");
                self.events.send(HotkeyEvent::Error(message));
            }
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
}

impl Drop for X11Backend {
    fn drop(&mut self) {
        self.unregister_all();
        GlobalHotKeyEvent::set_event_handler(None::<fn(GlobalHotKeyEvent)>);
    }
}

fn to_x11(hotkey: Hotkey) -> Option<HotKey> {
    let mut modifiers = HotKeyModifiers::empty();
    for (set, modifier) in [
        (hotkey.modifiers.ctrl, HotKeyModifiers::CONTROL),
        (hotkey.modifiers.alt, HotKeyModifiers::ALT),
        (hotkey.modifiers.shift, HotKeyModifiers::SHIFT),
        (hotkey.modifiers.super_key, HotKeyModifiers::META),
    ] {
        if set {
            modifiers |= modifier;
        }
    }
    Some(HotKey::new(Some(modifiers), code(hotkey.key)?))
}

/// The `global-hotkey` key code of every key in [`super::hotkey::keysym`]'s table.
fn code(key: Key) -> Option<Code> {
    Some(match key {
        Key::A => Code::KeyA,
        Key::B => Code::KeyB,
        Key::C => Code::KeyC,
        Key::D => Code::KeyD,
        Key::E => Code::KeyE,
        Key::F => Code::KeyF,
        Key::G => Code::KeyG,
        Key::H => Code::KeyH,
        Key::I => Code::KeyI,
        Key::J => Code::KeyJ,
        Key::K => Code::KeyK,
        Key::L => Code::KeyL,
        Key::M => Code::KeyM,
        Key::N => Code::KeyN,
        Key::O => Code::KeyO,
        Key::P => Code::KeyP,
        Key::Q => Code::KeyQ,
        Key::R => Code::KeyR,
        Key::S => Code::KeyS,
        Key::T => Code::KeyT,
        Key::U => Code::KeyU,
        Key::V => Code::KeyV,
        Key::W => Code::KeyW,
        Key::X => Code::KeyX,
        Key::Y => Code::KeyY,
        Key::Z => Code::KeyZ,
        Key::Num0 => Code::Digit0,
        Key::Num1 => Code::Digit1,
        Key::Num2 => Code::Digit2,
        Key::Num3 => Code::Digit3,
        Key::Num4 => Code::Digit4,
        Key::Num5 => Code::Digit5,
        Key::Num6 => Code::Digit6,
        Key::Num7 => Code::Digit7,
        Key::Num8 => Code::Digit8,
        Key::Num9 => Code::Digit9,
        Key::F1 => Code::F1,
        Key::F2 => Code::F2,
        Key::F3 => Code::F3,
        Key::F4 => Code::F4,
        Key::F5 => Code::F5,
        Key::F6 => Code::F6,
        Key::F7 => Code::F7,
        Key::F8 => Code::F8,
        Key::F9 => Code::F9,
        Key::F10 => Code::F10,
        Key::F11 => Code::F11,
        Key::F12 => Code::F12,
        Key::F13 => Code::F13,
        Key::F14 => Code::F14,
        Key::F15 => Code::F15,
        Key::F16 => Code::F16,
        Key::F17 => Code::F17,
        Key::F18 => Code::F18,
        Key::F19 => Code::F19,
        Key::F20 => Code::F20,
        Key::F21 => Code::F21,
        Key::F22 => Code::F22,
        Key::F23 => Code::F23,
        Key::F24 => Code::F24,
        Key::F25 => Code::F25,
        Key::F26 => Code::F26,
        Key::F27 => Code::F27,
        Key::F28 => Code::F28,
        Key::F29 => Code::F29,
        Key::F30 => Code::F30,
        Key::F31 => Code::F31,
        Key::F32 => Code::F32,
        Key::F33 => Code::F33,
        Key::F34 => Code::F34,
        Key::F35 => Code::F35,
        Key::Space => Code::Space,
        Key::Enter => Code::Enter,
        Key::Tab => Code::Tab,
        Key::Backspace => Code::Backspace,
        Key::Insert => Code::Insert,
        Key::Delete => Code::Delete,
        Key::Home => Code::Home,
        Key::End => Code::End,
        Key::PageUp => Code::PageUp,
        Key::PageDown => Code::PageDown,
        Key::ArrowUp => Code::ArrowUp,
        Key::ArrowDown => Code::ArrowDown,
        Key::ArrowLeft => Code::ArrowLeft,
        Key::ArrowRight => Code::ArrowRight,
        Key::Minus => Code::Minus,
        Key::Equals => Code::Equal,
        Key::Comma => Code::Comma,
        Key::Period => Code::Period,
        Key::Slash => Code::Slash,
        Key::Backslash => Code::Backslash,
        Key::Semicolon => Code::Semicolon,
        Key::Quote => Code::Quote,
        Key::Backtick => Code::Backquote,
        Key::OpenBracket => Code::BracketLeft,
        Key::CloseBracket => Code::BracketRight,
        Key::Pause => Code::Pause,
        Key::PrintScreen => Code::PrintScreen,
        Key::ScrollLock => Code::ScrollLock,
        Key::MediaPlayPause => Code::MediaPlay,
        Key::MediaStop => Code::MediaStop,
        Key::MediaTrackNext => Code::MediaTrackNext,
        Key::MediaTrackPrevious => Code::MediaTrackPrevious,
        Key::AudioVolumeMute => Code::AudioVolumeMute,
        Key::AudioVolumeDown => Code::AudioVolumeDown,
        Key::AudioVolumeUp => Code::AudioVolumeUp,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkeys::hotkey::keysym;

    #[test]
    fn every_hotkey_key_has_an_x11_code() {
        for &key in Key::ALL {
            assert_eq!(code(key).is_some(), keysym(key).is_some(), "{key:?}");
        }
    }

    #[test]
    fn modifiers_are_translated() {
        let hotkey: Hotkey = "Ctrl+Alt+Shift+Super+F13".parse().expect("valid");
        let x11 = to_x11(hotkey).expect("supported");
        assert_eq!(
            x11.mods,
            HotKeyModifiers::CONTROL
                | HotKeyModifiers::ALT
                | HotKeyModifiers::SHIFT
                | HotKeyModifiers::META
        );
        assert_eq!(x11.key, Code::F13);
    }
}
