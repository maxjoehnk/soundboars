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
use crate::hotkeys::{self, BackendKind, Hotkey};
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
}

impl TabConfig {
    /// A tab for `path`, named after the folder.
    pub fn for_folder(path: PathBuf) -> Self {
        Self {
            name: folder_name(&path),
            path,
            sounds: BTreeMap::new(),
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

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundConfig {
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
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            volume: 1.0,
            start_ms: 0,
            end_ms: None,
            hotkey: None,
            shortcut_id: None,
            midi: None,
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
            },
        );
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
