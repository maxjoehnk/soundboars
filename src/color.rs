//! Tile colors: a named preset with a fill tuned for each theme, or a custom RGB color. Stored in
//! the config as the preset's name (`"red"`) or as hex (`"#ff8800"`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The color of a sound's tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileColor {
    Preset(Preset),
    /// sRGB; the same fill in both themes.
    Custom([u8; 3]),
}

/// The named tile colors, in the order the pickers show them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Preset {
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Purple,
    Pink,
    Gray,
}

impl Preset {
    pub const ALL: [Self; 9] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Purple,
        Self::Pink,
        Self::Gray,
    ];

    /// The name in the config, lowercase.
    pub fn name(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Teal => "teal",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Pink => "pink",
            Self::Gray => "gray",
        }
    }

    /// The name in the UI, e.g. `Red`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Red => "Red",
            Self::Orange => "Orange",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Teal => "Teal",
            Self::Blue => "Blue",
            Self::Purple => "Purple",
            Self::Pink => "Pink",
            Self::Gray => "Gray",
        }
    }

    /// The fill: deep tones on the dark theme, pastels on the light one.
    fn rgb(self, dark: bool) -> [u8; 3] {
        match (self, dark) {
            (Self::Red, true) => [150, 45, 45],
            (Self::Orange, true) => [165, 85, 25],
            (Self::Yellow, true) => [150, 120, 20],
            (Self::Green, true) => [45, 115, 55],
            (Self::Teal, true) => [20, 110, 110],
            (Self::Blue, true) => [40, 90, 165],
            (Self::Purple, true) => [105, 65, 155],
            (Self::Pink, true) => [160, 55, 115],
            (Self::Gray, true) => [85, 85, 92],
            (Self::Red, false) => [245, 180, 178],
            (Self::Orange, false) => [250, 205, 160],
            (Self::Yellow, false) => [245, 228, 140],
            (Self::Green, false) => [185, 225, 172],
            (Self::Teal, false) => [165, 220, 214],
            (Self::Blue, false) => [175, 202, 245],
            (Self::Purple, false) => [212, 192, 240],
            (Self::Pink, false) => [245, 188, 216],
            (Self::Gray, false) => [208, 208, 214],
        }
    }
}

impl TileColor {
    /// The tile's fill in the dark (`true`) or light theme.
    pub fn rgb(self, dark: bool) -> [u8; 3] {
        match self {
            Self::Preset(preset) => preset.rgb(dark),
            Self::Custom(rgb) => rgb,
        }
    }
}

/// Whether dark text reads better than light text on `fill` (by WCAG contrast).
pub fn wants_dark_text(fill: [u8; 3]) -> bool {
    let luminance = relative_luminance(fill);
    // Contrast with black (L = 0) vs. with white (L = 1): (L + 0.05) / 0.05 vs. 1.05 / (L + 0.05).
    (luminance + 0.05) / 0.05 > 1.05 / (luminance + 0.05)
}

/// WCAG relative luminance of an sRGB color, 0 (black) to 1 (white).
fn relative_luminance([r, g, b]: [u8; 3]) -> f32 {
    let linear = |channel: u8| {
        let c = f32::from(channel) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown color {0:?}, expected a name like \"red\" or a hex color like \"#ff8800\"")]
pub struct ParseColorError(String);

impl FromStr for TileColor {
    type Err = ParseColorError;

    /// A preset's name (any case) or `#rrggbb`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let invalid = || ParseColorError(text.to_owned());
        if let Some(hex) = text.strip_prefix('#') {
            return parse_hex(hex).map(Self::Custom).ok_or_else(invalid);
        }
        Preset::ALL
            .into_iter()
            .find(|preset| preset.name().eq_ignore_ascii_case(text))
            .map(Self::Preset)
            .ok_or_else(invalid)
    }
}

fn parse_hex(hex: &str) -> Option<[u8; 3]> {
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

impl fmt::Display for TileColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preset(preset) => f.write_str(preset.name()),
            Self::Custom([r, g, b]) => write!(f, "#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

impl Serialize for TileColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// For `#[serde(deserialize_with)]` on optional colors: an invalid color is dropped with a warning
/// instead of rejecting the whole config file.
pub fn deserialize_lenient<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<TileColor>, D::Error> {
    let Some(text) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    match text.parse() {
        Ok(color) => Ok(Some(color)),
        Err(error) => {
            tracing::warn!("ignoring the color in the config: {error}");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_hex_colors_parse_and_print_back() {
        assert_eq!("red".parse(), Ok(TileColor::Preset(Preset::Red)));
        assert_eq!(" Teal ".parse(), Ok(TileColor::Preset(Preset::Teal)));
        assert_eq!("#FF8800".parse(), Ok(TileColor::Custom([255, 136, 0])));
        for preset in Preset::ALL {
            let color = TileColor::Preset(preset);
            assert_eq!(color.to_string().parse(), Ok(color));
        }
        assert_eq!(TileColor::Custom([255, 136, 0]).to_string(), "#ff8800");
    }

    #[test]
    fn invalid_colors_are_rejected() {
        for text in [
            "", "magenta", "#ff88", "#ff88000", "#gg8800", "ff8800", "#ff88é",
        ] {
            assert!(text.parse::<TileColor>().is_err(), "{text:?}");
        }
    }

    #[test]
    fn invalid_colors_in_the_config_are_dropped() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default, deserialize_with = "deserialize_lenient")]
            color: Option<TileColor>,
        }
        let parse = |text: &str| toml::from_str::<Wrapper>(text).expect("valid toml").color;
        assert_eq!(
            parse(r#"color = "pink""#),
            Some(TileColor::Preset(Preset::Pink))
        );
        assert_eq!(
            parse(r##"color = "#010203""##),
            Some(TileColor::Custom([1, 2, 3]))
        );
        assert_eq!(parse(r#"color = "nope""#), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn text_is_dark_on_light_fills_and_light_on_dark_ones() {
        assert!(wants_dark_text([255, 255, 255]));
        assert!(!wants_dark_text([0, 0, 0]));
        assert!(wants_dark_text([255, 230, 0]), "yellow");
        assert!(!wants_dark_text([20, 40, 160]), "deep blue");
    }

    #[test]
    fn light_theme_presets_want_dark_text_and_dark_theme_ones_mostly_light() {
        for preset in Preset::ALL {
            assert!(wants_dark_text(preset.rgb(false)), "{preset:?}");
        }
        let light_text = (Preset::ALL.iter())
            .filter(|preset| !wants_dark_text(preset.rgb(true)))
            .count();
        assert!(light_text >= 7, "{light_text}");
    }
}
