//! Tile colors, set per clip from the tile's menu or in "Edit sound".

use crate::color::TileColor;

use super::AppState;
use super::clips::ClipId;

impl AppState {
    /// The color of the clip's tile, if it has one.
    pub fn tile_color(&self, clip: &ClipId) -> Option<TileColor> {
        self.sound_config(clip)?.color
    }

    /// Colors the clip's tile (`None`: the theme's default); returns whether it changed.
    pub fn set_color(&mut self, clip: &ClipId, color: Option<TileColor>) -> bool {
        match self.sound_config_mut(clip) {
            Some(config) if config.color != color => {
                config.color = color;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::color::Preset;
    use crate::config::{Config, TabConfig};

    #[test]
    fn colors_are_set_per_clip() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("Airhorn.wav"), b"").expect("write");
        let config = Config {
            tabs: vec![TabConfig::for_folder(dir.path().to_owned())],
            ..Config::default()
        };
        let mut state = AppState::new(config);
        let airhorn = ClipId::file(dir.path().join("Airhorn.wav"));
        let red = Some(TileColor::Preset(Preset::Red));

        assert_eq!(state.tile_color(&airhorn), None);
        assert!(state.set_color(&airhorn, red));
        assert!(!state.set_color(&airhorn, red), "unchanged");
        let copy = state.duplicate_clip(&airhorn).expect("duplicated");
        assert!(state.set_color(&copy, Some(TileColor::Custom([1, 2, 3]))));
        assert_eq!(state.tile_color(&airhorn), red);
        assert_eq!(state.config.tabs[0].sounds["Airhorn.wav"].color, red);
        assert!(state.set_color(&airhorn, None));
        assert_eq!(state.tile_color(&copy), Some(TileColor::Custom([1, 2, 3])));
        assert!(!state.set_color(&ClipId::file("/gone.wav"), red));
    }
}
