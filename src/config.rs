//! The config file (`$XDG_CONFIG_HOME/soundboars/config.toml`): settings, the remembered target
//! app, the tabs and per-sound metadata. Every struct uses `#[serde(default)]`, so files written
//! by older or newer versions load: missing fields get defaults, unknown fields are ignored.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::audio::{RecordingApp, Retrigger, Trim};
use crate::color::{self, TileColor};
use crate::hotkeys::{self, BackendKind, Hotkey};
use crate::import::{self, BrowserCookies};
use crate::midi::{MidiBinding, MidiDevice};

/// Overrides where the config file lives (e.g. for tests).
pub const CONFIG_PATH_ENV: &str = "SOUNDBOARS_CONFIG";

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// How sounds get into the call.
    pub injection: Injection,
    pub settings: Settings,
    /// The app sounds are injected into with [`Injection::App`]; relinked whenever it
    /// (re)appears.
    pub target: Option<SavedTarget>,
    /// In display order.
    pub tabs: Vec<TabConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub call_volume: f32,
    pub local_volume: f32,
    pub local_enabled: bool,
    pub retrigger: Retrigger,
    pub theme: Theme,
    /// `None` picks the default for the session ([`BackendKind::default_for_session`]).
    pub hotkey_backend: Option<BackendKind>,
    #[serde(deserialize_with = "hotkeys::hotkey::deserialize_lenient")]
    pub stop_all_hotkey: Option<Hotkey>,
    /// The MIDI ports to listen to.
    pub midi_input: MidiDevice,
    /// Stops all sounds.
    pub midi_stop_all: Option<MidiBinding>,
    /// The browser whose cookies URL imports use (e.g. for age-restricted videos).
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "import::cookies::deserialize_lenient"
    )]
    pub cookies_from_browser: Option<BrowserCookies>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            call_volume: 1.0,
            local_volume: 1.0,
            local_enabled: true,
            retrigger: Retrigger::default(),
            theme: Theme::default(),
            hotkey_backend: None,
            stop_all_hotkey: None,
            midi_input: MidiDevice::default(),
            midi_stop_all: None,
            cookies_from_browser: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

/// How sounds get into the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Injection {
    /// Through the virtual microphone "SoundBoars Microphone", which also carries the user's real
    /// microphone; call apps use it as their input device.
    #[default]
    VirtualMic,
    /// Straight into the recording stream of [`Config::target`].
    App,
}

/// Identifies a recording app across restarts (its PipeWire node id changes every time).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedTarget {
    /// `application.process.binary`
    pub process_binary: Option<String>,
    /// `application.name`
    pub app_name: String,
}

impl SavedTarget {
    pub fn of(app: &RecordingApp) -> Self {
        Self {
            process_binary: app.process_binary.clone(),
            app_name: app.app_name.clone(),
        }
    }

    pub fn matches(&self, app: &RecordingApp) -> bool {
        self.app_name == app.app_name && self.process_binary == app.process_binary
    }
}

/// A tab: a folder whose audio files are the tab's sounds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TabConfig {
    pub name: String,
    pub path: PathBuf,
    /// Keyed by the sound's path relative to [`Self::path`] (with `/` separators).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub sounds: BTreeMap<String, SoundConfig>,
    /// The order of the tiles, as dragged; empty sorts them by file name. Clips that aren't listed
    /// follow in that order. Entries of files that are gone are kept (they may come back).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<ClipKey>,
}

impl TabConfig {
    /// A tab for `path`, named after the folder.
    pub fn for_folder(path: PathBuf) -> Self {
        Self {
            name: folder_name(&path),
            path,
            sounds: BTreeMap::new(),
            order: Vec::new(),
        }
    }

    /// The name to show: [`Self::name`], or the folder's name if that's empty.
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            folder_name(&self.path)
        } else {
            self.name.clone()
        }
    }
}

/// A clip in a tab's [`TabConfig::order`]: the sound's key in [`TabConfig::sounds`] and the clip id
/// (0 for the file's own clip). Written as the key, plus `#<id>` for more clips:
/// `"Airhorn.wav"`, `"Airhorn.wav#2"`. Keys end with an audio file extension, so a `#<digits>`
/// suffix is never part of one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClipKey {
    pub file: String,
    pub clip: u32,
}

impl ClipKey {
    pub fn new(file: impl Into<String>, clip: u32) -> Self {
        Self {
            file: file.into(),
            clip,
        }
    }
}

impl std::fmt::Display for ClipKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.clip {
            0 => f.write_str(&self.file),
            clip => write!(f, "{}#{clip}", self.file),
        }
    }
}

impl From<&str> for ClipKey {
    fn from(text: &str) -> Self {
        let numbered = text
            .rsplit_once('#')
            .and_then(|(file, clip)| Some((file, clip.parse().ok()?)));
        match numbered {
            Some((file, clip)) => Self::new(file, clip),
            None => Self::new(text, 0),
        }
    }
}

impl Serialize for ClipKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ClipKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from(String::deserialize(deserializer)?.as_str()))
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundConfig {
    /// The name shown instead of the file name; the file itself is never renamed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The sound's own volume; `1.0` plays it as it is.
    pub volume: f32,
    #[serde(skip_serializing_if = "is_zero")]
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "hotkeys::hotkey::deserialize_lenient"
    )]
    pub hotkey: Option<Hotkey>,
    /// The global shortcut's id (a slug of the file name, unique in the config); set while the
    /// sound has a global shortcut. Users bind it in their compositor's config, so it never
    /// changes once assigned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut_id: Option<String>,
    pub midi: Option<MidiBinding>,
    /// The tile's color; `None` is the theme's button color.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "color::deserialize_lenient"
    )]
    pub color: Option<TileColor>,
    /// More clips of the same file, e.g. other parts of it, each a tile of its own. The fields
    /// above are the file's own clip (id 0).
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_clips"
    )]
    pub clips: Vec<ClipConfig>,
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            name: None,
            volume: 1.0,
            start_ms: 0,
            end_ms: None,
            hotkey: None,
            shortcut_id: None,
            midi: None,
            color: None,
            clips: Vec::new(),
        }
    }
}

impl SoundConfig {
    /// The part of the file to play, or `None` for all of it.
    pub fn trim(&self) -> Option<Trim> {
        (self.start_ms > 0 || self.end_ms.is_some()).then_some(Trim {
            start_ms: self.start_ms,
            end_ms: self.end_ms,
        })
    }

    /// How long the trimmed part of a sound of `length` is.
    pub fn trimmed_length(&self, length: Duration) -> Duration {
        let end = self
            .end_ms
            .map_or(length, Duration::from_millis)
            .min(length);
        end.saturating_sub(Duration::from_millis(self.start_ms))
    }

    /// The file's clips with their ids: its own (0, `self`), then [`Self::clips`] in id order.
    pub fn with_clips(&self) -> impl Iterator<Item = (u32, &SoundConfig)> {
        let mut extra: Vec<&ClipConfig> = self.clips.iter().collect();
        extra.sort_by_key(|clip| clip.id);
        let extra = extra.into_iter().map(|clip| (clip.id, &clip.config));
        std::iter::once((0, self)).chain(extra)
    }

    /// The clip with this id: `self` for 0.
    pub fn clip(&self, id: u32) -> Option<&SoundConfig> {
        self.with_clips()
            .find(|&(clip, _)| clip == id)
            .map(|(_, config)| config)
    }

    /// The clip with this id to change: `self` for 0.
    pub fn clip_mut(&mut self, id: u32) -> Option<&mut SoundConfig> {
        if id == 0 {
            return Some(self);
        }
        let clip = self.clips.iter_mut().find(|clip| clip.id == id)?;
        Some(&mut clip.config)
    }

    /// The id for a new clip: one more than the highest so far.
    pub fn next_clip_id(&self) -> u32 {
        self.clips.iter().map(|clip| clip.id).max().unwrap_or(0) + 1
    }
}

/// Another clip of a sound's file ([`SoundConfig::clips`]): its own name, trim, volume and
/// triggers. The file itself is shared, never copied.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipConfig {
    /// At least 1 (0 is the file's own clip) and unique among the file's clips.
    pub id: u32,
    /// Its own [`SoundConfig::clips`] are meaningless and never kept.
    #[serde(flatten)]
    pub config: SoundConfig,
}

/// The extra clips of a sound, without their meaningless nested clips and without clips whose id
/// is 0 or taken (with a warning, instead of rejecting the file).
fn deserialize_clips<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ClipConfig>, D::Error> {
    let mut ids = std::collections::HashSet::new();
    let clips = Vec::<ClipConfig>::deserialize(deserializer)?;
    Ok(clips
        .into_iter()
        .filter(|clip| {
            let valid = clip.id > 0 && ids.insert(clip.id);
            if !valid {
                tracing::warn!(
                    "ignoring a clip in the config: its id {} is invalid",
                    clip.id
                );
            }
            valid
        })
        .map(|mut clip| {
            clip.config.clips.clear();
            clip
        })
        .collect())
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error(
        "{path} is invalid, starting with default settings (the old file was kept as {backup}): {source}"
    )]
    Corrupt {
        path: PathBuf,
        backup: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("could not serialize the config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("no config directory: neither $XDG_CONFIG_HOME nor $HOME is set")]
    NoConfigDir,
}

/// Loads and saves the config file.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `$SOUNDBOARS_CONFIG`, or `$XDG_CONFIG_HOME/soundboars/config.toml`.
    pub fn default_location() -> Result<Self, ConfigError> {
        if let Some(path) = std::env::var_os(CONFIG_PATH_ENV) {
            return Ok(Self::new(path.into()));
        }
        let dirs = directories::ProjectDirs::from("me", "maxjoehnk", "soundboars")
            .ok_or(ConfigError::NoConfigDir)?;
        Ok(Self::new(dirs.config_dir().join("config.toml")))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The saved config. A missing file gives the defaults. An unreadable or invalid file also
    /// gives the defaults, plus the error to show; an invalid file is copied to `<path>.bak`
    /// first, so saving the defaults doesn't lose it.
    pub fn load(&self) -> (Config, Option<ConfigError>) {
        match self.try_load() {
            Ok(config) => (config, None),
            Err(error) => (Config::default(), Some(error)),
        }
    }

    fn try_load(&self) -> Result<Config, ConfigError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| self.back_up_corrupt(source))
    }

    fn back_up_corrupt(&self, source: toml::de::Error) -> ConfigError {
        let backup = with_suffix(&self.path, ".bak");
        if let Err(error) = fs::copy(&self.path, &backup) {
            return ConfigError::Write {
                path: backup,
                source: error,
            };
        }
        ConfigError::Corrupt {
            path: self.path.clone(),
            backup,
            source: Box::new(source),
        }
    }

    /// Writes the config to a temporary file and renames it over the real one, so a crash never
    /// leaves a half-written config behind.
    pub fn save(&self, config: &Config) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(config)?;
        let write_error = |source| ConfigError::Write {
            path: self.path.clone(),
            source,
        };
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(write_error)?;
        }
        let temp = with_suffix(&self.path, ".tmp");
        fs::write(&temp, text).map_err(write_error)?;
        fs::rename(&temp, &self.path).map_err(write_error)
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::Browser;
    use crate::midi::MidiControl;

    fn store() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ConfigStore::new(dir.path().join("sub/config.toml"));
        (dir, store)
    }

    #[test]
    fn trimmed_length_is_clamped_to_the_sound() {
        let length = Duration::from_millis(1_000);
        let trimmed = |start_ms, end_ms| {
            let meta = SoundConfig {
                start_ms,
                end_ms,
                ..SoundConfig::default()
            };
            meta.trimmed_length(length).as_millis()
        };
        assert_eq!(trimmed(0, None), 1_000);
        assert_eq!(trimmed(250, Some(750)), 500);
        assert_eq!(trimmed(250, Some(5_000)), 750);
        assert_eq!(trimmed(5_000, None), 0);
    }

    fn sample() -> Config {
        let mut tab = TabConfig::for_folder("/home/me/Sounds/Memes".into());
        tab.sounds.insert(
            "airhorn.mp3".to_owned(),
            SoundConfig {
                name: Some("Air horn".to_owned()),
                volume: 0.5,
                start_ms: 250,
                end_ms: Some(1500),
                hotkey: "Ctrl+Shift+F13".parse().ok(),
                shortcut_id: Some("play-airhorn".to_owned()),
                midi: Some(MidiBinding {
                    port: Some("MPD218:MPD218 MIDI 1".to_owned()),
                    channel: 10,
                    control: MidiControl::Note(36),
                }),
                color: "red".parse().ok(),
                clips: vec![ClipConfig {
                    id: 2,
                    config: SoundConfig {
                        name: Some("Air horn (end)".to_owned()),
                        volume: 0.75,
                        start_ms: 1500,
                        end_ms: None,
                        hotkey: "Ctrl+F14".parse().ok(),
                        shortcut_id: Some("play-airhorn-2".to_owned()),
                        midi: Some(MidiBinding {
                            port: None,
                            channel: 10,
                            control: MidiControl::Note(37),
                        }),
                        color: Some(TileColor::Custom([255, 136, 0])),
                        clips: Vec::new(),
                    },
                }],
            },
        );
        tab.order = vec![
            ClipKey::new("airhorn.mp3", 2),
            ClipKey::new("gone#1.wav", 0),
            ClipKey::new("airhorn.mp3", 0),
        ];
        Config {
            injection: Injection::App,
            settings: Settings {
                call_volume: 0.8,
                local_volume: 0.25,
                local_enabled: false,
                retrigger: Retrigger::Restart,
                theme: Theme::Light,
                hotkey_backend: Some(BackendKind::X11),
                stop_all_hotkey: "Ctrl+Pause".parse().ok(),
                midi_input: MidiDevice::Port("MPD218:MPD218 MIDI 1".to_owned()),
                midi_stop_all: Some(MidiBinding {
                    port: None,
                    channel: 1,
                    control: MidiControl::ControlChange(20),
                }),
                cookies_from_browser: Some(BrowserCookies::new(
                    Browser::Firefox,
                    Some("~/.librewolf/abc.default".to_owned()),
                )),
            },
            target: Some(SavedTarget {
                process_binary: Some("Discord".to_owned()),
                app_name: "WEBRTC VoiceEngine".to_owned(),
            }),
            tabs: vec![tab, TabConfig::for_folder("/home/me/Sounds/SFX".into())],
        }
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let (_dir, store) = store();
        let (config, error) = store.load();
        assert_eq!(config, Config::default());
        assert!(error.is_none());
        assert_eq!(config.settings.call_volume, 1.0);
        assert!(config.settings.local_enabled);
    }

    #[test]
    fn saved_configs_load_unchanged() {
        let (_dir, store) = store();
        store.save(&sample()).expect("save");
        let (config, error) = store.load();
        assert!(error.is_none(), "{error:?}");
        assert_eq!(config, sample());
        assert!(!store.path().with_extension("toml.tmp").exists());
    }

    #[test]
    fn the_file_is_readable_toml() {
        let (_dir, store) = store();
        store.save(&sample()).expect("save");
        let text = fs::read_to_string(store.path()).expect("read");
        assert!(text.contains("retrigger = \"restart\""), "{text}");
        assert!(text.contains("theme = \"light\""), "{text}");
        assert!(text.contains("[tabs.sounds.\"airhorn.mp3\"]"), "{text}");
        assert!(text.contains("call_volume = 0.8"), "{text}");
        assert!(text.contains("hotkey_backend = \"x11\""), "{text}");
        assert!(text.contains("hotkey = \"Ctrl+Shift+F13\""), "{text}");
        assert!(text.contains("shortcut_id = \"play-airhorn\""), "{text}");
        assert!(text.contains("note = 36"), "{text}");
        assert!(text.contains("injection = \"app\""), "{text}");
        assert!(
            text.contains(
                "[settings.cookies_from_browser]\nbrowser = \"firefox\"\n\
                 profile = \"~/.librewolf/abc.default\"\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("[[tabs.sounds.\"airhorn.mp3\".clips]]"),
            "{text}"
        );
        assert!(text.contains("id = 2"), "{text}");
        assert!(text.contains("name = \"Air horn (end)\""), "{text}");
        assert!(text.contains("color = \"red\""), "{text}");
        assert!(text.contains("color = \"#ff8800\""), "{text}");
        let order =
            "order = [\n    \"airhorn.mp3#2\",\n    \"gone#1.wav\",\n    \"airhorn.mp3\",\n]";
        assert!(text.contains(order), "{text}");
    }

    #[test]
    fn clip_keys_read_their_clip_id_from_a_numeric_suffix() {
        assert_eq!(ClipKey::from("a.wav"), ClipKey::new("a.wav", 0));
        assert_eq!(ClipKey::from("a.wav#3"), ClipKey::new("a.wav", 3));
        assert_eq!(ClipKey::from("a#b.wav"), ClipKey::new("a#b.wav", 0));
        assert_eq!(ClipKey::from("a.wav#x"), ClipKey::new("a.wav#x", 0));
        for key in [ClipKey::new("a#1.wav", 0), ClipKey::new("a.wav", 7)] {
            assert_eq!(ClipKey::from(key.to_string().as_str()), key);
        }
    }

    #[test]
    fn cookies_from_browser_are_optional_and_lenient() {
        let (_dir, store) = store();
        store.save(&Config::default()).expect("save");
        let text = fs::read_to_string(store.path()).expect("read");
        assert!(!text.contains("cookies_from_browser"), "{text}");

        let read = |text: &str| {
            let config: Config = toml::from_str(text).expect("valid");
            config.settings.cookies_from_browser
        };
        let chrome = "[settings.cookies_from_browser]\nbrowser = \"chrome\"\nprofile = \"\"";
        assert_eq!(
            read(chrome),
            Some(BrowserCookies::new(Browser::Chrome, None))
        );
        let unknown = "[settings]\ntheme = \"light\"\n\
                       [settings.cookies_from_browser]\nbrowser = \"netscape\"";
        let config: Config = toml::from_str(unknown).expect("an unknown browser is dropped");
        assert_eq!(config.settings.cookies_from_browser, None);
        assert_eq!(config.settings.theme, Theme::Light);
    }

    #[test]
    fn invalid_colors_are_dropped_instead_of_rejecting_the_file() {
        let text = r#"
            [[tabs]]
            path = "/sounds"
            [tabs.sounds."a.wav"]
            color = "chartreuse-ish"
            volume = 0.5
        "#;
        let config: Config = toml::from_str(text).expect("valid");
        let sound = &config.tabs[0].sounds["a.wav"];
        assert_eq!((sound.color, sound.volume), (None, 0.5));
    }

    #[test]
    fn configs_without_clips_load_and_save_unchanged() {
        let (_dir, store) = store();
        let mut config = sample();
        config.tabs[0]
            .sounds
            .get_mut("airhorn.mp3")
            .expect("in the sample")
            .clips
            .clear();
        store.save(&config).expect("save");
        let text = fs::read_to_string(store.path()).expect("read");
        assert!(!text.contains("clips"), "{text}");
        assert_eq!(store.load().0, config);
        store.save(&store.load().0).expect("save again");
        assert_eq!(fs::read_to_string(store.path()).expect("read"), text);
    }

    #[test]
    fn invalid_and_nested_clips_are_ignored() {
        let (_dir, store) = store();
        fs::create_dir_all(store.path().parent().expect("parent")).expect("mkdir");
        let text = r#"
            [[tabs]]
            path = "/sounds"
            [[tabs.sounds."a.wav".clips]]
            id = 1
            start_ms = 100
            hotkey = "Ctrl+Nope"
            [[tabs.sounds."a.wav".clips.clips]]
            id = 5
            [[tabs.sounds."a.wav".clips]]
            id = 1
            [[tabs.sounds."a.wav".clips]]
            id = 0
            [[tabs.sounds."a.wav".clips]]
            name = "no id"
        "#;
        fs::write(store.path(), text).expect("write");

        let (config, error) = store.load();

        assert!(error.is_none(), "{error:?}");
        let sound = &config.tabs[0].sounds["a.wav"];
        assert_eq!(
            sound.clips,
            [ClipConfig {
                id: 1,
                config: SoundConfig {
                    start_ms: 100,
                    ..SoundConfig::default()
                },
            }]
        );
    }

    #[test]
    fn clips_are_found_by_id_and_new_ids_follow_the_highest() {
        let mut sound = SoundConfig::default();
        assert_eq!(sound.next_clip_id(), 1);
        for id in [3, 1] {
            sound.clips.push(ClipConfig {
                id,
                config: SoundConfig {
                    start_ms: u64::from(id),
                    ..SoundConfig::default()
                },
            });
        }
        assert_eq!(sound.next_clip_id(), 4);
        let ids: Vec<u32> = sound.with_clips().map(|(id, _)| id).collect();
        assert_eq!(ids, [0, 1, 3], "in id order");
        assert_eq!(sound.clip(3).map(|clip| clip.start_ms), Some(3));
        assert_eq!(sound.clip(2), None);
        sound.clip_mut(1).expect("exists").volume = 0.5;
        assert_eq!(sound.clips[1].config.volume, 0.5);
        sound.clip_mut(0).expect("the file's own").volume = 0.25;
        assert_eq!(sound.volume, 0.25);
    }

    #[test]
    fn injection_defaults_to_the_virtual_mic_and_keeps_the_target() {
        let (_dir, store) = store();
        fs::create_dir_all(store.path().parent().expect("parent")).expect("mkdir");
        let text = r#"
            [target]
            process_binary = "Discord"
            app_name = "WEBRTC VoiceEngine"
        "#;
        fs::write(store.path(), text).expect("write");

        let (config, error) = store.load();

        assert!(error.is_none(), "{error:?}");
        assert_eq!(config.injection, Injection::VirtualMic);
        assert_eq!(
            config.target.map(|target| target.app_name).as_deref(),
            Some("WEBRTC VoiceEngine")
        );
        let virtual_mic = Config::default();
        store.save(&virtual_mic).expect("save");
        let text = fs::read_to_string(store.path()).expect("read");
        assert!(text.contains("injection = \"virtual-mic\""), "{text}");
        assert_eq!(store.load().0, virtual_mic);
    }

    #[test]
    fn a_corrupt_file_is_backed_up_and_the_defaults_are_used() {
        let (_dir, store) = store();
        fs::create_dir_all(store.path().parent().expect("parent")).expect("mkdir");
        fs::write(store.path(), "settings = [this is not toml").expect("write");

        let (config, error) = store.load();

        assert_eq!(config, Config::default());
        assert!(
            matches!(error, Some(ConfigError::Corrupt { .. })),
            "{error:?}"
        );
        let backup = with_suffix(store.path(), ".bak");
        assert_eq!(
            fs::read_to_string(backup).expect("backup"),
            "settings = [this is not toml"
        );
    }

    #[test]
    fn missing_and_unknown_fields_are_tolerated() {
        let (_dir, store) = store();
        fs::create_dir_all(store.path().parent().expect("parent")).expect("mkdir");
        let text = r#"
            future_top_level = 1
            [settings]
            call_volume = 0.5
            future_setting = "x"
            [[tabs]]
            path = "/sounds"
            [tabs.sounds."a.wav"]
            future_field = "Ctrl+F13"
        "#;
        fs::write(store.path(), text).expect("write");

        let (config, error) = store.load();

        assert!(error.is_none(), "{error:?}");
        assert_eq!(config.settings.call_volume, 0.5);
        assert_eq!(config.settings.local_volume, 1.0);
        assert_eq!(config.tabs[0].path, Path::new("/sounds"));
        assert_eq!(config.tabs[0].sounds["a.wav"], SoundConfig::default());
    }

    #[test]
    fn trim_is_only_set_when_the_sound_is_trimmed() {
        assert_eq!(SoundConfig::default().trim(), None);
        let trimmed = SoundConfig {
            start_ms: 0,
            end_ms: Some(900),
            ..SoundConfig::default()
        };
        assert_eq!(
            trimmed.trim(),
            Some(Trim {
                start_ms: 0,
                end_ms: Some(900)
            })
        );
    }

    #[test]
    fn tabs_are_named_after_their_folder() {
        assert_eq!(TabConfig::for_folder("/a/Memes".into()).name, "Memes");
        let unnamed = TabConfig {
            path: "/a/SFX".into(),
            ..TabConfig::default()
        };
        assert_eq!(unnamed.display_name(), "SFX");
    }

    #[test]
    fn saved_targets_match_name_and_binary() {
        let app = RecordingApp {
            node_id: 7,
            app_name: "WEBRTC VoiceEngine".to_owned(),
            process_binary: Some("Discord".to_owned()),
            media_name: None,
            ports: Vec::new(),
        };
        let target = SavedTarget::of(&app);
        assert!(target.matches(&app));
        let other = RecordingApp {
            process_binary: Some("chromium".to_owned()),
            ..app
        };
        assert!(!target.matches(&other));
    }
}
