//! Global hotkeys as the app sees them: which bindings the config asks for, keeping the backend
//! (and its registered bindings) in sync with the config, and handing presses to the `triggers`
//! thread ([`super::triggers`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::config::Config;
use crate::hotkeys::{BackendKind, Binding, Capabilities, EventSink, HotkeyBackend, HotkeyEvent};

use super::services::HotkeyBackends;
use super::triggers::Trigger;

/// The shortcut id of "Stop all".
pub const STOP_ALL_ID: &str = "stop-all";

/// What a binding does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Play(PathBuf),
    StopAll,
}

/// The bindings the config asks for: every sound with a shortcut id, and "Stop all".
pub fn bindings(config: &Config) -> Vec<(Binding, Target)> {
    let mut ids = HashSet::from([STOP_ALL_ID]);
    let mut bindings = vec![(
        Binding {
            id: STOP_ALL_ID.to_owned(),
            description: "Stop all sounds".to_owned(),
            hotkey: config.settings.stop_all_hotkey,
        },
        Target::StopAll,
    )];
    for tab in &config.tabs {
        for (key, sound) in &tab.sounds {
            let Some(id) = &sound.shortcut_id else {
                continue;
            };
            if !ids.insert(id) {
                tracing::warn!("the shortcut id {id} is used twice, ignoring it for {key}");
                continue;
            }
            let binding = Binding {
                id: id.clone(),
                description: format!(
                    "Play {}",
                    sound.name.clone().unwrap_or_else(|| file_stem(key))
                ),
                hotkey: sound.hotkey,
            };
            bindings.push((binding, Target::Play(tab.path.join(key))));
        }
    }
    bindings
}

/// A new shortcut id for the sound `file_name`: `play-<slug>`, made unique in `config` with a
/// number suffix.
pub fn unique_shortcut_id(config: &Config, file_name: &str) -> String {
    let taken: HashSet<&str> = config
        .tabs
        .iter()
        .flat_map(|tab| tab.sounds.values())
        .filter_map(|sound| sound.shortcut_id.as_deref())
        .chain([STOP_ALL_ID])
        .collect();
    let base = format!("play-{}", slug(&file_stem(file_name)));
    (1..)
        .map(|n| match n {
            1 => base.clone(),
            n => format!("{base}-{n}"),
        })
        .find(|id| !taken.contains(id.as_str()))
        .unwrap_or(base)
}

/// Lowercase ASCII letters and digits, everything else collapsed into single dashes.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "sound".to_owned()
    } else {
        slug.to_owned()
    }
}

fn file_stem(file_name: &str) -> String {
    let path = Path::new(file_name);
    path.file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// The hotkey backend, kept in sync with the config.
pub struct Hotkeys {
    backends: Box<dyn HotkeyBackends>,
    sink: EventSink,
    errors: mpsc::Receiver<String>,
    /// The backend kind last started (even if starting failed, so it isn't retried every frame).
    kind: Option<BackendKind>,
    backend: Option<Box<dyn HotkeyBackend>>,
    /// What the backend has registered.
    registered: Vec<Binding>,
}

impl Hotkeys {
    /// Presses go to `triggers`. Errors are polled; `wake` is called from the backends' threads
    /// whenever there is one.
    pub fn new(
        backends: Box<dyn HotkeyBackends>,
        triggers: mpsc::Sender<Trigger>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (error_tx, errors) = mpsc::channel();
        // Sending fails only when the app is gone.
        let sink = EventSink::new(move |event| match event {
            HotkeyEvent::Pressed(id) => {
                let _ = triggers.send(Trigger::Hotkey(id));
            }
            HotkeyEvent::Released(_) => {}
            HotkeyEvent::Error(error) => {
                if error_tx.send(error).is_ok() {
                    wake();
                }
            }
        });
        Self {
            backends,
            sink,
            errors,
            kind: None,
            backend: None,
            registered: Vec::new(),
        }
    }

    /// Starts the backend of `kind` if another one runs, and registers `config`'s bindings if
    /// they changed. Returns why the backend couldn't start.
    pub fn sync(&mut self, kind: BackendKind, config: &Config) -> Option<String> {
        let error = (self.kind != Some(kind))
            .then(|| self.restart(kind))
            .flatten();
        let bindings: Vec<Binding> = bindings(config).into_iter().map(|(b, _)| b).collect();
        if bindings != self.registered {
            if let Some(backend) = &mut self.backend {
                backend.register(&bindings);
            }
            self.registered = bindings;
        }
        error
    }

    fn restart(&mut self, kind: BackendKind) -> Option<String> {
        // Stop the old backend first: the X11 one owns a process-wide event handler.
        self.backend = None;
        self.registered.clear();
        self.kind = Some(kind);
        match self.backends.start(kind, self.sink.clone()) {
            Ok(backend) => {
                self.backend = backend;
                None
            }
            Err(error) => Some(format!("Hotkeys are not available: {error}")),
        }
    }

    pub fn capabilities(&self) -> Capabilities {
        self.backend
            .as_ref()
            .map(|backend| backend.capabilities())
            .unwrap_or_default()
    }

    /// The backends' errors since the last call.
    pub fn errors(&self) -> Vec<String> {
        self.errors.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SoundConfig, TabConfig};

    fn sound(shortcut_id: Option<&str>, hotkey: Option<&str>) -> SoundConfig {
        SoundConfig {
            shortcut_id: shortcut_id.map(str::to_owned),
            hotkey: hotkey.map(|hotkey| hotkey.parse().expect("valid hotkey")),
            ..SoundConfig::default()
        }
    }

    fn config() -> Config {
        let mut memes = TabConfig::for_folder("/sounds/Memes".into());
        memes.sounds.insert(
            "Air horn.mp3".to_owned(),
            sound(Some("play-air-horn"), Some("Ctrl+F13")),
        );
        memes
            .sounds
            .insert("Bruh.wav".to_owned(), sound(None, None));
        let mut sfx = TabConfig::for_folder("/sounds/SFX".into());
        sfx.sounds.insert(
            "Applause.ogg".to_owned(),
            sound(Some("play-applause"), None),
        );
        sfx.sounds
            .insert("Copy.ogg".to_owned(), sound(Some("play-applause"), None));
        let mut config = Config {
            tabs: vec![memes, sfx],
            ..Config::default()
        };
        config.settings.stop_all_hotkey = "Ctrl+Pause".parse().ok();
        config
    }

    #[test]
    fn sounds_with_a_shortcut_id_and_stop_all_are_bound() {
        let bindings = bindings(&config());
        let summary: Vec<(&str, &str, Option<String>, &Target)> = bindings
            .iter()
            .map(|(binding, target)| {
                (
                    binding.id.as_str(),
                    binding.description.as_str(),
                    binding.hotkey.map(|hotkey| hotkey.to_string()),
                    target,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "stop-all",
                    "Stop all sounds",
                    Some("Ctrl+Pause".to_owned()),
                    &Target::StopAll
                ),
                (
                    "play-air-horn",
                    "Play Air horn",
                    Some("Ctrl+F13".to_owned()),
                    &Target::Play("/sounds/Memes/Air horn.mp3".into())
                ),
                (
                    "play-applause",
                    "Play Applause",
                    None,
                    &Target::Play("/sounds/SFX/Applause.ogg".into())
                ),
            ],
            "Bruh has no shortcut; Copy's duplicate id is skipped"
        );
    }

    #[test]
    fn stop_all_is_bound_without_a_hotkey() {
        let bindings = bindings(&Config::default());
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].0.hotkey, None);
    }

    #[test]
    fn shortcut_ids_are_readable_slugs() {
        let config = Config::default();
        let id = |name| unique_shortcut_id(&config, name);
        assert_eq!(id("Air horn.mp3"), "play-air-horn");
        assert_eq!(id("  Vine BOOM!!  (loud).wav"), "play-vine-boom-loud");
        assert_eq!(id("sub/dir/Bruh.wav"), "play-bruh");
        assert_eq!(id("ÄÖÜ.mp3"), "play-sound");
        assert_eq!(id("Über cool.mp3"), "play-ber-cool");
    }

    #[test]
    fn shortcut_ids_are_unique() {
        let mut config = config();
        assert_eq!(
            unique_shortcut_id(&config, "Air horn.wav"),
            "play-air-horn-2"
        );
        config.tabs[0].sounds.insert(
            "Air-horn.ogg".to_owned(),
            sound(Some("play-air-horn-2"), None),
        );
        assert_eq!(
            unique_shortcut_id(&config, "AIR HORN.flac"),
            "play-air-horn-3"
        );
        assert_eq!(unique_shortcut_id(&config, "Stop all.wav"), "play-stop-all");
    }
}
