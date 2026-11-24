//! What the UI shows, and the decisions behind it, free of egui and I/O so it can be unit-tested.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::audio::{Destination, PlayRequest, Playing};
use crate::config::{Config, SoundConfig};
use crate::library::{Library, Sound};

use super::midi::MidiState;
use super::sound_editor::SoundEditor;
use super::targets::Targets;
use super::url_import::UrlImport;

#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    /// The contents of the tabs in `config.tabs`, in the same order.
    pub library: Library,
    pub tab: usize,
    pub search: String,
    pub targets: Targets,
    /// Shown in the error bar until dismissed.
    pub errors: Vec<String>,
    /// What plays right now, refreshed every frame.
    pub playing: Vec<Playing>,
    /// The lengths of the (untrimmed) sound files, once known.
    pub durations: HashMap<PathBuf, Duration>,
    /// `false` when PipeWire isn't available: nothing can be played or linked.
    pub audio_available: bool,
    pub settings_open: bool,
    /// The "Edit sound" window, while open.
    pub editor: Option<SoundEditor>,
    pub midi: MidiState,
    /// The "Add sound from URL" window, while open.
    pub url_import: Option<UrlImport>,
    /// Why sounds can't be added from URLs (e.g. yt-dlp isn't installed).
    pub url_import_unavailable: Option<String>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let library = Library::new(config.tabs.iter().map(|tab| tab.path.clone()));
        Self {
            config,
            library,
            tab: 0,
            search: String::new(),
            targets: Targets::default(),
            errors: Vec::new(),
            playing: Vec::new(),
            durations: HashMap::new(),
            audio_available: true,
            settings_open: false,
            editor: None,
            midi: MidiState::default(),
            url_import: None,
            url_import_unavailable: None,
        }
    }

    pub fn is_searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    /// The tiles to show: the search results across all tabs while searching, else the sounds of
    /// the selected tab.
    pub fn visible_sounds(&self) -> Vec<&Sound> {
        if self.is_searching() {
            return self.library.search(&self.search);
        }
        self.library
            .folders()
            .get(self.tab)
            .map(|folder| folder.sounds.iter().collect())
            .unwrap_or_default()
    }

    /// Plays the sound at `path` with its volume and trim from the config.
    pub fn play_request(&self, path: &Path) -> Option<PlayRequest> {
        let meta = self.sound_config(path)?;
        Some(PlayRequest {
            path: path.to_owned(),
            trim: meta.trim(),
            volume: meta.volume,
            destination: Destination::Everywhere,
        })
    }

    /// The saved metadata of the sound at `path` (defaults if there is none yet).
    pub fn sound_config(&self, path: &Path) -> Option<SoundConfig> {
        let (tab, sound) = self.library.find(path)?;
        let meta = self.config.tabs.get(tab)?.sounds.get(&sound.key);
        Some(meta.cloned().unwrap_or_default())
    }

    /// The metadata of the sound at `path` to change, created with defaults if missing.
    pub fn sound_config_mut(&mut self, path: &Path) -> Option<&mut SoundConfig> {
        let (tab, sound) = self.library.find(path)?;
        let key = sound.key.clone();
        Some(
            self.config
                .tabs
                .get_mut(tab)?
                .sounds
                .entry(key)
                .or_default(),
        )
    }

    /// The board's sounds that play right now, without previews.
    fn playing_on_board(&self) -> impl Iterator<Item = &Playing> {
        self.playing
            .iter()
            .filter(|playing| playing.destination == Destination::Everywhere)
    }

    /// The preview of the sound at `path`, while it plays.
    pub fn preview(&self, path: &Path) -> Option<&Playing> {
        self.playing
            .iter()
            .rfind(|playing| playing.destination == Destination::Preview && playing.path == path)
    }

    /// How far the most recently started voice of `path` is, if it plays.
    pub fn progress(&self, path: &Path, now: Instant) -> Option<f32> {
        self.playing_on_board()
            .filter(|playing| playing.path == path)
            .map(|playing| playing.progress(now))
            .reduce(f32::min)
    }

    /// The names of the playing sounds, oldest first.
    pub fn now_playing(&self) -> Vec<String> {
        self.playing_on_board()
            .map(|playing| match self.library.find(&playing.path) {
                Some((_, sound)) => sound.name.clone(),
                None => file_stem(&playing.path),
            })
            .collect()
    }

    /// The second line of a tile: the trimmed length once known, else the file format; then what
    /// triggers the sound, e.g. `0:01.20 · Note 36`.
    pub fn subtitle(&self, sound: &Sound) -> String {
        let meta = self.sound_config(&sound.path).unwrap_or_default();
        let length = match self.durations.get(&sound.path) {
            Some(&length) => format_duration(meta.trimmed_length(length)),
            None => sound.format(),
        };
        let midi = meta.midi.map(|binding| binding.control.to_string());
        let parts: Vec<String> = std::iter::once(length).chain(midi).collect();
        parts.join(" · ")
    }

    /// Selects a tab, keeping the selection in range after tabs were removed.
    pub fn select_tab(&mut self, tab: usize) {
        self.tab = tab.min(self.config.tabs.len().saturating_sub(1));
    }

    pub fn tab_index(&self, dir: &Path) -> Option<usize> {
        self.config.tabs.iter().position(|tab| tab.path == dir)
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// `m:ss.cc`
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs_f32();
    format!("{}:{:05.2}", duration.as_secs() / 60, secs % 60.0)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::audio::Trim;
    use crate::config::TabConfig;
    use crate::midi::{MidiBinding, MidiControl};

    struct Fixture {
        _dirs: [tempfile::TempDir; 2],
        state: AppState,
    }

    /// Two tabs: `Memes` (Airhorn, Bruh) and `SFX` (Applause, Horn).
    fn fixture() -> Fixture {
        let memes = tempfile::tempdir().expect("temp dir");
        let sfx = tempfile::tempdir().expect("temp dir");
        for (dir, name) in [
            (&memes, "Airhorn.mp3"),
            (&memes, "Bruh.wav"),
            (&sfx, "Applause.ogg"),
            (&sfx, "Horn.flac"),
        ] {
            fs::write(dir.path().join(name), b"").expect("write");
        }
        let mut memes_tab = TabConfig::for_folder(memes.path().to_owned());
        memes_tab.sounds.insert(
            "Airhorn.mp3".to_owned(),
            SoundConfig {
                volume: 0.5,
                start_ms: 100,
                end_ms: Some(900),
                ..SoundConfig::default()
            },
        );
        let config = Config {
            tabs: vec![memes_tab, TabConfig::for_folder(sfx.path().to_owned())],
            ..Config::default()
        };
        Fixture {
            state: AppState::new(config),
            _dirs: [memes, sfx],
        }
    }

    fn names(sounds: Vec<&Sound>) -> Vec<&str> {
        sounds.iter().map(|sound| sound.name.as_str()).collect()
    }

    #[test]
    fn shows_the_selected_tab() {
        let mut f = fixture();
        assert_eq!(names(f.state.visible_sounds()), ["Airhorn", "Bruh"]);
        f.state.select_tab(1);
        assert_eq!(names(f.state.visible_sounds()), ["Applause", "Horn"]);
    }

    #[test]
    fn searching_shows_matches_from_all_tabs() {
        let mut f = fixture();
        f.state.search = "horn".to_owned();
        assert_eq!(names(f.state.visible_sounds()), ["Airhorn", "Horn"]);
        f.state.search = "  ".to_owned();
        assert_eq!(names(f.state.visible_sounds()), ["Airhorn", "Bruh"]);
    }

    #[test]
    fn tab_selection_stays_in_range() {
        let mut f = fixture();
        f.state.select_tab(5);
        assert_eq!(f.state.tab, 1);
        f.state.config.tabs.clear();
        f.state.library = Library::default();
        f.state.select_tab(1);
        assert_eq!(f.state.tab, 0);
        assert!(f.state.visible_sounds().is_empty());
    }

    #[test]
    fn plays_use_the_volume_and_trim_from_the_config() {
        let f = fixture();
        let airhorn = f.state.visible_sounds()[0].path.clone();
        let bruh = f.state.visible_sounds()[1].path.clone();

        assert_eq!(
            f.state.play_request(&airhorn),
            Some(PlayRequest {
                path: airhorn,
                trim: Some(Trim {
                    start_ms: 100,
                    end_ms: Some(900)
                }),
                volume: 0.5,
                destination: Destination::Everywhere,
            })
        );
        assert_eq!(
            f.state.play_request(&bruh),
            Some(PlayRequest {
                path: bruh,
                trim: None,
                volume: 1.0,
                destination: Destination::Everywhere,
            })
        );
        assert_eq!(f.state.play_request(Path::new("/gone.wav")), None);
    }

    #[test]
    fn sound_metadata_is_created_on_first_change() {
        let mut f = fixture();
        let bruh = f.state.visible_sounds()[1].path.clone();
        assert_eq!(f.state.sound_config(&bruh), Some(SoundConfig::default()));
        f.state.sound_config_mut(&bruh).expect("in a tab").volume = 0.3;
        assert_eq!(f.state.config.tabs[0].sounds["Bruh.wav"].volume, 0.3);
        assert!(f.state.sound_config_mut(Path::new("/gone.wav")).is_none());
    }

    #[test]
    fn progress_and_names_of_playing_sounds() {
        let mut f = fixture();
        let airhorn = f.state.visible_sounds()[0].path.clone();
        let now = Instant::now();
        let voice = |voice, started_at, destination| Playing {
            voice,
            path: airhorn.clone(),
            started_at,
            duration: Duration::from_secs(4),
            destination,
        };
        f.state.playing = vec![
            voice(0, now - Duration::from_secs(3), Destination::Everywhere),
            voice(1, now - Duration::from_secs(1), Destination::Everywhere),
            voice(2, now, Destination::Preview),
        ];

        assert_eq!(f.state.progress(&airhorn, now), Some(0.25));
        assert_eq!(f.state.progress(Path::new("/other.wav"), now), None);
        assert_eq!(f.state.now_playing(), ["Airhorn", "Airhorn"]);
        assert_eq!(
            f.state.preview(&airhorn).map(|preview| preview.voice),
            Some(2)
        );
    }

    #[test]
    fn subtitles_show_the_length_once_known() {
        let mut f = fixture();
        let [airhorn, bruh] = [0, 1].map(|index| f.state.visible_sounds()[index].clone());
        assert_eq!(f.state.subtitle(&airhorn), "MP3");
        for sound in [&airhorn, &bruh] {
            f.state
                .durations
                .insert(sound.path.clone(), Duration::from_millis(62_500));
        }
        assert_eq!(f.state.subtitle(&bruh), "1:02.50");
        assert_eq!(
            f.state.subtitle(&airhorn),
            "0:00.80",
            "trimmed to 0.1–0.9 s"
        );
    }

    #[test]
    fn subtitles_show_the_midi_binding() {
        let mut f = fixture();
        let bruh = f.state.visible_sounds()[1].clone();
        f.state.sound_config_mut(&bruh.path).expect("in a tab").midi = Some(MidiBinding {
            port: None,
            channel: 1,
            control: MidiControl::ControlChange(20),
        });
        assert_eq!(f.state.subtitle(&bruh), "WAV · CC 20");
    }
}
