//! Mixes the playing sounds ("voices") of one output stream. [`Mixer::render`] runs inside the
//! stream's real-time callback; the control side talks to it through a [`MixerHandle`] made of two
//! lock-free rings: commands go in, [`Finished`] voices come back out.
//!
//! Real-time rules: no allocation, locks, logging or blocking, and the mixer never drops a
//! [`Samples`] buffer: dropping the last `Arc` would free memory. Every buffer it was given goes
//! back to the control side inside a [`Finished`] event, which is dropped there.

use rtrb::{Consumer, Producer, RingBuffer};

use super::decode::Samples;
use super::format::CHANNELS;

/// Identifies a sound (a file) across voices, for retriggering and stopping.
pub type SoundId = u64;
/// Identifies one playback of a sound.
pub type VoiceId = u64;

/// How many voices a mixer plays at once; plays beyond that are rejected.
pub const VOICE_CAPACITY: usize = 64;
/// Capacity of both rings, comfortably above [`VOICE_CAPACITY`].
const RING_CAPACITY: usize = 256;
/// Below this level the limiter leaves samples untouched; above it, it bends them towards ±1.
const LIMITER_KNEE: f32 = 0.8;

/// What a play does when voices of the same sound are still playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Retrigger {
    /// Play another voice on top.
    #[default]
    Overlap,
    /// Stop the playing voices and start over.
    Restart,
    /// Stop the playing voices and don't start a new one: the trigger toggles the sound.
    Stop,
}

/// Starts a voice playing `samples[start_frame..end_frame]` (in frames; clamped to the buffer).
#[derive(Debug, Clone)]
pub struct Play {
    pub voice_id: VoiceId,
    pub sound_id: SoundId,
    pub samples: Samples,
    pub start_frame: usize,
    pub end_frame: usize,
    pub gain: f32,
    pub retrigger: Retrigger,
}

#[derive(Debug, Clone)]
pub enum Command {
    Play(Play),
    /// Stops every voice of this sound.
    Stop(SoundId),
    StopAll,
    /// The output volume, applied after the limiter; with `0.0..=1.0` the output stays within it.
    SetGain(f32),
}

/// A voice ended: it played out, was stopped, or was rejected (no free voice, or
/// [`Retrigger::Stop`]). Carries the voice's buffer back so it's dropped off the real-time thread.
#[derive(Debug)]
pub struct Finished {
    pub voice_id: VoiceId,
    pub samples: Samples,
}

/// The control side's ends of a mixer's rings.
pub struct MixerHandle {
    pub commands: Producer<Command>,
    pub finished: Consumer<Finished>,
}

pub struct Mixer {
    /// Fixed-size; a slot is `None` when free. Ended voices keep their slot until their buffer
    /// has been handed back.
    voices: Vec<Option<Voice>>,
    commands: Consumer<Command>,
    finished: Producer<Finished>,
    gain: f32,
}

impl Mixer {
    /// A mixer with output volume `gain`, and the handle to control it. Allocates; call it off the
    /// real-time thread.
    pub fn new(gain: f32) -> (Self, MixerHandle) {
        let (command_tx, command_rx) = RingBuffer::new(RING_CAPACITY);
        let (finished_tx, finished_rx) = RingBuffer::new(RING_CAPACITY);
        let mixer = Self {
            voices: (0..VOICE_CAPACITY).map(|_| None).collect(),
            commands: command_rx,
            finished: finished_tx,
            gain,
        };
        let handle = MixerHandle {
            commands: command_tx,
            finished: finished_rx,
        };
        (mixer, handle)
    }

    /// Renders the next interleaved stereo frames into `out`. Real-time safe.
    pub fn render(&mut self, out: &mut [f32]) {
        self.apply_commands();
        out.fill(0.0);
        for voice in self.voices.iter_mut().flatten() {
            voice.mix_into(out);
        }
        limit(out, self.gain);
        self.hand_back_ended_voices();
    }

    /// Applies queued commands. Each command emits at most one [`Finished`] event right away (a
    /// rejected play), so commands are only taken while the ring has room for it.
    fn apply_commands(&mut self) {
        while self.finished.slots() > 0 {
            let Ok(command) = self.commands.pop() else {
                return;
            };
            match command {
                Command::Play(play) => self.play(play),
                Command::Stop(sound_id) => {
                    self.stop_sound(sound_id);
                }
                Command::StopAll => self.voices.iter_mut().flatten().for_each(Voice::end),
                Command::SetGain(gain) => self.gain = gain,
            }
        }
    }

    fn play(&mut self, play: Play) {
        let rejected = match play.retrigger {
            Retrigger::Overlap => false,
            Retrigger::Restart => {
                self.stop_sound(play.sound_id);
                false
            }
            Retrigger::Stop => self.stop_sound(play.sound_id),
        };
        let free_slot = self.voices.iter_mut().find(|slot| slot.is_none());
        match free_slot {
            Some(slot) if !rejected => *slot = Some(Voice::new(play)),
            _ => self.reject(play),
        }
    }

    fn reject(&mut self, play: Play) {
        let finished = Finished {
            voice_id: play.voice_id,
            samples: play.samples,
        };
        // Can't fail: `apply_commands` only takes a command while the ring has a free slot.
        let _ = self.finished.push(finished);
    }

    /// Ends the playing voices of `sound_id`; returns whether there were any.
    fn stop_sound(&mut self, sound_id: SoundId) -> bool {
        let mut stopped = false;
        for voice in self.voices.iter_mut().flatten() {
            if voice.sound_id == sound_id && !voice.ended() {
                voice.end();
                stopped = true;
            }
        }
        stopped
    }

    /// Frees the slots of ended voices, handing their buffers back. Voices that don't fit into the
    /// ring right now keep their slot (silently) and are retried next time.
    fn hand_back_ended_voices(&mut self) {
        for slot in &mut self.voices {
            if !slot.as_ref().is_some_and(Voice::ended) {
                continue;
            }
            if self.finished.slots() == 0 {
                return;
            }
            if let Some(voice) = slot.take() {
                // Can't fail: we just checked there's a free slot.
                let _ = self.finished.push(voice.into_finished());
            }
        }
    }
}

struct Voice {
    id: VoiceId,
    sound_id: SoundId,
    samples: Samples,
    /// Next frame to play.
    position: usize,
    /// Frame to stop before.
    end: usize,
    gain: f32,
}

impl Voice {
    fn new(play: Play) -> Self {
        let end = play.end_frame.min(play.samples.len() / CHANNELS);
        Self {
            id: play.voice_id,
            sound_id: play.sound_id,
            position: play.start_frame.min(end),
            end,
            gain: play.gain,
            samples: play.samples,
        }
    }

    fn ended(&self) -> bool {
        self.position >= self.end
    }

    fn end(&mut self) {
        self.position = self.end;
    }

    fn mix_into(&mut self, out: &mut [f32]) {
        let frames = (out.len() / CHANNELS).min(self.end - self.position);
        let source = &self.samples[self.position * CHANNELS..][..frames * CHANNELS];
        for (out, sample) in out.iter_mut().zip(source) {
            *out += sample * self.gain;
        }
        self.position += frames;
    }

    fn into_finished(self) -> Finished {
        Finished {
            voice_id: self.id,
            samples: self.samples,
        }
    }
}

/// Soft-clips every sample into ±1, then applies the output `gain`.
fn limit(out: &mut [f32], gain: f32) {
    for sample in out {
        *sample = soft_clip(*sample) * gain;
    }
}

/// Linear up to [`LIMITER_KNEE`], then a `tanh` curve that approaches ±1 and never exceeds it. The
/// slope is continuous at the knee, so there's no audible edge.
fn soft_clip(x: f32) -> f32 {
    let magnitude = x.abs();
    if magnitude <= LIMITER_KNEE {
        return x;
    }
    let headroom = 1.0 - LIMITER_KNEE;
    let bent = LIMITER_KNEE + headroom * ((magnitude - LIMITER_KNEE) / headroom).tanh();
    bent.copysign(x)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A sound of `frames` frames whose left and right samples are both `value`.
    fn constant(frames: usize, value: f32) -> Samples {
        vec![value; frames * CHANNELS].into()
    }

    /// A sound whose frame `i` has the value `i / 1000` on both channels.
    fn ramp(frames: usize) -> Samples {
        (0..frames)
            .flat_map(|i| [i as f32 / 1000.0; CHANNELS])
            .collect::<Vec<_>>()
            .into()
    }

    fn play(voice_id: VoiceId, sound_id: SoundId, samples: &Samples) -> Play {
        Play {
            voice_id,
            sound_id,
            samples: Arc::clone(samples),
            start_frame: 0,
            end_frame: usize::MAX,
            gain: 1.0,
            retrigger: Retrigger::Overlap,
        }
    }

    struct Fixture {
        mixer: Mixer,
        handle: MixerHandle,
    }

    impl Fixture {
        fn new() -> Self {
            let (mixer, handle) = Mixer::new(1.0);
            Self { mixer, handle }
        }

        fn send(&mut self, command: Command) {
            self.handle.commands.push(command).expect("ring has room");
        }

        fn play(&mut self, play: Play) {
            self.send(Command::Play(play));
        }

        /// Renders `frames` frames and returns the left channel.
        fn render(&mut self, frames: usize) -> Vec<f32> {
            let mut out = vec![f32::NAN; frames * CHANNELS];
            self.mixer.render(&mut out);
            out.iter().step_by(CHANNELS).copied().collect()
        }

        fn finished(&mut self) -> Vec<VoiceId> {
            std::iter::from_fn(|| self.handle.finished.pop().ok())
                .map(|finished| finished.voice_id)
                .collect()
        }
    }

    fn assert_close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-6, "{actual:?} vs {expected:?}");
        }
    }

    #[test]
    fn renders_silence_without_voices() {
        let mut fixture = Fixture::new();
        assert_close(&fixture.render(4), &[0.0; 4]);
    }

    #[test]
    fn mixes_two_voices_with_their_gains() {
        let mut fixture = Fixture::new();
        let sound = constant(8, 0.2);
        fixture.play(play(1, 1, &sound));
        fixture.play(Play {
            gain: 0.5,
            ..play(2, 2, &sound)
        });
        assert_close(&fixture.render(2), &[0.3, 0.3]);
    }

    #[test]
    fn output_gain_scales_the_mix() {
        let mut fixture = Fixture::new();
        fixture.send(Command::SetGain(0.5));
        fixture.play(play(1, 1, &constant(8, 0.4)));
        assert_close(&fixture.render(2), &[0.2, 0.2]);
    }

    #[test]
    fn plays_only_the_trimmed_range() {
        let mut fixture = Fixture::new();
        fixture.play(Play {
            start_frame: 2,
            end_frame: 5,
            ..play(1, 1, &ramp(10))
        });
        assert_close(&fixture.render(5), &[0.002, 0.003, 0.004, 0.0, 0.0]);
        assert_eq!(fixture.finished(), [1]);
    }

    #[test]
    fn trim_beyond_the_buffer_is_clamped() {
        let mut fixture = Fixture::new();
        fixture.play(Play {
            start_frame: 20,
            end_frame: 30,
            ..play(1, 1, &ramp(10))
        });
        assert_close(&fixture.render(2), &[0.0, 0.0]);
        assert_eq!(fixture.finished(), [1]);
    }

    #[test]
    fn a_voice_continues_across_renders() {
        let mut fixture = Fixture::new();
        fixture.play(play(1, 1, &ramp(4)));
        assert_close(&fixture.render(2), &[0.0, 0.001]);
        assert!(fixture.finished().is_empty());
        assert_close(&fixture.render(3), &[0.002, 0.003, 0.0]);
        assert_eq!(fixture.finished(), [1]);
    }

    #[test]
    fn overlap_plays_the_same_sound_twice() {
        let mut fixture = Fixture::new();
        let sound = constant(8, 0.1);
        fixture.play(play(1, 7, &sound));
        fixture.render(1);
        fixture.play(play(2, 7, &sound));
        assert_close(&fixture.render(1), &[0.2]);
        assert!(fixture.finished().is_empty());
    }

    #[test]
    fn restart_replaces_the_playing_voice() {
        let mut fixture = Fixture::new();
        let sound = ramp(8);
        fixture.play(play(1, 7, &sound));
        fixture.render(3);
        fixture.play(Play {
            retrigger: Retrigger::Restart,
            ..play(2, 7, &sound)
        });
        assert_close(&fixture.render(2), &[0.0, 0.001]);
        assert_eq!(fixture.finished(), [1]);
    }

    #[test]
    fn stop_retrigger_stops_a_playing_sound_instead_of_playing_it() {
        let mut fixture = Fixture::new();
        let sound = constant(8, 0.1);
        fixture.play(play(1, 7, &sound));
        fixture.render(1);
        fixture.play(Play {
            retrigger: Retrigger::Stop,
            ..play(2, 7, &sound)
        });
        assert_close(&fixture.render(1), &[0.0]);
        let mut finished = fixture.finished();
        finished.sort_unstable();
        assert_eq!(finished, [1, 2]);
    }

    #[test]
    fn stop_retrigger_plays_a_sound_that_is_not_playing() {
        let mut fixture = Fixture::new();
        fixture.play(Play {
            retrigger: Retrigger::Stop,
            ..play(1, 7, &constant(8, 0.1))
        });
        assert_close(&fixture.render(1), &[0.1]);
    }

    #[test]
    fn retriggering_ignores_other_sounds() {
        let mut fixture = Fixture::new();
        fixture.play(play(1, 7, &constant(8, 0.1)));
        fixture.play(Play {
            retrigger: Retrigger::Stop,
            ..play(2, 8, &constant(8, 0.2))
        });
        assert_close(&fixture.render(1), &[0.3]);
    }

    #[test]
    fn stop_ends_all_voices_of_a_sound() {
        let mut fixture = Fixture::new();
        let sound = constant(8, 0.1);
        fixture.play(play(1, 7, &sound));
        fixture.play(play(2, 7, &sound));
        fixture.play(play(3, 8, &constant(8, 0.2)));
        fixture.render(1);
        fixture.send(Command::Stop(7));
        assert_close(&fixture.render(1), &[0.2]);
        let mut finished = fixture.finished();
        finished.sort_unstable();
        assert_eq!(finished, [1, 2]);
    }

    #[test]
    fn stop_all_ends_every_voice() {
        let mut fixture = Fixture::new();
        fixture.play(play(1, 7, &constant(8, 0.1)));
        fixture.play(play(2, 8, &constant(8, 0.2)));
        fixture.render(1);
        fixture.send(Command::StopAll);
        assert_close(&fixture.render(1), &[0.0]);
        assert_eq!(fixture.finished().len(), 2);
    }

    #[test]
    fn plays_beyond_capacity_are_rejected() {
        let mut fixture = Fixture::new();
        let sound = constant(8, 0.001);
        for voice_id in 0..VOICE_CAPACITY as VoiceId + 1 {
            fixture.play(play(voice_id, voice_id, &sound));
        }
        let left = fixture.render(1);
        assert_close(&left, &[VOICE_CAPACITY as f32 * 0.001]);
        assert_eq!(fixture.finished(), [VOICE_CAPACITY as VoiceId]);
    }

    #[test]
    fn a_freed_slot_can_be_reused() {
        let mut fixture = Fixture::new();
        let sound = constant(1, 0.1);
        for voice_id in 0..VOICE_CAPACITY as VoiceId {
            fixture.play(play(voice_id, voice_id, &sound));
        }
        fixture.render(1);
        assert_eq!(fixture.finished().len(), VOICE_CAPACITY);
        fixture.play(play(100, 100, &constant(4, 0.1)));
        assert_close(&fixture.render(1), &[0.1]);
    }

    #[test]
    fn limiter_keeps_the_output_within_gain() {
        let mut fixture = Fixture::new();
        let loud = constant(8, 0.9);
        for voice_id in 0..10 {
            fixture.play(play(voice_id, voice_id, &loud));
        }
        let left = fixture.render(2);
        assert!(left.iter().all(|s| s.abs() <= 1.0), "{left:?}");

        fixture.send(Command::SetGain(0.5));
        let left = fixture.render(2);
        assert!(left.iter().all(|s| s.abs() <= 0.5), "{left:?}");
    }

    #[test]
    fn soft_clip_is_transparent_below_the_knee_and_bounded_above() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-LIMITER_KNEE), -LIMITER_KNEE);
        assert!((LIMITER_KNEE..1.0).contains(&soft_clip(0.9)));
        assert!((-1.0..-LIMITER_KNEE).contains(&soft_clip(-0.9)));
        assert!(soft_clip(100.0) <= 1.0);
        assert!(soft_clip(1.0) > soft_clip(0.9));
    }

    #[test]
    fn buffers_are_handed_back_not_dropped() {
        let mut fixture = Fixture::new();
        let sound = constant(2, 0.1);
        fixture.play(play(1, 1, &sound));
        fixture.play(Play {
            retrigger: Retrigger::Stop,
            ..play(2, 1, &sound)
        });
        fixture.render(4);
        // Both voices ended; their clones live in the ring, none was dropped by the mixer.
        assert_eq!(Arc::strong_count(&sound), 3);
        let finished: Vec<Finished> =
            std::iter::from_fn(|| fixture.handle.finished.pop().ok()).collect();
        assert_eq!(finished.len(), 2);
        assert!(finished.iter().all(|f| Arc::ptr_eq(&f.samples, &sound)));
        drop(finished);
        assert_eq!(Arc::strong_count(&sound), 1);
    }

    #[test]
    fn ended_voices_and_commands_wait_while_the_finished_ring_is_full() {
        let mut fixture = Fixture::new();
        let sound = constant(2, 0.1);
        fixture.play(play(1, 1, &sound));
        fixture.render(1);

        let filler = constant(1, 0.0);
        for voice_id in 100..100 + RING_CAPACITY as VoiceId {
            let finished = Finished {
                voice_id,
                samples: Arc::clone(&filler),
            };
            fixture
                .mixer
                .finished
                .push(finished)
                .expect("ring has room");
        }
        fixture.play(play(2, 2, &constant(4, 0.3)));
        // The voice ends but stays in its slot; the play stays queued.
        assert_close(&fixture.render(2), &[0.1, 0.0]);
        assert_eq!(Arc::strong_count(&sound), 2);

        assert_eq!(fixture.finished().len(), RING_CAPACITY);
        assert_close(&fixture.render(1), &[0.3]);
        assert_eq!(fixture.finished(), [1]);
    }
}
