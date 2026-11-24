//! What the UI shows, and the decisions behind it, free of egui and I/O so it can be unit-tested.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::audio::{PlayRequest, Playing};
use crate::config::Config;
use crate::library::{Library, Sound};

use super::targets::Targets;

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
    /// The (trimmed) length of sounds that played before.
    pub durations: HashMap<PathBuf, Duration>,
    /// `false` when PipeWire isn't available: nothing can be played or linked.
    pub audio_available: bool,
    pub settings_open: bool,
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
        let (tab, sound) = self.library.find(path)?;
        let meta = self.config.tabs.get(tab)?.sounds.get(&sound.key);
        Some(PlayRequest {
            path: sound.path.clone(),
            trim: meta.and_then(|meta| meta.trim()),
            volume: meta.map_or(1.0, |meta| meta.volume),
        })
    }

    /// How far the most recently started voice of `path` is, if it plays.
    pub fn progress(&self, path: &Path, now: Instant) -> Option<f32> {
        self.playing
            .iter()
            .filter(|playing| playing.path == path)
            .map(|playing| playing.progress(now))
            .reduce(f32::min)
    }

    /// The names of the playing sounds, oldest first.
    pub fn now_playing(&self) -> Vec<String> {
        self.playing
            .iter()
            .map(|playing| match self.library.find(&playing.path) {
                Some((_, sound)) => sound.name.clone(),
                None => file_stem(&playing.path),
            })
            .collect()
    }

    /// The second line of a tile: the length once known, else the file format.
    pub fn subtitle(&self, sound: &Sound) -> String {
        match self.durations.get(&sound.path) {
            Some(duration) => format_duration(*duration),
            None => sound.format(),
        }
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
    use crate::config::{SoundConfig, TabConfig};

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
            })
        );
        assert_eq!(
            f.state.play_request(&bruh),
            Some(PlayRequest {
                path: bruh,
                trim: None,
                volume: 1.0,
            })
        );
        assert_eq!(f.state.play_request(Path::new("/gone.wav")), None);
    }

    #[test]
    fn progress_and_names_of_playing_sounds() {
        let mut f = fixture();
        let airhorn = f.state.visible_sounds()[0].path.clone();
        let now = Instant::now();
        let voice = |voice, started_at| Playing {
            voice,
            path: airhorn.clone(),
            started_at,
            duration: Duration::from_secs(4),
        };
        f.state.playing = vec![
            voice(0, now - Duration::from_secs(3)),
            voice(1, now - Duration::from_secs(1)),
        ];

        assert_eq!(f.state.progress(&airhorn, now), Some(0.25));
        assert_eq!(f.state.progress(Path::new("/other.wav"), now), None);
        assert_eq!(f.state.now_playing(), ["Airhorn", "Airhorn"]);
    }

    #[test]
    fn subtitles_show_the_length_once_known() {
        let mut f = fixture();
        let sound = f.state.visible_sounds()[0].clone();
        assert_eq!(f.state.subtitle(&sound), "MP3");
        f.state
            .durations
            .insert(sound.path.clone(), Duration::from_millis(62_500));
        assert_eq!(f.state.subtitle(&sound), "1:02.50");
    }
}
