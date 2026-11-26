//! What the UI shows, and the decisions behind it, free of egui and I/O so it can be unit-tested.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::audio::{Destination, PlayRequest, Playing};
use crate::config::{Config, SoundConfig};
use crate::hotkeys::{BackendKind, Capabilities};
use crate::library::Library;

use super::clips::{ClipId, default_name, file_stem};
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
    /// What the running hotkey backend can do.
    pub hotkey_capabilities: Capabilities,
    /// The hotkey backend used while the settings don't choose one.
    pub default_hotkey_backend: BackendKind,
    /// The settings' "Stop all" hotkey recorder waits for a key combination.
    pub recording_stop_all: bool,
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
            hotkey_capabilities: Capabilities::default(),
            default_hotkey_backend: BackendKind::default_for_session(),
            recording_stop_all: false,
            midi: MidiState::default(),
            url_import: None,
            url_import_unavailable: None,
        }
    }

    pub fn is_searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    /// The tiles to show: the search results across all tabs while searching, else the clips of
    /// the selected tab's sounds (each file's own clip followed by its more clips).
    pub fn visible_clips(&self) -> Vec<ClipId> {
        if self.is_searching() {
            return self.search_results();
        }
        let sounds = self.library.folders().get(self.tab).into_iter();
        (sounds.flat_map(|folder| &folder.sounds))
            .flat_map(|sound| self.clips_of(&sound.path))
            .collect()
    }

    /// Clips of all tabs whose shown name or file name contains the search, ignoring case.
    fn search_results(&self) -> Vec<ClipId> {
        let query = self.search.trim().to_lowercase();
        (self.library.sounds())
            .flat_map(|sound| self.clips_of(&sound.path))
            .filter(|clip| {
                self.display_name(clip).to_lowercase().contains(&query)
                    || file_stem(&clip.path).to_lowercase().contains(&query)
            })
            .collect()
    }

    /// The name on the clip's tile: the one given in "Edit sound", else the file name (numbered
    /// for more clips of the file, see [`default_name`]).
    pub fn display_name(&self, clip: &ClipId) -> String {
        self.sound_config(clip)
            .and_then(|meta| meta.name)
            .unwrap_or_else(|| default_name(&file_stem(&clip.path), clip.clip))
    }

    /// Plays the clip with its volume and trim from the config.
    pub fn play_request(&self, clip: &ClipId) -> Option<PlayRequest> {
        let meta = self.sound_config(clip)?;
        Some(PlayRequest {
            path: clip.path.clone(),
            clip: clip.clip,
            trim: meta.trim(),
            volume: meta.volume,
            destination: Destination::Everywhere,
        })
    }

    /// The saved metadata of the clip (defaults for a file's own clip if there is none yet);
    /// `None` if its file isn't in a tab or there's no such clip.
    pub fn sound_config(&self, clip: &ClipId) -> Option<SoundConfig> {
        let (tab, sound) = self.library.find(&clip.path)?;
        let file = self.config.tabs.get(tab)?.sounds.get(&sound.key);
        match file {
            Some(file) => file.clip(clip.clip).cloned(),
            None => (clip.clip == 0).then(SoundConfig::default),
        }
    }

    /// The metadata of the clip to change; a file's own clip's is created with defaults if
    /// missing.
    pub fn sound_config_mut(&mut self, clip: &ClipId) -> Option<&mut SoundConfig> {
        let (tab, sound) = self.library.find(&clip.path)?;
        let key = sound.key.clone();
        let sounds = &mut self.config.tabs.get_mut(tab)?.sounds;
        match clip.clip {
            0 => Some(sounds.entry(key).or_default()),
            id => sounds.get_mut(&key)?.clip_mut(id),
        }
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

    /// How far the most recently started voice of the clip is, if it plays.
    pub fn progress(&self, clip: &ClipId, now: Instant) -> Option<f32> {
        self.playing_on_board()
            .filter(|playing| playing.path == clip.path && playing.clip == clip.clip)
            .map(|playing| playing.progress(now))
            .reduce(f32::min)
    }

    /// The names of the playing sounds, oldest first.
    pub fn now_playing(&self) -> Vec<String> {
        self.playing_on_board()
            .map(|playing| self.display_name(&ClipId::new(&playing.path, playing.clip)))
            .collect()
    }

    /// The second line of a tile: the trimmed length once known, else the file format; then what
    /// triggers the sound, e.g. `0:01.20 · Note 36 · Ctrl+F13`.
    pub fn subtitle(&self, clip: &ClipId) -> String {
        let meta = self.sound_config(clip).unwrap_or_default();
        let length = match self.durations.get(&clip.path) {
            Some(&length) => format_duration(meta.trimmed_length(length)),
            None => (self.library.find(&clip.path))
                .map(|(_, sound)| sound.format())
                .unwrap_or_default(),
        };
        let midi = meta.midi.map(|binding| binding.control.to_string());
        let hotkey = meta.hotkey.map(|hotkey| hotkey.to_string());
        let parts: Vec<String> = [Some(length), midi, hotkey].into_iter().flatten().collect();
        parts.join(" · ")
    }

    /// The hotkey backend to use: the one chosen in the settings, else the session's default.
    pub fn hotkey_backend(&self) -> BackendKind {
        (self.config.settings.hotkey_backend).unwrap_or(self.default_hotkey_backend)
    }

    /// Selects a tab, keeping the selection in range after tabs were removed.
    pub fn select_tab(&mut self, tab: usize) {
        self.tab = tab.min(self.config.tabs.len().saturating_sub(1));
    }

    pub fn tab_index(&self, dir: &Path) -> Option<usize> {
        self.config.tabs.iter().position(|tab| tab.path == dir)
    }
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
    use crate::config::{ClipConfig, TabConfig};
    use crate::midi::{MidiBinding, MidiControl};

    struct Fixture {
        _dirs: [tempfile::TempDir; 2],
        state: AppState,
    }

    /// Two tabs: `Memes` (Airhorn, Bruh) and `SFX` (Applause, shown as "Clapping"; Horn).
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
                hotkey: "Ctrl+Shift+F13".parse().ok(),
                ..SoundConfig::default()
            },
        );
        let mut sfx_tab = TabConfig::for_folder(sfx.path().to_owned());
        sfx_tab.sounds.insert(
            "Applause.ogg".to_owned(),
            SoundConfig {
                name: Some("Clapping".to_owned()),
                ..SoundConfig::default()
            },
        );
        let config = Config {
            tabs: vec![memes_tab, sfx_tab],
            ..Config::default()
        };
        Fixture {
            state: AppState::new(config),
            _dirs: [memes, sfx],
        }
    }

    /// The file names of `clips`.
    fn names(clips: Vec<ClipId>) -> Vec<String> {
        clips.iter().map(|clip| file_stem(&clip.path)).collect()
    }

    fn shown(state: &AppState) -> Vec<String> {
        (state.visible_clips().iter())
            .map(|clip| state.display_name(clip))
            .collect()
    }

    /// Gives Airhorn a second clip, "Intro": its first 300 ms at 0.8 on note 36.
    fn add_intro(state: &mut AppState) -> ClipId {
        let airhorn = state.visible_clips()[0].clone();
        let file = state.sound_config_mut(&airhorn).expect("in a tab");
        file.clips.push(ClipConfig {
            id: 1,
            config: SoundConfig {
                name: Some("Intro".to_owned()),
                volume: 0.8,
                end_ms: Some(300),
                midi: Some(MidiBinding {
                    port: None,
                    channel: 1,
                    control: MidiControl::Note(36),
                }),
                ..SoundConfig::default()
            },
        });
        ClipId::new(airhorn.path, 1)
    }

    #[test]
    fn shows_the_selected_tab() {
        let mut f = fixture();
        assert_eq!(names(f.state.visible_clips()), ["Airhorn", "Bruh"]);
        f.state.select_tab(1);
        assert_eq!(names(f.state.visible_clips()), ["Applause", "Horn"]);
    }

    #[test]
    fn searching_shows_matches_from_all_tabs() {
        let mut f = fixture();
        f.state.search = "horn".to_owned();
        assert_eq!(names(f.state.visible_clips()), ["Airhorn", "Horn"]);
        f.state.search = "  ".to_owned();
        assert_eq!(names(f.state.visible_clips()), ["Airhorn", "Bruh"]);
    }

    #[test]
    fn renamed_sounds_show_and_are_found_by_both_names() {
        let mut f = fixture();
        f.state.select_tab(1);
        assert_eq!(shown(&f.state), ["Clapping", "Horn"]);
        f.state.search = "CLAP".to_owned();
        assert_eq!(names(f.state.visible_clips()), ["Applause"]);
        f.state.search = "applause".to_owned();
        assert_eq!(names(f.state.visible_clips()), ["Applause"]);
    }

    #[test]
    fn more_clips_follow_their_file_and_are_found_by_their_own_name() {
        let mut f = fixture();
        let intro = add_intro(&mut f.state);
        assert_eq!(shown(&f.state), ["Airhorn", "Intro", "Bruh"]);
        f.state.search = "INTRO".to_owned();
        assert_eq!(f.state.visible_clips(), std::slice::from_ref(&intro));
        f.state.search = "airhorn".to_owned();
        assert_eq!(f.state.visible_clips().len(), 2, "by the file name");

        f.state.search.clear();
        f.state.sound_config_mut(&intro).expect("exists").name = None;
        assert_eq!(shown(&f.state), ["Airhorn", "Airhorn (2)", "Bruh"]);
        assert_eq!(f.state.subtitle(&intro), "MP3 · Note 36");
    }

    #[test]
    fn every_clip_plays_its_own_trim_and_volume() {
        let mut f = fixture();
        let intro = add_intro(&mut f.state);
        assert_eq!(
            f.state.play_request(&intro),
            Some(PlayRequest {
                path: intro.path.clone(),
                clip: 1,
                trim: Some(Trim {
                    start_ms: 0,
                    end_ms: Some(300)
                }),
                volume: 0.8,
                destination: Destination::Everywhere,
            })
        );
        let airhorn = ClipId::file(&intro.path);
        assert_eq!(f.state.play_request(&airhorn).map(|r| r.volume), Some(0.5));
        let missing = ClipId::new(&intro.path, 2);
        assert_eq!(f.state.play_request(&missing), None);
        assert!(f.state.sound_config_mut(&missing).is_none());
    }

    #[test]
    fn progress_is_per_clip() {
        let mut f = fixture();
        let intro = add_intro(&mut f.state);
        let now = Instant::now();
        f.state.playing = vec![Playing {
            voice: 0,
            path: intro.path.clone(),
            clip: 1,
            started_at: now,
            duration: Duration::from_secs(1),
            destination: Destination::Everywhere,
        }];
        assert_eq!(f.state.progress(&intro, now), Some(0.0));
        assert_eq!(f.state.progress(&ClipId::file(&intro.path), now), None);
        assert_eq!(f.state.now_playing(), ["Intro"]);
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
        assert!(f.state.visible_clips().is_empty());
    }

    #[test]
    fn plays_use_the_volume_and_trim_from_the_config() {
        let f = fixture();
        let airhorn = f.state.visible_clips()[0].clone();
        let bruh = f.state.visible_clips()[1].clone();

        assert_eq!(
            f.state.play_request(&airhorn),
            Some(PlayRequest {
                path: airhorn.path,
                clip: 0,
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
                path: bruh.path,
                clip: 0,
                trim: None,
                volume: 1.0,
                destination: Destination::Everywhere,
            })
        );
        assert_eq!(f.state.play_request(&ClipId::file("/gone.wav")), None);
    }

    #[test]
    fn sound_metadata_is_created_on_first_change() {
        let mut f = fixture();
        let bruh = f.state.visible_clips()[1].clone();
        assert_eq!(f.state.sound_config(&bruh), Some(SoundConfig::default()));
        f.state.sound_config_mut(&bruh).expect("in a tab").volume = 0.3;
        assert_eq!(f.state.config.tabs[0].sounds["Bruh.wav"].volume, 0.3);
        assert!(
            f.state
                .sound_config_mut(&ClipId::file("/gone.wav"))
                .is_none()
        );
    }

    #[test]
    fn progress_and_names_of_playing_sounds() {
        let mut f = fixture();
        let airhorn = f.state.visible_clips()[0].clone();
        let now = Instant::now();
        let voice = |voice, started_at, destination| Playing {
            voice,
            path: airhorn.path.clone(),
            clip: 0,
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
        assert_eq!(f.state.progress(&ClipId::file("/other.wav"), now), None);
        assert_eq!(f.state.now_playing(), ["Airhorn", "Airhorn"]);
        assert_eq!(
            f.state.preview(&airhorn.path).map(|preview| preview.voice),
            Some(2)
        );
    }

    #[test]
    fn subtitles_show_the_length_once_known() {
        let mut f = fixture();
        let [airhorn, bruh] = [0, 1].map(|index| f.state.visible_clips()[index].clone());
        assert_eq!(f.state.subtitle(&airhorn), "MP3 · Ctrl+Shift+F13");
        for clip in [&airhorn, &bruh] {
            f.state
                .durations
                .insert(clip.path.clone(), Duration::from_millis(62_500));
        }
        assert_eq!(f.state.subtitle(&bruh), "1:02.50");
        assert_eq!(
            f.state.subtitle(&airhorn),
            "0:00.80 · Ctrl+Shift+F13",
            "trimmed to 0.1–0.9 s"
        );
    }

    #[test]
    fn the_settings_choose_the_hotkey_backend_or_the_default() {
        let mut f = fixture();
        f.state.default_hotkey_backend = BackendKind::Portal;
        assert_eq!(f.state.hotkey_backend(), BackendKind::Portal);
        f.state.config.settings.hotkey_backend = Some(BackendKind::Off);
        assert_eq!(f.state.hotkey_backend(), BackendKind::Off);
    }

    #[test]
    fn subtitles_show_the_midi_binding() {
        let mut f = fixture();
        let bruh = f.state.visible_clips()[1].clone();
        f.state.sound_config_mut(&bruh).expect("in a tab").midi = Some(MidiBinding {
            port: None,
            channel: 1,
            control: MidiControl::ControlChange(20),
        });
        assert_eq!(f.state.subtitle(&bruh), "WAV · CC 20");
    }
}
