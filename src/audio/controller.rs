//! The control side of [`super::AudioEngine`]: turns play/stop/volume requests into mixer commands
//! for both output streams, waits for decodes, and keeps track of what's playing. Runs on the
//! engine's control thread; never on a real-time thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::decode::{DecodeError, Samples};
use super::format::{CHANNELS, SAMPLE_RATE};
use super::graph::RecordingApp;
use super::link_plan::sums_channels;
use super::loader::{Decoded, SoundCache};
use super::mixer::{Command, MixerHandle, Play, Retrigger, SoundId, VoiceId};
use super::service::Event;
use super::waveform::Waveform;

/// The sound id of previews: unlike file paths, all previews count as one sound.
const PREVIEW_SOUND: SoundId = SoundId::MAX;

/// Plays a sound file.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayRequest {
    pub path: PathBuf,
    /// Only play this part of the file.
    pub trim: Option<Trim>,
    /// The sound's own volume; `1.0` plays it as it is.
    pub volume: f32,
    pub destination: Destination,
}

/// Where a sound plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Destination {
    /// Into the call (the linked app or the virtual microphone) and, while local playback is
    /// enabled, locally.
    #[default]
    Everywhere,
    /// Only locally (even while local playback is disabled), never into the call: the trim
    /// editor's preview. A preview replaces the one playing; stop it with
    /// [`Message::StopPreview`].
    Preview,
}

/// A part of a sound file, in milliseconds from its start. Out-of-range values are clamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Trim {
    pub start_ms: u64,
    /// `None` plays to the end.
    pub end_ms: Option<u64>,
}

/// A sound that is playing, for showing it (and its progress) in the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct Playing {
    pub voice: VoiceId,
    pub path: PathBuf,
    pub started_at: Instant,
    /// The length of the (trimmed) sound.
    pub duration: Duration,
    pub destination: Destination,
}

impl Playing {
    /// How far playback is at `now`, from `0.0` to `1.0`. Approximate: it's based on when the
    /// sound was sent to the mixers, not on the samples played.
    pub fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(self.started_at);
        (elapsed.as_secs_f32() / self.duration.as_secs_f32()).min(1.0)
    }
}

#[derive(Debug)]
pub enum EngineEvent {
    /// The apps that currently record audio; sent whenever the list changes.
    RecordingApps(Vec<RecordingApp>),
    /// The description of the microphone the virtual microphone mixes in, `None` if there is
    /// none; sent whenever it changes.
    Microphone(Option<String>),
    /// Something went wrong in PipeWire or the engine.
    Error(String),
    /// A sound started playing.
    Started(Playing),
    /// A voice ended: it played out, was stopped, or was rejected by retriggering
    /// ([`Retrigger::Stop`]) or because too many sounds play at once.
    Finished { voice: VoiceId, path: PathBuf },
    /// A sound could not be decoded, so it wasn't played.
    DecodeFailed { path: PathBuf, error: DecodeError },
    /// The waveform asked for with [`Message::LoadWaveform`], or why the sound couldn't be decoded.
    Waveform {
        path: PathBuf,
        waveform: Result<Waveform, String>,
    },
}

/// Everything the control thread reacts to.
pub enum Message {
    Play(PlayRequest),
    Stop(PathBuf),
    StopAll,
    StopPreview,
    LoadWaveform(PathBuf),
    Preload(Vec<PathBuf>),
    SetCallVolume(f32),
    SetLocalVolume(f32),
    SetLocalEnabled(bool),
    SetRetrigger(Retrigger),
    Invalidate(PathBuf),
    SetTarget(Option<u32>),
    PipeWire(Event),
    Decoded(Decoded),
    Quit,
}

/// A voice that at least one mixer still plays.
struct ActiveVoice {
    playing: Playing,
    /// Mixers that haven't reported the voice finished yet.
    mixers: u8,
}

pub struct Controller {
    cache: SoundCache,
    inject: MixerHandle,
    local: MixerHandle,
    events: mpsc::Sender<EngineEvent>,
    playing: Arc<Mutex<Vec<Playing>>>,
    call_volume: f32,
    local_volume: f32,
    local_enabled: bool,
    retrigger: Retrigger,
    target: Option<u32>,
    apps: Vec<RecordingApp>,
    sound_ids: HashMap<PathBuf, SoundId>,
    /// Plays waiting for their sound to be decoded.
    waiting: Vec<PlayRequest>,
    /// Sounds whose waveform is wanted once they are decoded.
    waiting_waveforms: Vec<PathBuf>,
    voices: HashMap<VoiceId, ActiveVoice>,
    next_voice: VoiceId,
}

impl Controller {
    /// `playing` is kept up to date with the playing sounds, for the engine to read.
    pub fn new(
        cache: SoundCache,
        inject: MixerHandle,
        local: MixerHandle,
        events: mpsc::Sender<EngineEvent>,
        playing: Arc<Mutex<Vec<Playing>>>,
    ) -> Self {
        Self {
            cache,
            inject,
            local,
            events,
            playing,
            call_volume: 1.0,
            local_volume: 1.0,
            local_enabled: true,
            retrigger: Retrigger::default(),
            target: None,
            apps: Vec::new(),
            sound_ids: HashMap::new(),
            waiting: Vec::new(),
            waiting_waveforms: Vec::new(),
            voices: HashMap::new(),
            next_voice: 0,
        }
    }

    /// Whether any voice plays; then the mixers must be polled for finished voices.
    pub fn has_voices(&self) -> bool {
        !self.voices.is_empty()
    }

    /// Reacts to one message. [`Message::Quit`] is the run loop's business and ignored here.
    pub fn handle(&mut self, message: Message) {
        match message {
            Message::Play(request) => self.play(request),
            Message::Stop(path) => self.stop(&path),
            Message::StopAll => self.stop_all(),
            Message::StopPreview => self.stop_preview(),
            Message::LoadWaveform(path) => self.load_waveform(path),
            Message::Preload(paths) => self.preload(paths),
            Message::SetCallVolume(volume) => {
                self.call_volume = volume.clamp(0.0, 1.0);
                self.update_inject_gain();
            }
            Message::SetLocalVolume(volume) => {
                self.local_volume = volume.clamp(0.0, 1.0);
                self.send_local(Command::SetGain(self.local_volume));
            }
            Message::SetLocalEnabled(enabled) => self.set_local_enabled(enabled),
            Message::SetRetrigger(retrigger) => self.retrigger = retrigger,
            Message::Invalidate(path) => self.invalidate(&path),
            Message::SetTarget(target) => {
                self.target = target;
                self.update_inject_gain();
            }
            Message::PipeWire(event) => self.pipewire_event(event),
            Message::Decoded(decoded) => self.decoded(decoded),
            Message::Quit => {}
        }
    }

    /// Collects the voices the mixers finished, dropping their buffers here, off the real-time
    /// threads.
    pub fn poll_mixers(&mut self) {
        let finished: Vec<VoiceId> = [&mut self.inject, &mut self.local]
            .into_iter()
            .flat_map(|mixer| std::iter::from_fn(|| mixer.finished.pop().ok()))
            .map(|finished| finished.voice_id)
            .collect();
        for voice in finished {
            self.mixer_finished(voice);
        }
    }

    fn play(&mut self, request: PlayRequest) {
        match self.cache.get(&request.path) {
            Some(samples) => {
                let samples = Samples::clone(samples);
                self.start(request, samples);
            }
            None => {
                self.cache.load(&request.path);
                self.waiting.push(request);
            }
        }
    }

    /// Decodes `paths` in the background (after any sound someone waits for) and forgets the
    /// decoded sounds that aren't among them.
    fn preload(&mut self, paths: Vec<PathBuf>) {
        for path in &paths {
            self.cache.preload(path);
        }
        self.cache
            .retain(&paths.into_iter().collect::<HashSet<_>>());
    }

    fn decoded(&mut self, decoded: Decoded) {
        let path = decoded.path.clone();
        let Some(result) = self.cache.finish(decoded) else {
            return;
        };
        let (ready, still_waiting): (Vec<_>, _) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|request| request.path == path);
        self.waiting = still_waiting;
        let waveform_wanted = self.waiting_waveforms.contains(&path);
        self.waiting_waveforms.retain(|waiting| *waiting != path);
        match result {
            Ok(samples) => {
                for request in ready {
                    self.start(request, Samples::clone(&samples));
                }
                if waveform_wanted {
                    self.compute_waveform(path, samples);
                }
            }
            Err(error) => {
                if waveform_wanted {
                    let waveform = Err(error.to_string());
                    let path = path.clone();
                    self.emit(EngineEvent::Waveform { path, waveform });
                }
                if !ready.is_empty() {
                    self.emit(EngineEvent::DecodeFailed { path, error });
                }
            }
        }
    }

    fn load_waveform(&mut self, path: PathBuf) {
        match self.cache.get(&path) {
            Some(samples) => {
                let samples = Samples::clone(samples);
                self.compute_waveform(path, samples);
            }
            None => {
                self.cache.load(&path);
                self.waiting_waveforms.push(path);
            }
        }
    }

    /// Computes the waveform on a thread of its own: it takes a pass over the whole sound, which
    /// would hold up the control thread.
    fn compute_waveform(&self, path: PathBuf, samples: Samples) {
        let events = self.events.clone();
        let thread_path = path.clone();
        let spawned = thread::Builder::new()
            .name("waveform".to_owned())
            .spawn(move || {
                let waveform = Ok(Waveform::of(&samples));
                // Fails only if the engine is gone, and then nobody is listening anymore.
                let _ = events.send(EngineEvent::Waveform {
                    path: thread_path,
                    waveform,
                });
            });
        if let Err(error) = spawned {
            let waveform = Err(format!("could not start a thread: {error}"));
            self.emit(EngineEvent::Waveform { path, waveform });
        }
    }

    fn start(&mut self, request: PlayRequest, samples: Samples) {
        let (start_frame, end_frame) = trim_frames(request.trim, samples.len() / CHANNELS);
        let voice = self.next_voice;
        self.next_voice += 1;
        let (sound_id, retrigger) = match request.destination {
            Destination::Everywhere => (self.sound_id(&request.path), self.retrigger),
            Destination::Preview => (PREVIEW_SOUND, Retrigger::Restart),
        };
        let play = Play {
            voice_id: voice,
            sound_id,
            samples,
            start_frame,
            end_frame,
            gain: request.volume.max(0.0),
            retrigger,
        };
        let mixers = self.send_play(play, request.destination);
        if mixers == 0 {
            return;
        }
        let playing = Playing {
            voice,
            path: request.path,
            started_at: Instant::now(),
            duration: frames_to_duration(end_frame - start_frame),
            destination: request.destination,
        };
        self.voices.insert(
            voice,
            ActiveVoice {
                playing: playing.clone(),
                mixers,
            },
        );
        self.publish_playing();
        self.emit(EngineEvent::Started(playing));
    }

    /// Sends `play` to the mixers of `destination`; returns how many took it.
    fn send_play(&mut self, play: Play, destination: Destination) -> u8 {
        match destination {
            Destination::Everywhere => {
                let mut mixers = u8::from(self.send_inject(Command::Play(play.clone())));
                if self.local_enabled {
                    mixers += u8::from(self.send_local(Command::Play(play)));
                }
                mixers
            }
            Destination::Preview => u8::from(self.send_local(Command::Play(play))),
        }
    }

    fn stop(&mut self, path: &Path) {
        self.waiting.retain(|request| request.path != path);
        if let Some(&sound_id) = self.sound_ids.get(path) {
            self.send_inject(Command::Stop(sound_id));
            self.send_local(Command::Stop(sound_id));
        }
    }

    fn stop_all(&mut self) {
        self.waiting.clear();
        self.send_inject(Command::StopAll);
        self.send_local(Command::StopAll);
    }

    fn stop_preview(&mut self) {
        self.waiting
            .retain(|request| request.destination != Destination::Preview);
        self.send_local(Command::Stop(PREVIEW_SOUND));
    }

    fn set_local_enabled(&mut self, enabled: bool) {
        self.local_enabled = enabled;
        if !enabled {
            self.send_local(Command::StopAll);
        }
    }

    fn invalidate(&mut self, path: &Path) {
        self.cache.invalidate(path);
        let waited_for = self.waiting.iter().any(|request| request.path == path)
            || self.waiting_waveforms.iter().any(|waiting| waiting == path);
        if waited_for {
            self.cache.load(path);
        }
    }

    fn pipewire_event(&mut self, event: Event) {
        match event {
            Event::RecordingApps(apps) => {
                self.apps = apps.clone();
                self.update_inject_gain();
                self.emit(EngineEvent::RecordingApps(apps));
            }
            Event::Microphone(microphone) => self.emit(EngineEvent::Microphone(microphone)),
            Event::Error(message) => self.emit(EngineEvent::Error(message)),
        }
    }

    /// The call volume, halved for targets that sum our two channels into one: a centred sound
    /// would otherwise arrive at twice the level.
    fn update_inject_gain(&mut self) {
        let target = self
            .apps
            .iter()
            .find(|app| Some(app.node_id) == self.target);
        let compensation = match target {
            Some(app) if sums_channels(&app.ports) => 0.5,
            _ => 1.0,
        };
        self.send_inject(Command::SetGain(self.call_volume * compensation));
    }

    fn mixer_finished(&mut self, voice: VoiceId) {
        let Some(active) = self.voices.get_mut(&voice) else {
            return;
        };
        active.mixers -= 1;
        if active.mixers > 0 {
            return;
        }
        if let Some(active) = self.voices.remove(&voice) {
            self.publish_playing();
            self.emit(EngineEvent::Finished {
                voice,
                path: active.playing.path,
            });
        }
    }

    fn sound_id(&mut self, path: &Path) -> SoundId {
        let next = self.sound_ids.len() as SoundId;
        *self.sound_ids.entry(path.to_owned()).or_insert(next)
    }

    fn send_inject(&mut self, command: Command) -> bool {
        send(&mut self.inject, command, "voice chat", &self.events)
    }

    fn send_local(&mut self, command: Command) -> bool {
        send(&mut self.local, command, "local", &self.events)
    }

    fn publish_playing(&self) {
        let mut playing: Vec<Playing> = self
            .voices
            .values()
            .map(|active| active.playing.clone())
            .collect();
        playing.sort_by_key(|playing| playing.voice);
        *self.playing.lock().unwrap_or_else(PoisonError::into_inner) = playing;
    }

    fn emit(&self, event: EngineEvent) {
        // Fails only if the engine is gone, and then nobody is listening anymore.
        let _ = self.events.send(event);
    }
}

/// Queues `command` for a mixer; returns whether it fit. The ring only fills up if the stream
/// stopped rendering.
fn send(
    mixer: &mut MixerHandle,
    command: Command,
    stream: &str,
    events: &mpsc::Sender<EngineEvent>,
) -> bool {
    if mixer.commands.push(command).is_ok() {
        return true;
    }
    let message = format!("the {stream} stream isn't playing; a command was dropped");
    let _ = events.send(EngineEvent::Error(message));
    false
}

/// The frame range of a sound with `frames` frames to play for `trim`.
fn trim_frames(trim: Option<Trim>, frames: usize) -> (usize, usize) {
    let trim = trim.unwrap_or_default();
    let end = trim.end_ms.map_or(frames, ms_to_frames).min(frames);
    let start = ms_to_frames(trim.start_ms).min(end);
    (start, end)
}

fn ms_to_frames(ms: u64) -> usize {
    usize::try_from(ms.saturating_mul(u64::from(SAMPLE_RATE)) / 1000).unwrap_or(usize::MAX)
}

fn frames_to_duration(frames: usize) -> Duration {
    Duration::from_secs_f64(frames as f64 / f64::from(SAMPLE_RATE))
}

#[cfg(test)]
mod tests {
    use rtrb::{Consumer, Producer, RingBuffer};

    use super::*;
    use crate::audio::graph::Port;
    use crate::audio::mixer::Finished;

    const TIMEOUT: Duration = Duration::from_secs(5);

    /// The real-time ends of a mixer's rings, so tests can see commands and finish voices.
    struct FakeMixer {
        commands: Consumer<Command>,
        finished: Producer<Finished>,
    }

    impl FakeMixer {
        fn new() -> (Self, MixerHandle) {
            let (command_tx, command_rx) = RingBuffer::new(64);
            let (finished_tx, finished_rx) = RingBuffer::new(64);
            let fake = Self {
                commands: command_rx,
                finished: finished_tx,
            };
            let handle = MixerHandle {
                commands: command_tx,
                finished: finished_rx,
            };
            (fake, handle)
        }

        fn commands(&mut self) -> Vec<Command> {
            std::iter::from_fn(|| self.commands.pop().ok()).collect()
        }

        fn plays(&mut self) -> Vec<Play> {
            self.commands()
                .into_iter()
                .filter_map(|command| match command {
                    Command::Play(play) => Some(play),
                    _ => None,
                })
                .collect()
        }

        fn gains(&mut self) -> Vec<f32> {
            self.commands()
                .into_iter()
                .filter_map(|command| match command {
                    Command::SetGain(gain) => Some(gain),
                    _ => None,
                })
                .collect()
        }

        fn finish(&mut self, play: Play) {
            let finished = Finished {
                voice_id: play.voice_id,
                samples: play.samples,
            };
            self.finished.push(finished).expect("ring has room");
        }
    }

    struct Fixture {
        controller: Controller,
        inject: FakeMixer,
        local: FakeMixer,
        decoded: mpsc::Receiver<Decoded>,
        events: mpsc::Receiver<EngineEvent>,
        playing: Arc<Mutex<Vec<Playing>>>,
    }

    impl Fixture {
        /// A controller whose "decoder" yields one second of silence, or fails for `bad.wav`.
        fn new() -> Self {
            let (decoded_tx, decoded) = mpsc::channel();
            let decode = |path: &Path| {
                if path == Path::new("bad.wav") {
                    return Err(DecodeError::NoAudio);
                }
                Ok(Samples::from(vec![0.0; SAMPLE_RATE as usize * CHANNELS]))
            };
            let on_decoded = move |decoded| {
                let _ = decoded_tx.send(decoded);
            };
            let cache = SoundCache::new(decode, on_decoded).expect("spawns");
            let (inject, inject_handle) = FakeMixer::new();
            let (local, local_handle) = FakeMixer::new();
            let (events_tx, events) = mpsc::channel();
            let playing = Arc::default();
            let controller = Controller::new(
                cache,
                inject_handle,
                local_handle,
                events_tx,
                Arc::clone(&playing),
            );
            Self {
                controller,
                inject,
                local,
                decoded,
                events,
                playing,
            }
        }

        fn send(&mut self, message: Message) {
            self.controller.handle(message);
        }

        /// Plays `path` and lets its decode finish.
        fn play(&mut self, path: &str, trim: Option<Trim>) {
            self.send(Message::Play(PlayRequest {
                path: path.into(),
                trim,
                volume: 0.5,
                destination: Destination::Everywhere,
            }));
            self.finish_decode();
        }

        fn finish_decode(&mut self) {
            let decoded = self.decoded.recv_timeout(TIMEOUT).expect("decodes");
            self.send(Message::Decoded(decoded));
        }

        fn events(&self) -> Vec<EngineEvent> {
            self.events.try_iter().collect()
        }

        fn playing(&self) -> Vec<Playing> {
            self.playing.lock().expect("not poisoned").clone()
        }
    }

    #[test]
    fn plays_on_both_streams_once_decoded() {
        let mut fixture = Fixture::new();
        fixture.play("a.wav", None);

        let inject = fixture.inject.plays();
        let local = fixture.local.plays();
        assert_eq!(inject.len(), 1);
        assert_eq!(local.len(), 1);
        assert_eq!(inject[0].voice_id, local[0].voice_id);
        assert_eq!(inject[0].gain, 0.5);
        assert_eq!(
            (inject[0].start_frame, inject[0].end_frame),
            (0, SAMPLE_RATE as usize)
        );
        assert!(Arc::ptr_eq(&inject[0].samples, &local[0].samples));

        let events = fixture.events();
        let [EngineEvent::Started(playing)] = &events[..] else {
            panic!("{events:?}");
        };
        assert_eq!(playing.path, Path::new("a.wav"));
        assert_eq!(playing.duration, Duration::from_secs(1));
        assert_eq!(fixture.playing(), std::slice::from_ref(playing));
    }

    #[test]
    fn cached_sounds_play_right_away() {
        let mut fixture = Fixture::new();
        fixture.play("a.wav", None);
        fixture.send(Message::Play(PlayRequest {
            path: "a.wav".into(),
            trim: None,
            volume: 1.0,
            destination: Destination::Everywhere,
        }));
        assert_eq!(fixture.inject.plays().len(), 2);
    }

    #[test]
    fn preloaded_sounds_play_right_away_and_unlisted_ones_are_dropped() {
        let mut fixture = Fixture::new();
        fixture.send(Message::Preload(vec!["a.wav".into(), "bad.wav".into()]));
        fixture.finish_decode();
        fixture.finish_decode();
        assert!(fixture.events().is_empty(), "preload failures are silent");

        let play = PlayRequest {
            path: "a.wav".into(),
            trim: None,
            volume: 1.0,
            destination: Destination::Everywhere,
        };
        fixture.send(Message::Play(play.clone()));
        assert_eq!(fixture.inject.plays().len(), 1);

        fixture.send(Message::Preload(Vec::new()));
        fixture.send(Message::Play(play));
        assert!(fixture.inject.plays().is_empty(), "waits for a new decode");
    }

    #[test]
    fn trim_becomes_a_frame_range_and_duration() {
        let mut fixture = Fixture::new();
        let trim = Trim {
            start_ms: 250,
            end_ms: Some(750),
        };
        fixture.play("a.wav", Some(trim));
        let play = &fixture.inject.plays()[0];
        assert_eq!((play.start_frame, play.end_frame), (12_000, 36_000));
        assert_eq!(fixture.playing()[0].duration, Duration::from_millis(500));
    }

    #[test]
    fn trim_frames_are_clamped_to_the_sound() {
        let late = Trim {
            start_ms: 5_000,
            end_ms: None,
        };
        assert_eq!(trim_frames(Some(late), 100), (100, 100));
        let backwards = Trim {
            start_ms: 2,
            end_ms: Some(1),
        };
        assert_eq!(trim_frames(Some(backwards), 1_000), (48, 48));
    }

    #[test]
    fn local_playback_can_be_disabled() {
        let mut fixture = Fixture::new();
        fixture.send(Message::SetLocalEnabled(false));
        assert!(matches!(fixture.local.commands()[..], [Command::StopAll]));
        fixture.play("a.wav", None);
        assert_eq!(fixture.inject.plays().len(), 1);
        assert!(fixture.local.plays().is_empty());
    }

    #[test]
    fn a_sound_finishes_when_both_mixers_are_done() {
        let mut fixture = Fixture::new();
        fixture.play("a.wav", None);
        let play = fixture.inject.plays().remove(0);
        fixture.events();

        fixture.inject.finish(play.clone());
        fixture.controller.poll_mixers();
        assert!(fixture.events().is_empty());
        assert!(fixture.controller.has_voices());

        fixture.local.finish(play);
        fixture.controller.poll_mixers();
        let events = fixture.events();
        assert!(
            matches!(&events[..], [EngineEvent::Finished { voice: 0, path }] if path == Path::new("a.wav")),
            "{events:?}"
        );
        assert!(fixture.playing().is_empty());
        assert!(!fixture.controller.has_voices());
    }

    #[test]
    fn decode_failures_are_reported() {
        let mut fixture = Fixture::new();
        fixture.play("bad.wav", None);
        assert!(fixture.inject.plays().is_empty());
        let events = fixture.events();
        assert!(
            matches!(&events[..], [EngineEvent::DecodeFailed { path, .. }] if path == Path::new("bad.wav")),
            "{events:?}"
        );
    }

    #[test]
    fn stopping_a_sound_cancels_its_pending_play() {
        let mut fixture = Fixture::new();
        fixture.send(Message::Play(PlayRequest {
            path: "a.wav".into(),
            trim: None,
            volume: 1.0,
            destination: Destination::Everywhere,
        }));
        fixture.send(Message::Stop("a.wav".into()));
        fixture.finish_decode();
        assert!(fixture.inject.plays().is_empty());
    }

    #[test]
    fn stop_targets_the_sound_on_both_mixers() {
        let mut fixture = Fixture::new();
        fixture.play("a.wav", None);
        let sound_id = fixture.inject.plays()[0].sound_id;
        fixture.local.commands();
        fixture.send(Message::Stop("a.wav".into()));
        assert!(matches!(fixture.inject.commands()[..], [Command::Stop(id)] if id == sound_id));
        assert!(matches!(fixture.local.commands()[..], [Command::Stop(id)] if id == sound_id));
    }

    #[test]
    fn retrigger_policy_is_passed_to_the_mixers() {
        let mut fixture = Fixture::new();
        fixture.send(Message::SetRetrigger(Retrigger::Stop));
        fixture.play("a.wav", None);
        assert_eq!(fixture.inject.plays()[0].retrigger, Retrigger::Stop);
    }

    #[test]
    fn volumes_go_to_their_stream() {
        let mut fixture = Fixture::new();
        fixture.send(Message::SetCallVolume(0.25));
        fixture.send(Message::SetLocalVolume(2.0));
        assert_eq!(fixture.inject.gains(), [0.25]);
        assert_eq!(fixture.local.gains(), [1.0]);
    }

    #[test]
    fn call_volume_is_halved_for_targets_that_sum_channels() {
        let mut fixture = Fixture::new();
        let mono_app = RecordingApp {
            node_id: 7,
            app_name: "app".to_owned(),
            process_binary: None,
            media_name: None,
            ports: vec![Port {
                id: 8,
                channel: "MONO".to_owned(),
            }],
        };
        fixture.send(Message::PipeWire(Event::RecordingApps(vec![mono_app])));
        fixture.send(Message::SetCallVolume(0.8));
        fixture.send(Message::SetTarget(Some(7)));
        fixture.send(Message::SetTarget(None));
        assert_eq!(fixture.inject.gains(), [1.0, 0.8, 0.4, 0.8]);
        assert!(matches!(
            fixture.events()[..],
            [EngineEvent::RecordingApps(_)]
        ));
    }

    fn preview(path: &str) -> Message {
        Message::Play(PlayRequest {
            path: path.into(),
            trim: Some(Trim {
                start_ms: 250,
                end_ms: None,
            }),
            volume: 0.8,
            destination: Destination::Preview,
        })
    }

    #[test]
    fn previews_play_only_locally_even_with_local_playback_disabled() {
        let mut fixture = Fixture::new();
        fixture.send(Message::SetLocalEnabled(false));
        fixture.local.commands();
        fixture.send(preview("a.wav"));
        fixture.finish_decode();

        assert!(fixture.inject.plays().is_empty());
        let local = fixture.local.plays();
        let [play] = &local[..] else {
            panic!("{local:?}");
        };
        assert_eq!((play.start_frame, play.gain), (12_000, 0.8));
        assert_eq!(play.retrigger, Retrigger::Restart);
        assert_eq!(fixture.playing()[0].destination, Destination::Preview);

        fixture.local.finish(play.clone());
        fixture.controller.poll_mixers();
        assert!(!fixture.controller.has_voices(), "one mixer is enough");
    }

    #[test]
    fn previews_are_a_sound_of_their_own() {
        let mut fixture = Fixture::new();
        fixture.send(Message::SetRetrigger(Retrigger::Stop));
        fixture.play("a.wav", None);
        fixture.send(preview("a.wav"));
        let plays = fixture.local.plays();
        let [board, preview] = &plays[..] else {
            panic!("{plays:?}");
        };
        assert_ne!(preview.sound_id, board.sound_id);

        fixture.send(Message::StopPreview);
        let inject = fixture.inject.commands();
        assert!(
            !inject
                .iter()
                .any(|command| matches!(command, Command::Stop(_) | Command::StopAll)),
            "{inject:?}"
        );
        assert!(
            matches!(fixture.local.commands()[..], [Command::Stop(id)] if id == preview.sound_id)
        );
    }

    #[test]
    fn stopping_the_preview_cancels_a_pending_one() {
        let mut fixture = Fixture::new();
        fixture.send(preview("a.wav"));
        fixture.send(Message::StopPreview);
        fixture.finish_decode();
        assert!(fixture.local.plays().is_empty());
    }

    fn waveform_event(fixture: &Fixture) -> (PathBuf, Result<Waveform, String>) {
        loop {
            match fixture.events.recv_timeout(TIMEOUT).expect("a waveform") {
                EngineEvent::Waveform { path, waveform } => return (path, waveform),
                _ => continue,
            }
        }
    }

    #[test]
    fn waveforms_come_from_the_decoded_sound() {
        let mut fixture = Fixture::new();
        fixture.send(Message::LoadWaveform("a.wav".into()));
        fixture.finish_decode();
        let (path, waveform) = waveform_event(&fixture);
        assert_eq!(path, Path::new("a.wav"));
        assert_eq!(waveform.expect("decodes").duration, Duration::from_secs(1));

        // Now it's cached: no second decode.
        fixture.send(Message::LoadWaveform("a.wav".into()));
        assert!(waveform_event(&fixture).1.is_ok());
        assert!(fixture.decoded.try_recv().is_err());
    }

    #[test]
    fn waveforms_of_undecodable_sounds_are_errors() {
        let mut fixture = Fixture::new();
        fixture.send(Message::LoadWaveform("bad.wav".into()));
        fixture.finish_decode();
        let (_, waveform) = waveform_event(&fixture);
        assert!(waveform.is_err());
        assert!(fixture.events().is_empty(), "no play failed");
    }

    #[test]
    fn progress_runs_from_zero_to_one() {
        let start = Instant::now();
        let playing = Playing {
            voice: 0,
            path: "a.wav".into(),
            started_at: start,
            duration: Duration::from_secs(2),
            destination: Destination::Everywhere,
        };
        assert_eq!(playing.progress(start), 0.0);
        assert_eq!(playing.progress(start + Duration::from_secs(1)), 0.5);
        assert_eq!(playing.progress(start + Duration::from_secs(5)), 1.0);
    }
}
