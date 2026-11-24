//! [`Hotkey`]: a key combination, stored in the config in one canonical, human-readable format
//! (`Ctrl+Alt+Shift+Super+<key>`, e.g. `Ctrl+Shift+F13`), and translated to the formats of the
//! XDG shortcuts spec and of Hyprland's config.

use std::fmt;
use std::str::FromStr;

use eframe::egui::{self, Key};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The modifiers a hotkey can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Windows/logo key. egui doesn't report it on Linux, so it can't be recorded, only
    /// written into the config by hand.
    pub super_key: bool,
}

impl Modifiers {
    /// `(set, canonical name, XDG shortcuts spec name, Hyprland name)`, in canonical order.
    fn names(self) -> [(bool, &'static str, &'static str, &'static str); 4] {
        [
            (self.ctrl, "Ctrl", "CTRL", "CTRL"),
            (self.alt, "Alt", "ALT", "ALT"),
            (self.shift, "Shift", "SHIFT", "SHIFT"),
            (self.super_key, "Super", "LOGO", "SUPER"),
        ]
    }
}

impl From<egui::Modifiers> for Modifiers {
    fn from(modifiers: egui::Modifiers) -> Self {
        Self {
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
            shift: modifiers.shift,
            super_key: false,
        }
    }
}

/// A key plus modifiers. Only keys with an X11 keysym (see [`keysym`]) are allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: Key,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseHotkeyError {
    #[error("“{0}” has no key")]
    NoKey(String),
    #[error("“{0}” is not a supported key")]
    UnknownKey(String),
    #[error("“{0}” has more than one key")]
    TwoKeys(String),
}

impl Hotkey {
    /// The hotkey of a key press, or `None` for keys that can't be hotkeys (modifiers, Escape, …).
    /// Prefers the logical key (what the keyboard layout says) and falls back to the physical one,
    /// e.g. for Shift+1, whose logical key is `!`.
    pub fn from_key_press(
        key: Key,
        physical_key: Option<Key>,
        modifiers: egui::Modifiers,
    ) -> Option<Self> {
        let key = [Some(key), physical_key]
            .into_iter()
            .flatten()
            .find(|&key| keysym(key).is_some())?;
        Some(Self {
            modifiers: modifiers.into(),
            key,
        })
    }

    /// For the XDG GlobalShortcuts portal's `preferred_trigger` (shortcuts spec format), e.g.
    /// `CTRL+SHIFT+F13`.
    pub fn xdg_trigger(self) -> String {
        self.join("+", |(_, _, xdg, _)| xdg)
    }

    /// For Hyprland's `hl.bind`, e.g. `CTRL + SHIFT + F13`.
    pub fn hyprland_keys(self) -> String {
        self.join(" + ", |(_, _, _, hyprland)| hyprland)
    }

    fn join(
        self,
        separator: &str,
        name: impl Fn((bool, &'static str, &'static str, &'static str)) -> &'static str,
    ) -> String {
        let mut parts: Vec<&str> = (self.modifiers.names().into_iter())
            .filter(|(set, ..)| *set)
            .map(name)
            .collect();
        parts.push(keysym(self.key).unwrap_or(self.key.name()));
        parts.join(separator)
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (set, name, ..) in self.modifiers.names() {
            if set {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(self.key.name())
    }
}

impl FromStr for Hotkey {
    type Err = ParseHotkeyError;

    /// Parses the canonical format, tolerating other cases, spaces and modifier aliases
    /// (`Control`, `Meta`, `Logo`).
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut modifiers = Modifiers::default();
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.ctrl = true,
                "alt" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "super" | "meta" | "logo" => modifiers.super_key = true,
                _ if key.is_some() => return Err(ParseHotkeyError::TwoKeys(text.to_owned())),
                _ => key = Some(parse_key(part)?),
            }
        }
        let key = key.ok_or_else(|| ParseHotkeyError::NoKey(text.to_owned()))?;
        Ok(Self { modifiers, key })
    }
}

fn parse_key(name: &str) -> Result<Key, ParseHotkeyError> {
    let unknown = || ParseHotkeyError::UnknownKey(name.to_owned());
    let key = KEYSYMS
        .iter()
        .map(|&(key, _)| key)
        .find(|key| key.name().eq_ignore_ascii_case(name))
        .ok_or_else(unknown)?;
    Ok(key)
}

impl Serialize for Hotkey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// For `#[serde(deserialize_with)]` on optional hotkeys: an invalid hotkey is dropped with a
/// warning instead of rejecting the whole config file.
pub fn deserialize_lenient<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Hotkey>, D::Error> {
    let Some(text) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    match text.parse() {
        Ok(hotkey) => Ok(Some(hotkey)),
        Err(error) => {
            tracing::warn!("ignoring the hotkey in the config: {error}");
            Ok(None)
        }
    }
}

/// The X11 keysym name of `key` (as used by xkbcommon, the XDG shortcuts spec and Hyprland), or
/// `None` if it can't be a hotkey.
pub fn keysym(key: Key) -> Option<&'static str> {
    KEYSYMS
        .iter()
        .find(|&&(candidate, _)| candidate == key)
        .map(|&(_, keysym)| keysym)
}

/// Every key a hotkey can use, with its keysym name.
const KEYSYMS: &[(Key, &str)] = &[
    (Key::A, "a"),
    (Key::B, "b"),
    (Key::C, "c"),
    (Key::D, "d"),
    (Key::E, "e"),
    (Key::F, "f"),
    (Key::G, "g"),
    (Key::H, "h"),
    (Key::I, "i"),
    (Key::J, "j"),
    (Key::K, "k"),
    (Key::L, "l"),
    (Key::M, "m"),
    (Key::N, "n"),
    (Key::O, "o"),
    (Key::P, "p"),
    (Key::Q, "q"),
    (Key::R, "r"),
    (Key::S, "s"),
    (Key::T, "t"),
    (Key::U, "u"),
    (Key::V, "v"),
    (Key::W, "w"),
    (Key::X, "x"),
    (Key::Y, "y"),
    (Key::Z, "z"),
    (Key::Num0, "0"),
    (Key::Num1, "1"),
    (Key::Num2, "2"),
    (Key::Num3, "3"),
    (Key::Num4, "4"),
    (Key::Num5, "5"),
    (Key::Num6, "6"),
    (Key::Num7, "7"),
    (Key::Num8, "8"),
    (Key::Num9, "9"),
    (Key::F1, "F1"),
    (Key::F2, "F2"),
    (Key::F3, "F3"),
    (Key::F4, "F4"),
    (Key::F5, "F5"),
    (Key::F6, "F6"),
    (Key::F7, "F7"),
    (Key::F8, "F8"),
    (Key::F9, "F9"),
    (Key::F10, "F10"),
    (Key::F11, "F11"),
    (Key::F12, "F12"),
    (Key::F13, "F13"),
    (Key::F14, "F14"),
    (Key::F15, "F15"),
    (Key::F16, "F16"),
    (Key::F17, "F17"),
    (Key::F18, "F18"),
    (Key::F19, "F19"),
    (Key::F20, "F20"),
    (Key::F21, "F21"),
    (Key::F22, "F22"),
    (Key::F23, "F23"),
    (Key::F24, "F24"),
    (Key::F25, "F25"),
    (Key::F26, "F26"),
    (Key::F27, "F27"),
    (Key::F28, "F28"),
    (Key::F29, "F29"),
    (Key::F30, "F30"),
    (Key::F31, "F31"),
    (Key::F32, "F32"),
    (Key::F33, "F33"),
    (Key::F34, "F34"),
    (Key::F35, "F35"),
    (Key::Space, "space"),
    (Key::Enter, "Return"),
    (Key::Tab, "Tab"),
    (Key::Backspace, "BackSpace"),
    (Key::Insert, "Insert"),
    (Key::Delete, "Delete"),
    (Key::Home, "Home"),
    (Key::End, "End"),
    (Key::PageUp, "Prior"),
    (Key::PageDown, "Next"),
    (Key::ArrowUp, "Up"),
    (Key::ArrowDown, "Down"),
    (Key::ArrowLeft, "Left"),
    (Key::ArrowRight, "Right"),
    (Key::Minus, "minus"),
    (Key::Equals, "equal"),
    (Key::Comma, "comma"),
    (Key::Period, "period"),
    (Key::Slash, "slash"),
    (Key::Backslash, "backslash"),
    (Key::Semicolon, "semicolon"),
    (Key::Quote, "apostrophe"),
    (Key::Backtick, "grave"),
    (Key::OpenBracket, "bracketleft"),
    (Key::CloseBracket, "bracketright"),
    (Key::Pause, "Pause"),
    (Key::PrintScreen, "Print"),
    (Key::ScrollLock, "Scroll_Lock"),
    (Key::MediaPlayPause, "XF86AudioPlay"),
    (Key::MediaStop, "XF86AudioStop"),
    (Key::MediaTrackNext, "XF86AudioNext"),
    (Key::MediaTrackPrevious, "XF86AudioPrev"),
    (Key::AudioVolumeMute, "XF86AudioMute"),
    (Key::AudioVolumeDown, "XF86AudioLowerVolume"),
    (Key::AudioVolumeUp, "XF86AudioRaiseVolume"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn hotkey(text: &str) -> Hotkey {
        text.parse().expect("valid hotkey")
    }

    #[test]
    fn canonical_strings_round_trip() {
        for text in [
            "Ctrl+Shift+F13",
            "F13",
            "Ctrl+Alt+Shift+Super+A",
            "Alt+1",
            "Ctrl+Space",
            "Shift+Up",
            "Ctrl+PageDown",
            "Minus",
            "MediaPlayPause",
        ] {
            assert_eq!(hotkey(text).to_string(), text);
        }
    }

    #[test]
    fn every_supported_key_round_trips() {
        for &(key, _) in KEYSYMS {
            let hotkey = Hotkey {
                modifiers: Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
                key,
            };
            assert_eq!(hotkey.to_string().parse(), Ok(hotkey), "{key:?}");
        }
    }

    #[test]
    fn parsing_is_lenient_and_normalizes() {
        assert_eq!(
            hotkey(" shift + control +f13 ").to_string(),
            "Ctrl+Shift+F13"
        );
        assert_eq!(hotkey("meta+a").to_string(), "Super+A");
        assert_eq!(hotkey("LOGO+a").to_string(), "Super+A");
    }

    #[test]
    fn invalid_hotkeys_are_rejected() {
        let error = |text: &str| text.parse::<Hotkey>().expect_err(text);
        assert_eq!(error("Ctrl+"), ParseHotkeyError::UnknownKey(String::new()));
        assert_eq!(
            error("Ctrl+Shift"),
            ParseHotkeyError::NoKey("Ctrl+Shift".into())
        );
        assert_eq!(
            error("Ctrl+A+B"),
            ParseHotkeyError::TwoKeys("Ctrl+A+B".into())
        );
        assert_eq!(
            error("Hyper+A"),
            ParseHotkeyError::UnknownKey("Hyper".into())
        );
        assert_eq!(
            error("Escape"),
            ParseHotkeyError::UnknownKey("Escape".into())
        );
    }

    #[test]
    fn translates_to_xdg_and_hyprland_formats() {
        let ctrl_shift_f13 = hotkey("Ctrl+Shift+F13");
        assert_eq!(ctrl_shift_f13.xdg_trigger(), "CTRL+SHIFT+F13");
        assert_eq!(ctrl_shift_f13.hyprland_keys(), "CTRL + SHIFT + F13");
        assert_eq!(hotkey("Super+Alt+A").xdg_trigger(), "ALT+LOGO+a");
        assert_eq!(hotkey("Ctrl+Space").hyprland_keys(), "CTRL + space");
    }

    #[test]
    fn key_presses_become_hotkeys() {
        let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        let pressed = Hotkey::from_key_press(Key::F13, None, ctrl_shift);
        assert_eq!(
            pressed.map(|h| h.to_string()).as_deref(),
            Some("Ctrl+Shift+F13")
        );

        // Shift+1 is "!" on a US layout; the physical key is used instead.
        let pressed = Hotkey::from_key_press(
            Key::Exclamationmark,
            Some(Key::Num1),
            egui::Modifiers::SHIFT,
        );
        assert_eq!(pressed.map(|h| h.to_string()).as_deref(), Some("Shift+1"));

        // On macOS `command` is Cmd; on Linux it mirrors Ctrl and must not add anything.
        let pressed = Hotkey::from_key_press(Key::A, None, egui::Modifiers::COMMAND);
        assert_eq!(pressed.map(|h| h.to_string()).as_deref(), Some("A"));

        assert_eq!(Hotkey::from_key_press(Key::Escape, None, ctrl_shift), None);
        assert_eq!(
            Hotkey::from_key_press(Key::ShiftLeft, None, ctrl_shift),
            None
        );
    }

    #[test]
    fn invalid_hotkeys_in_the_config_are_dropped() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default, deserialize_with = "deserialize_lenient")]
            hotkey: Option<Hotkey>,
        }
        let parse = |text: &str| toml::from_str::<Wrapper>(text).expect("valid toml").hotkey;
        assert_eq!(parse(r#"hotkey = "Ctrl+F13""#), Some(hotkey("Ctrl+F13")));
        assert_eq!(parse(r#"hotkey = "Ctrl+Nope""#), None);
        assert_eq!(parse(""), None);
    }
}
