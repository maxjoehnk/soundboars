//! The "Edit sound" window's state and logic: per-sound volume, the non-destructive trim and the
//! global hotkey, edited on a copy that [`SoundEditor::apply_to`] writes into the sound's [`SoundConfig`] on save.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::audio::{Destination, PlayRequest, Playing, Trim, Waveform};
use crate::config::SoundConfig;
use crate::hotkeys::Hotkey;

/// The shortest trimmed sound the handles allow.
pub const MIN_LENGTH_MS: u64 = 50;

/// What the user did in the editor (see [`super::Action::Editor`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditorAction {
    Volume(f32),
    Start(u64),
    End(u64),
    /// The pointer went down on the waveform at this time: grab the nearer handle and move it there.
    Grab(u64),
    /// Move the grabbed handle here.
    Drag(u64),
    Release,
    /// Play the whole sound again.
    Reset,
    Preview,
    StopPreview,
    Save,
    /// Cancel: close without saving.
    Close,
    /// Capture the next key combination pressed in the editor as the hotkey.
    RecordHotkey,
    CancelRecording,
    /// The recorded hotkey; also gives the sound a global shortcut.
    Hotkey(Hotkey),
    /// Removes the hotkey and the global shortcut.
    ClearHotkey,
    /// Gives the sound a global shortcut without a hotkey (for desktops that bind keys
    /// themselves, like Hyprland). The app assigns its id.
    AddShortcut,
}

/// A trim handle on the waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Start,
    End,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WaveformState {
    Loading,
    Ready(Waveform),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SoundEditor {
    pub path: PathBuf,
    /// The file name, shown read-only.
    pub name: String,
    pub volume: f32,
    pub start_ms: u64,
    /// `None` plays to the end. Once the waveform (and so the length) is known, this is always set.
    pub end_ms: Option<u64>,
    pub waveform: WaveformState,
    /// The handle being dragged on the waveform.
    pub grabbed: Option<Handle>,
    /// Where the last preview started, for the playhead.
    pub preview_start_ms: u64,
    pub hotkey: Option<Hotkey>,
    /// Set while the sound has a global shortcut (see [`SoundConfig::shortcut_id`]).
    pub shortcut_id: Option<String>,
    /// The next key combination pressed becomes the hotkey.
    pub recording: bool,
}

impl SoundEditor {
    /// Edits the sound at `path`, starting from its saved metadata. The waveform is loading.
    pub fn open(path: &Path, saved: &SoundConfig) -> Self {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        Self {
            path: path.to_owned(),
            name,
            volume: saved.volume,
            start_ms: saved.start_ms,
            end_ms: saved.end_ms,
            waveform: WaveformState::Loading,
            grabbed: None,
            preview_start_ms: 0,
            hotkey: saved.hotkey,
            shortcut_id: saved.shortcut_id.clone(),
            recording: false,
        }
    }

    /// Takes in the waveform (or why there is none) and fits the trim into the sound's length.
    pub fn waveform_loaded(&mut self, waveform: Result<Waveform, String>) {
        self.waveform = match waveform {
            Ok(waveform) => WaveformState::Ready(waveform),
            Err(error) => WaveformState::Failed(error),
        };
        if let Some(duration) = self.duration_ms() {
            let end = self.end_ms.unwrap_or(duration).min(duration);
            self.end_ms = Some(end);
            self.set_start(self.start_ms);
            self.set_end(end);
        }
    }

    /// The length of the whole sound, once the waveform is loaded.
    pub fn duration_ms(&self) -> Option<u64> {
        match &self.waveform {
            WaveformState::Ready(waveform) => Some(duration_ms(waveform.duration)),
            _ => None,
        }
    }

    /// The length of the trimmed sound, once the waveform is loaded.
    pub fn trimmed_length(&self) -> Option<Duration> {
        let end = self.end_ms.or(self.duration_ms())?;
        Some(Duration::from_millis(end.saturating_sub(self.start_ms)))
    }

    /// Changes the editor's copy; actions that need services or the config
    /// ([`EditorAction::Preview`], [`EditorAction::StopPreview`], [`EditorAction::Save`],
    /// [`EditorAction::Close`], assigning the id for [`EditorAction::AddShortcut`]) are the app's
    /// business and ignored here.
    pub fn apply(&mut self, action: EditorAction) {
        match action {
            EditorAction::Volume(volume) => self.volume = volume.max(0.0),
            EditorAction::Start(ms) => self.set_start(ms),
            EditorAction::End(ms) => self.set_end(ms),
            EditorAction::Grab(ms) => {
                let handle = nearest_handle(ms, self.start_ms, self.end());
                self.grabbed = Some(handle);
                self.move_handle(handle, ms);
            }
            EditorAction::Drag(ms) => {
                if let Some(handle) = self.grabbed {
                    self.move_handle(handle, ms);
                }
            }
            EditorAction::Release => self.grabbed = None,
            EditorAction::Reset => {
                self.start_ms = 0;
                self.end_ms = self.duration_ms();
            }
            EditorAction::RecordHotkey => self.recording = true,
            EditorAction::CancelRecording => self.recording = false,
            EditorAction::Hotkey(hotkey) => {
                self.hotkey = Some(hotkey);
                self.recording = false;
            }
            EditorAction::ClearHotkey => {
                self.hotkey = None;
                self.shortcut_id = None;
                self.recording = false;
            }
            EditorAction::Preview
            | EditorAction::StopPreview
            | EditorAction::Save
            | EditorAction::Close
            | EditorAction::AddShortcut => {}
        }
    }

    /// Plays the edited trim and volume locally only; remembers where it starts.
    pub fn preview_request(&mut self) -> PlayRequest {
        self.preview_start_ms = self.start_ms;
        PlayRequest {
            path: self.path.clone(),
            trim: self.trim(),
            volume: self.volume,
            destination: Destination::Preview,
        }
    }

    /// Where the playing `preview` is at `now`, in milliseconds from the start of the sound.
    pub fn playhead_ms(&self, preview: &Playing, now: Instant) -> u64 {
        let elapsed = now.saturating_duration_since(preview.started_at);
        self.preview_start_ms + duration_ms(elapsed.min(preview.duration))
    }

    /// Writes the volume, trim and hotkey into `config`: a trim to the end of the sound is stored
    /// as no end, so the whole sound is played without any trim at all.
    pub fn apply_to(&self, config: &mut SoundConfig) {
        let trim = self.trim().unwrap_or_default();
        config.volume = self.volume;
        config.start_ms = trim.start_ms;
        config.end_ms = trim.end_ms;
        config.hotkey = self.hotkey;
        config.shortcut_id = self.shortcut_id.clone();
    }

    /// The edited trim, `None` for the whole sound.
    fn trim(&self) -> Option<Trim> {
        let end_ms = self.end_ms.filter(|&end| Some(end) != self.duration_ms());
        (self.start_ms > 0 || end_ms.is_some()).then_some(Trim {
            start_ms: self.start_ms,
            end_ms,
        })
    }

    /// The end, or `u64::MAX` while it's unknown.
    fn end(&self) -> u64 {
        self.end_ms.unwrap_or(u64::MAX)
    }

    fn move_handle(&mut self, handle: Handle, ms: u64) {
        match handle {
            Handle::Start => self.set_start(ms),
            Handle::End => self.set_end(ms),
        }
    }

    /// Moves the start, at most to [`MIN_LENGTH_MS`] before the end.
    fn set_start(&mut self, ms: u64) {
        self.start_ms = ms.min(self.end().saturating_sub(MIN_LENGTH_MS));
    }

    /// Moves the end, at least [`MIN_LENGTH_MS`] after the start, at most to the end of the sound.
    fn set_end(&mut self, ms: u64) {
        let Some(duration) = self.duration_ms() else {
            return;
        };
        let earliest = (self.start_ms + MIN_LENGTH_MS).min(duration);
        self.end_ms = Some(ms.clamp(earliest, duration));
    }
}

/// The handle closer to `ms`. On a tie the end, unless both handles are stacked right of `ms`:
/// then only the start can move there.
pub fn nearest_handle(ms: u64, start_ms: u64, end_ms: u64) -> Handle {
    if ms.abs_diff(start_ms) < ms.abs_diff(end_ms) || (ms < start_ms && start_ms == end_ms) {
        Handle::Start
    } else {
        Handle::End
    }
}

/// The time at horizontal position `x` of a waveform drawn from `left` to `left + width` for a
/// sound of `duration_ms`; clamped to the sound.
pub fn ms_at(x: f32, left: f32, width: f32, duration_ms: u64) -> u64 {
    if width <= 0.0 {
        return 0;
    }
    let fraction = ((x - left) / width).clamp(0.0, 1.0);
    (f64::from(fraction) * duration_ms as f64).round() as u64
}

/// Where time `ms` is on a waveform drawn from `left` to `left + width` (see [`ms_at`]).
pub fn x_of(ms: u64, left: f32, width: f32, duration_ms: u64) -> f32 {
    if duration_ms == 0 {
        return left;
    }
    left + width * (ms.min(duration_ms) as f64 / duration_ms as f64) as f32
}

pub fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOUND_MS: u64 = 2_000;

    fn waveform() -> Waveform {
        Waveform {
            peaks: Vec::new(),
            duration: Duration::from_millis(SOUND_MS),
        }
    }

    fn editor(start_ms: u64, end_ms: Option<u64>) -> SoundEditor {
        let saved = SoundConfig {
            volume: 0.5,
            start_ms,
            end_ms,
            ..SoundConfig::default()
        };
        let mut editor = SoundEditor::open(Path::new("/sounds/Air horn.mp3"), &saved);
        editor.waveform_loaded(Ok(waveform()));
        editor
    }

    fn trim(editor: &SoundEditor) -> (u64, Option<u64>) {
        (editor.start_ms, editor.end_ms)
    }

    fn saved(editor: &SoundEditor) -> SoundConfig {
        let mut config = SoundConfig::default();
        editor.apply_to(&mut config);
        config
    }

    #[test]
    fn opens_with_the_saved_values_and_the_file_name() {
        let saved = SoundConfig {
            volume: 0.5,
            start_ms: 100,
            end_ms: None,
            ..SoundConfig::default()
        };
        let editor = SoundEditor::open(Path::new("/sounds/Air horn.mp3"), &saved);
        assert_eq!(editor.name, "Air horn.mp3");
        assert_eq!(editor.volume, 0.5);
        assert_eq!(trim(&editor), (100, None));
        assert_eq!(editor.waveform, WaveformState::Loading);
        assert_eq!(editor.duration_ms(), None);
    }

    #[test]
    fn the_end_is_known_once_the_waveform_is() {
        assert_eq!(trim(&editor(100, None)), (100, Some(SOUND_MS)));
        assert_eq!(trim(&editor(100, Some(900))), (100, Some(900)));
        assert_eq!(
            editor(100, Some(900)).trimmed_length(),
            Some(Duration::from_millis(800))
        );
    }

    #[test]
    fn a_saved_trim_beyond_the_sound_is_clamped() {
        assert_eq!(trim(&editor(5_000, Some(9_000))), (1_950, Some(SOUND_MS)));
    }

    #[test]
    fn a_failed_waveform_keeps_the_trim() {
        let mut editor = SoundEditor::open(Path::new("a.wav"), &SoundConfig::default());
        editor.waveform_loaded(Err("broken".to_owned()));
        assert_eq!(editor.waveform, WaveformState::Failed("broken".to_owned()));
        assert_eq!(trim(&editor), (0, None));
        assert_eq!(saved(&editor), SoundConfig::default());
    }

    #[test]
    fn handles_keep_a_minimum_length_and_stay_in_the_sound() {
        let mut editor = editor(0, None);
        editor.apply(EditorAction::End(500));
        editor.apply(EditorAction::Start(490));
        assert_eq!(trim(&editor), (450, Some(500)));
        editor.apply(EditorAction::End(10));
        assert_eq!(trim(&editor), (450, Some(500)));
        editor.apply(EditorAction::End(99_999));
        assert_eq!(trim(&editor), (450, Some(SOUND_MS)));
    }

    #[test]
    fn grabbing_moves_the_nearer_handle_until_released() {
        let mut editor = editor(400, Some(1_600));
        editor.apply(EditorAction::Grab(700));
        assert_eq!(editor.grabbed, Some(Handle::Start));
        assert_eq!(trim(&editor), (700, Some(1_600)));

        // Still the start, though the pointer is now nearer the end; it stops before the end.
        editor.apply(EditorAction::Drag(1_590));
        assert_eq!(trim(&editor), (1_550, Some(1_600)));

        editor.apply(EditorAction::Release);
        editor.apply(EditorAction::Drag(0));
        assert_eq!(trim(&editor), (1_550, Some(1_600)));
        editor.apply(EditorAction::Grab(1_900));
        assert_eq!(editor.grabbed, Some(Handle::End));
        assert_eq!(trim(&editor), (1_550, Some(1_900)));
    }

    #[test]
    fn nearest_handle_on_ties_and_stacked_handles() {
        assert_eq!(nearest_handle(100, 0, 1_000), Handle::Start);
        assert_eq!(nearest_handle(900, 0, 1_000), Handle::End);
        assert_eq!(nearest_handle(500, 0, 1_000), Handle::End);
        // Both handles at the very end: only the start can move left.
        assert_eq!(nearest_handle(10, 2_000, 2_000), Handle::Start);
        assert_eq!(nearest_handle(0, 0, 0), Handle::End);
    }

    #[test]
    fn reset_selects_the_whole_sound() {
        let mut editor = editor(100, Some(900));
        editor.apply(EditorAction::Reset);
        assert_eq!(trim(&editor), (0, Some(SOUND_MS)));
        assert_eq!(saved(&editor).trim(), None);
    }

    #[test]
    fn saving_stores_no_end_for_the_end_of_the_sound() {
        let mut editor = editor(0, None);
        editor.apply(EditorAction::Volume(0.8));
        assert_eq!(
            saved(&editor),
            SoundConfig {
                volume: 0.8,
                start_ms: 0,
                end_ms: None,
                ..SoundConfig::default()
            }
        );
        editor.apply(EditorAction::Start(300));
        assert_eq!(
            (saved(&editor).start_ms, saved(&editor).end_ms),
            (300, None)
        );
        editor.apply(EditorAction::End(1_200));
        assert_eq!(
            (saved(&editor).start_ms, saved(&editor).end_ms),
            (300, Some(1_200))
        );
    }

    #[test]
    fn previews_play_the_edited_trim_locally() {
        let mut editor = editor(300, Some(1_200));
        editor.apply(EditorAction::Volume(0.25));
        let request = editor.preview_request();
        assert_eq!(request.destination, Destination::Preview);
        assert_eq!(request.volume, 0.25);
        assert_eq!(
            request.trim,
            Some(Trim {
                start_ms: 300,
                end_ms: Some(1_200)
            })
        );
        assert_eq!(editor.preview_start_ms, 300);
        editor.apply(EditorAction::Reset);
        assert_eq!(editor.preview_request().trim, None);
    }

    #[test]
    fn the_playhead_moves_from_where_the_preview_started() {
        let mut editor = editor(300, Some(1_200));
        editor.preview_request();
        editor.apply(EditorAction::Start(500));
        let started_at = Instant::now();
        let preview = Playing {
            voice: 1,
            path: editor.path.clone(),
            started_at,
            duration: Duration::from_millis(900),
            destination: Destination::Preview,
        };
        assert_eq!(editor.playhead_ms(&preview, started_at), 300);
        let later = started_at + Duration::from_millis(250);
        assert_eq!(editor.playhead_ms(&preview, later), 550);
        let much_later = started_at + Duration::from_secs(5);
        assert_eq!(editor.playhead_ms(&preview, much_later), 1_200);
    }

    #[test]
    fn pixels_and_times_convert_both_ways() {
        assert_eq!(ms_at(10.0, 10.0, 200.0, SOUND_MS), 0);
        assert_eq!(ms_at(60.0, 10.0, 200.0, SOUND_MS), 500);
        assert_eq!(ms_at(210.0, 10.0, 200.0, SOUND_MS), SOUND_MS);
        assert_eq!(ms_at(-50.0, 10.0, 200.0, SOUND_MS), 0, "clamped left");
        assert_eq!(
            ms_at(500.0, 10.0, 200.0, SOUND_MS),
            SOUND_MS,
            "clamped right"
        );
        assert_eq!(ms_at(50.0, 10.0, 0.0, SOUND_MS), 0, "no width");
        assert_eq!(x_of(500, 10.0, 200.0, SOUND_MS), 60.0);
        assert_eq!(x_of(99_999, 10.0, 200.0, SOUND_MS), 210.0);
        assert_eq!(x_of(100, 10.0, 200.0, 0), 10.0);
    }

    #[test]
    fn hotkeys_are_recorded_cleared_and_saved() {
        let ctrl_f13: Hotkey = "Ctrl+F13".parse().expect("valid hotkey");
        let saved = SoundConfig {
            hotkey: Some(ctrl_f13),
            shortcut_id: Some("play-air-horn".to_owned()),
            ..SoundConfig::default()
        };
        let mut editor = SoundEditor::open(Path::new("/sounds/Air horn.mp3"), &saved);
        assert_eq!(editor.hotkey, Some(ctrl_f13));
        assert_eq!(editor.shortcut_id.as_deref(), Some("play-air-horn"));

        editor.apply(EditorAction::RecordHotkey);
        assert!(editor.recording);
        editor.apply(EditorAction::CancelRecording);
        assert!(!editor.recording);
        assert_eq!(editor.hotkey, Some(ctrl_f13));

        editor.apply(EditorAction::ClearHotkey);
        assert_eq!((editor.hotkey, editor.shortcut_id.as_deref()), (None, None));
        assert_eq!(saved_hotkey(&editor), (None, None));

        let alt_1: Hotkey = "Alt+1".parse().expect("valid hotkey");
        editor.apply(EditorAction::RecordHotkey);
        editor.apply(EditorAction::Hotkey(alt_1));
        assert!(!editor.recording);
        editor.shortcut_id = Some("play-air-horn".to_owned());
        assert_eq!(
            saved_hotkey(&editor),
            (Some(alt_1), Some("play-air-horn".to_owned()))
        );
    }

    fn saved_hotkey(editor: &SoundEditor) -> (Option<Hotkey>, Option<String>) {
        let saved = saved(editor);
        (saved.hotkey, saved.shortcut_id)
    }
}
