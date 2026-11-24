//! The outside world as the app sees it: small traits so GUI tests can run with fakes instead of
//! PipeWire and the desktop portal.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use crate::audio::{AudioEngine, EngineEvent, PlayRequest, Playing, Retrigger};

/// What the UI needs from [`AudioEngine`]. Every call returns right away.
pub trait AudioBackend {
    fn play(&self, request: PlayRequest);
    fn stop_all(&self);
    fn set_call_volume(&self, volume: f32);
    fn set_local_volume(&self, volume: f32);
    fn set_local_enabled(&self, enabled: bool);
    fn set_retrigger_policy(&self, retrigger: Retrigger);
    fn invalidate(&self, path: &Path);
    fn link_to(&self, node_id: u32);
    fn unlink(&self);
    /// The events since the last call.
    fn poll_events(&self) -> Vec<EngineEvent>;
    fn playing(&self) -> Vec<Playing>;
}

impl AudioBackend for AudioEngine {
    fn play(&self, request: PlayRequest) {
        AudioEngine::play(self, request);
    }

    fn stop_all(&self) {
        AudioEngine::stop_all(self);
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

    fn unlink(&self) {
        AudioEngine::unlink(self);
    }

    fn poll_events(&self) -> Vec<EngineEvent> {
        self.events().try_iter().collect()
    }

    fn playing(&self) -> Vec<Playing> {
        AudioEngine::playing(self)
    }
}

/// Asks the user for a folder.
pub trait FolderPicker {
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
