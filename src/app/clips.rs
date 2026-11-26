//! Clips: a sound file can be on the board several times, e.g. with different parts of it. Every
//! clip has its own name, trim, volume and triggers; clip 0 is the file's own, more clips live in
//! its [`SoundConfig::clips`]. The file itself is never copied.

use std::path::{Path, PathBuf};

use crate::config::{ClipConfig, SoundConfig};

use super::AppState;

/// A clip on the board: a sound file and which of its clips.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClipId {
    pub path: PathBuf,
    /// 0 is the file's own clip, more clips have the id of their [`ClipConfig`].
    pub clip: u32,
}

impl ClipId {
    pub fn new(path: impl Into<PathBuf>, clip: u32) -> Self {
        Self {
            path: path.into(),
            clip,
        }
    }

    /// The file's own clip.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::new(path, 0)
    }
}

/// The name a clip shows without a name of its own: the file name without extension, and for
/// more clips of the file also their number, e.g. `Airhorn (2)` for clip 1.
pub fn default_name(file_stem: &str, clip: u32) -> String {
    match clip {
        0 => file_stem.to_owned(),
        clip => format!("{file_stem} ({})", u64::from(clip) + 1),
    }
}

/// The file name without extension.
pub fn file_stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// `<name> (2)`, or the next number that none of `taken` has.
fn numbered_name(name: &str, taken: &[String]) -> String {
    (2_u64..)
        .map(|n| format!("{name} ({n})"))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| name.to_owned())
}

impl AppState {
    /// The clips of the sound file at `path` in id order, starting with its own.
    pub fn clips_of(&self, path: &Path) -> Vec<ClipId> {
        let Some(config) = self.sound_config(&ClipId::file(path)) else {
            return Vec::new();
        };
        let ids = config.with_clips().map(|(clip, _)| clip);
        ids.map(|clip| ClipId::new(path, clip)).collect()
    }

    /// Adds a clip of the same file as `source` with its name (numbered), volume and trim, but
    /// without its triggers: two tiles never share one. Returns the new clip.
    pub fn duplicate_clip(&mut self, source: &ClipId) -> Option<ClipId> {
        let copied = self.sound_config(source)?;
        let taken: Vec<String> = (self.clips_of(&source.path).iter())
            .map(|clip| self.display_name(clip))
            .collect();
        let name = numbered_name(&self.display_name(source), &taken);
        let file = self.sound_config_mut(&ClipId::file(&source.path))?;
        let id = file.next_clip_id();
        file.clips.push(ClipConfig {
            id,
            config: SoundConfig {
                name: Some(name),
                volume: copied.volume,
                start_ms: copied.start_ms,
                end_ms: copied.end_ms,
                ..SoundConfig::default()
            },
        });
        Some(ClipId::new(&source.path, id))
    }

    /// Removes one of the more clips of a file; returns whether it did. The file's own clip stays
    /// as long as the file is in the folder.
    pub fn delete_clip(&mut self, clip: &ClipId) -> bool {
        if clip.clip == 0 {
            return false;
        }
        let Some(file) = self.sound_config_mut(&ClipId::file(&clip.path)) else {
            return false;
        };
        let before = file.clips.len();
        file.clips.retain(|extra| extra.id != clip.clip);
        file.clips.len() != before
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::config::{Config, TabConfig};
    use crate::midi::{MidiBinding, MidiControl};

    struct Fixture {
        dir: tempfile::TempDir,
        state: AppState,
    }

    impl Fixture {
        fn clip(&self, clip: u32) -> ClipId {
            ClipId::new(self.dir.path().join("Airhorn.wav"), clip)
        }
    }

    /// One tab with Airhorn: trimmed, half volume, with a hotkey and a MIDI binding.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("Airhorn.wav"), b"").expect("write");
        let mut tab = TabConfig::for_folder(dir.path().to_owned());
        tab.sounds.insert(
            "Airhorn.wav".to_owned(),
            SoundConfig {
                volume: 0.5,
                start_ms: 100,
                end_ms: Some(900),
                hotkey: "Ctrl+F13".parse().ok(),
                shortcut_id: Some("play-airhorn".to_owned()),
                midi: Some(MidiBinding {
                    port: None,
                    channel: 1,
                    control: MidiControl::Note(36),
                }),
                ..SoundConfig::default()
            },
        );
        let config = Config {
            tabs: vec![tab],
            ..Config::default()
        };
        Fixture {
            state: AppState::new(config),
            dir,
        }
    }

    #[test]
    fn default_names_number_the_clips() {
        assert_eq!(default_name("Airhorn", 0), "Airhorn");
        assert_eq!(default_name("Airhorn", 1), "Airhorn (2)");
        assert_eq!(default_name("Airhorn", u32::MAX), "Airhorn (4294967296)");
    }

    #[test]
    fn numbered_names_skip_taken_numbers() {
        let taken = ["Airhorn (2)".to_owned(), "Airhorn (3)".to_owned()];
        assert_eq!(numbered_name("Airhorn", &[]), "Airhorn (2)");
        assert_eq!(numbered_name("Airhorn", &taken), "Airhorn (4)");
    }

    #[test]
    fn duplicates_copy_name_volume_and_trim_but_not_triggers() {
        let mut f = fixture();
        let copy = f.state.duplicate_clip(&f.clip(0)).expect("duplicated");
        assert_eq!(copy, f.clip(1));
        assert_eq!(
            f.state.sound_config(&copy),
            Some(SoundConfig {
                name: Some("Airhorn (2)".to_owned()),
                volume: 0.5,
                start_ms: 100,
                end_ms: Some(900),
                ..SoundConfig::default()
            })
        );
        let original = f.state.sound_config(&f.clip(0)).expect("still there");
        assert_eq!(original.shortcut_id.as_deref(), Some("play-airhorn"));
        assert!(original.midi.is_some());
        assert_eq!(f.state.clips_of(&f.clip(0).path), [f.clip(0), f.clip(1)]);
    }

    #[test]
    fn duplicates_get_the_next_free_number_and_id() {
        let mut f = fixture();
        f.state.duplicate_clip(&f.clip(0));
        let second = f.state.duplicate_clip(&f.clip(0)).expect("duplicated");
        assert_eq!(second, f.clip(2));
        assert_eq!(f.state.display_name(&second), "Airhorn (3)");
        let of_copy = f.state.duplicate_clip(&f.clip(1)).expect("duplicated");
        assert_eq!(f.state.display_name(&of_copy), "Airhorn (2) (2)");

        assert!(f.state.delete_clip(&f.clip(1)));
        let after_delete = f.state.duplicate_clip(&f.clip(0)).expect("duplicated");
        assert_eq!(
            after_delete,
            f.clip(4),
            "ids are never reused while higher ones exist"
        );
        assert_eq!(f.state.display_name(&after_delete), "Airhorn (2)");
    }

    #[test]
    fn only_more_clips_can_be_deleted() {
        let mut f = fixture();
        f.state.duplicate_clip(&f.clip(0));
        assert!(!f.state.delete_clip(&f.clip(0)), "the file's own clip");
        assert!(!f.state.delete_clip(&f.clip(7)), "no such clip");
        assert!(f.state.delete_clip(&f.clip(1)));
        assert_eq!(f.state.sound_config(&f.clip(1)), None);
        assert_eq!(f.state.clips_of(&f.clip(0).path), [f.clip(0)]);
        let gone = ClipId::new("/gone.wav", 1);
        assert!(f.state.duplicate_clip(&gone).is_none());
        assert!(!f.state.delete_clip(&gone));
    }
}
