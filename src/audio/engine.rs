//! [`AudioEngine`]: what the app uses to play sounds. It runs a control thread that owns the sound
//! cache and talks to the mixers of both output streams (see [`super::controller`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::AudioError;
use super::controller::{Controller, EngineEvent, Message, PlayRequest, Playing};
use super::decode::decode_file;
use super::loader::SoundCache;
use super::mixer::{Mixer, Retrigger};
use super::service::{PipeWireService, Renders};

/// How often the control thread collects finished voices while sounds play.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Plays sounds into the call (`soundboars-inject`, linked into an app or the virtual microphone)
/// and, if enabled, on the default sink
/// (`soundboars-local`). Every call returns right away; results arrive as [`EngineEvent`]s.
/// Dropping the engine stops playback, unlinks and disconnects from PipeWire.
pub struct AudioEngine {
    messages: mpsc::Sender<Message>,
    events: mpsc::Receiver<EngineEvent>,
    playing: Arc<Mutex<Vec<Playing>>>,
    control: Option<JoinHandle<()>>,
    // Dropped after the control thread has stopped (see `Drop`).
    pipewire: PipeWireService,
}

impl AudioEngine {
    /// Connects to PipeWire and starts both output streams. Call and local volume start at `1.0`,
    /// local playback is enabled and sounds overlap ([`Retrigger::Overlap`]).
    pub fn start() -> Result<Self, AudioError> {
        let (messages, message_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let (mut inject, inject_handle) = Mixer::new(1.0);
        let (mut local, local_handle) = Mixer::new(1.0);
        let renders = Renders {
            inject: Box::new(move |out: &mut [f32]| inject.render(out)),
            local: Box::new(move |out: &mut [f32]| local.render(out)),
        };
        let pipewire = PipeWireService::start(renders, forward(&messages, Message::PipeWire))?;
        let cache = SoundCache::new(decode_file, forward(&messages, Message::Decoded))?;
        let playing = Arc::default();
        let controller = Controller::new(
            cache,
            inject_handle,
            local_handle,
            event_tx,
            Arc::clone(&playing),
        );
        let control = thread::Builder::new()
            .name("audio-control".to_owned())
            .spawn(move || run(controller, &message_rx))?;
        Ok(Self {
            messages,
            events,
            playing,
            control: Some(control),
            pipewire,
        })
    }

    /// Plays a sound on the streams of its [`PlayRequest::destination`]. A sound that isn't
    /// decoded yet is decoded in the background first; [`EngineEvent::DecodeFailed`] reports if
    /// that fails.
    pub fn play(&self, request: PlayRequest) {
        self.send(Message::Play(request));
    }

    /// Stops every voice of every clip of the sound file at `path`, and plays of it still waiting
    /// for decoding.
    pub fn stop(&self, path: &Path) {
        self.send(Message::Stop(path.to_owned()));
    }

    pub fn stop_all(&self) {
        self.send(Message::StopAll);
    }

    /// Stops the playing [`Destination::Preview`](super::Destination::Preview), if any.
    pub fn stop_preview(&self) {
        self.send(Message::StopPreview);
    }

    /// Computes the waveform of the sound at `path` in the background (decoding it first if
    /// needed); it arrives as [`EngineEvent::Waveform`].
    pub fn load_waveform(&self, path: &Path) {
        self.send(Message::LoadWaveform(path.to_owned()));
    }

    /// Decodes the sounds at `paths` in the background, so they play right away; decoded sounds
    /// not among them are forgotten. Sounds that are played meanwhile are decoded first.
    pub fn preload(&self, paths: Vec<PathBuf>) {
        self.send(Message::Preload(paths));
    }

    /// Volume of what the call receives, `0.0..=1.0`.
    pub fn set_call_volume(&self, volume: f32) {
        self.send(Message::SetCallVolume(volume));
    }

    /// Volume of local playback, `0.0..=1.0`.
    pub fn set_local_volume(&self, volume: f32) {
        self.send(Message::SetLocalVolume(volume));
    }

    /// Whether sounds also play locally. Disabling it stops local playback right away.
    pub fn set_local_enabled(&self, enabled: bool) {
        self.send(Message::SetLocalEnabled(enabled));
    }

    /// What playing a sound that is already playing does; applies to later plays.
    pub fn set_retrigger_policy(&self, retrigger: Retrigger) {
        self.send(Message::SetRetrigger(retrigger));
    }

    /// Forgets the decoded sound at `path`, e.g. because the file changed; the next play decodes
    /// it again.
    pub fn invalidate(&self, path: &Path) {
        self.send(Message::Invalidate(path.to_owned()));
    }

    /// Links the inject stream into the recording stream `node_id` (see
    /// [`EngineEvent::RecordingApps`]), replacing any previous target.
    pub fn link_to(&self, node_id: u32) {
        self.pipewire.link_to(node_id);
        self.send(Message::SetTarget(Some(node_id)));
    }

    /// Sends sounds and the user's microphone through the virtual microphone "SoundBoars
    /// Microphone" (see [`EngineEvent::Microphone`]), replacing any previous target.
    pub fn use_virtual_mic(&self) {
        self.pipewire.use_virtual_mic();
        // Consumers of the virtual microphone downmix by averaging, so no compensation is needed.
        self.send(Message::SetTarget(None));
    }

    pub fn unlink(&self) {
        self.pipewire.unlink();
        self.send(Message::SetTarget(None));
    }

    /// A handle that plays and stops sounds from any thread, e.g. the one handling hotkeys and
    /// MIDI. It does nothing once the engine is dropped.
    pub fn play_handle(&self) -> PlayHandle {
        PlayHandle {
            messages: self.messages.clone(),
        }
    }

    pub fn events(&self) -> &mpsc::Receiver<EngineEvent> {
        &self.events
    }

    /// The sounds playing right now, oldest first.
    pub fn playing(&self) -> Vec<Playing> {
        self.playing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn send(&self, message: Message) {
        // Fails only if the control thread is gone, and then nothing can be played anymore.
        let _ = self.messages.send(message);
    }
}

/// Plays and stops sounds like [`AudioEngine::play`] and [`AudioEngine::stop_all`], from any
/// thread (see [`AudioEngine::play_handle`]).
#[derive(Clone)]
pub struct PlayHandle {
    messages: mpsc::Sender<Message>,
}

impl PlayHandle {
    pub fn play(&self, request: PlayRequest) {
        // Fails only if the engine is gone, and then nothing can be played anymore.
        let _ = self.messages.send(Message::Play(request));
    }

    pub fn stop_all(&self) {
        let _ = self.messages.send(Message::StopAll);
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.send(Message::Quit);
        if let Some(control) = self.control.take() {
            let _ = control.join();
        }
    }
}

/// A callback that wraps what it's given into a [`Message`] for the control thread.
fn forward<T: 'static>(
    messages: &mpsc::Sender<Message>,
    wrap: fn(T) -> Message,
) -> impl Fn(T) + Send + 'static {
    let messages = messages.clone();
    // Fails only if the control thread is gone, and then nobody is interested anymore.
    move |value| {
        let _ = messages.send(wrap(value));
    }
}

/// The control thread: handles messages, and while sounds play, polls the mixers for finished
/// voices (the real-time threads can't wake it up).
fn run(mut controller: Controller, messages: &mpsc::Receiver<Message>) {
    loop {
        let message = if controller.has_voices() {
            match messages.recv_timeout(POLL_INTERVAL) {
                Ok(message) => Some(message),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match messages.recv() {
                Ok(message) => Some(message),
                Err(mpsc::RecvError) => return,
            }
        };
        match message {
            Some(Message::Quit) => return,
            Some(message) => controller.handle(message),
            None => {}
        }
        controller.poll_mixers();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_can_move_to_another_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<AudioEngine>();
    }

    #[test]
    fn the_play_handle_can_be_shared_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PlayHandle>();
    }
}
