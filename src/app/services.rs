//! The outside world as the app sees it: small traits so GUI tests can run with fakes instead of
//! PipeWire, the desktop portal and global hotkeys.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use crate::audio::{AudioEngine, EngineEvent, PlayHandle, PlayRequest, Playing, Retrigger};
use crate::hotkeys::{self, BackendKind, EventSink, HotkeyBackend};

/// What the UI needs from [`AudioEngine`]. Every call returns right away.
pub trait AudioBackend: Send {
    fn play(&self, request: PlayRequest);
    fn stop_all(&self);
    fn stop_preview(&self);
    fn load_waveform(&self, path: &Path);
    /// Keeps the sounds at `paths` decoded in memory (see [`AudioEngine::preload`]).
    fn preload(&self, paths: Vec<PathBuf>);
    fn set_call_volume(&self, volume: f32);
    fn set_local_volume(&self, volume: f32);
    fn set_local_enabled(&self, enabled: bool);
    fn set_retrigger_policy(&self, retrigger: Retrigger);
    fn invalidate(&self, path: &Path);
    fn link_to(&self, node_id: u32);
    fn use_virtual_mic(&self);
    fn unlink(&self);
    /// The events since the last call.
    fn poll_events(&self) -> Vec<EngineEvent>;
    fn playing(&self) -> Vec<Playing>;
    /// Plays and stops sounds from the thread that handles hotkeys and MIDI.
    fn player(&self) -> Box<dyn Player>;
}

/// What hotkeys and MIDI triggers need from the audio engine, from their own thread.
pub trait Player: Send {
    fn play(&self, request: PlayRequest);
    fn stop_all(&self);
}

impl Player for PlayHandle {
    fn play(&self, request: PlayRequest) {
        PlayHandle::play(self, request);
    }

    fn stop_all(&self) {
        PlayHandle::stop_all(self);
    }
}

impl AudioBackend for AudioEngine {
    fn play(&self, request: PlayRequest) {
        AudioEngine::play(self, request);
    }

    fn stop_all(&self) {
        AudioEngine::stop_all(self);
    }

    fn stop_preview(&self) {
        AudioEngine::stop_preview(self);
    }

    fn load_waveform(&self, path: &Path) {
        AudioEngine::load_waveform(self, path);
    }

    fn preload(&self, paths: Vec<PathBuf>) {
        AudioEngine::preload(self, paths);
    }

    fn set_call_volume(&self, volume: f32) {
        AudioEngine::set_call_volume(self, volume);
    }

    fn set_local_volume(&self, volume: f32) {
        AudioEngine::set_local_volume(self, volume);
    }

    fn set_local_enabled(&self, enabled: bool) {
        AudioEngine::set_local_enabled(self, enabled);
    }

    fn set_retrigger_policy(&self, retrigger: Retrigger) {
        AudioEngine::set_retrigger_policy(self, retrigger);
    }

    fn invalidate(&self, path: &Path) {
        AudioEngine::invalidate(self, path);
    }

    fn link_to(&self, node_id: u32) {
        AudioEngine::link_to(self, node_id);
    }

    fn use_virtual_mic(&self) {
        AudioEngine::use_virtual_mic(self);
    }

    fn unlink(&self) {
        AudioEngine::unlink(self);
    }

    fn poll_events(&self) -> Vec<EngineEvent> {
        self.events().try_iter().collect()
    }

    fn playing(&self) -> Vec<Playing> {
        AudioEngine::playing(self)
    }

    fn player(&self) -> Box<dyn Player> {
        Box::new(self.play_handle())
    }
}

/// Asks the user for a folder.
pub trait FolderPicker: Send {
    /// Opens the dialog without blocking; the chosen folder, if any, is sent to `picked`.
    fn pick_folder(&self, picked: mpsc::Sender<PathBuf>);
}

/// The desktop's folder chooser through the XDG desktop portal (`rfd`).
#[derive(Debug, Default)]
pub struct PortalFolderPicker;

impl FolderPicker for PortalFolderPicker {
    fn pick_folder(&self, picked: mpsc::Sender<PathBuf>) {
        // The portal call blocks until the dialog closes, so it runs on its own thread.
        let spawned = thread::Builder::new()
            .name("folder-picker".to_owned())
            .spawn(move || {
                let folder = rfd::FileDialog::new()
                    .set_title("Add folder as tab")
                    .pick_folder();
                if let Some(folder) = folder {
                    // Fails only if the app is gone.
                    let _ = picked.send(folder);
                }
            });
        if let Err(error) = spawned {
            tracing::error!("could not open the folder picker: {error}");
        }
    }
}

/// Starts hotkey backends; switching the backend in the settings starts a new one.
pub trait HotkeyBackends: Send {
    /// The backend of `kind` (`None` for [`BackendKind::Off`]), sending its events to `events`.
    fn start(
        &self,
        kind: BackendKind,
        events: EventSink,
    ) -> Result<Option<Box<dyn HotkeyBackend>>, String>;
}

/// The real backends: the XDG portal and X11.
#[derive(Debug, Default)]
pub struct SystemHotkeys;

impl HotkeyBackends for SystemHotkeys {
    fn start(
        &self,
        kind: BackendKind,
        events: EventSink,
    ) -> Result<Option<Box<dyn HotkeyBackend>>, String> {
        hotkeys::start(kind, events)
    }
}
