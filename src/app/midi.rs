//! MIDI in the app: learning bindings and turning triggers into the actions a click would cause.

use std::path::Path;

use crate::midi::{MidiBinding, MidiDevice, MidiTrigger};

use super::{Action, AppState};

/// What the next trigger is learned for while learning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnTarget {
    /// The sound open in the "Edit sound" window.
    Sound,
    StopAll,
}

/// What the user did about MIDI (see [`Action::Midi`]).
#[derive(Debug, Clone, PartialEq)]
pub enum MidiAction {
    SelectInput(MidiDevice),
    /// Capture the next trigger instead of acting on it.
    Learn(LearnTarget),
    CancelLearn,
    ClearStopAll,
}

#[derive(Debug, Default)]
pub struct MidiState {
    /// The names of the input ports there are.
    pub ports: Vec<String>,
    /// The names of the ports listened to.
    pub connected: Vec<String>,
    pub learning: Option<LearnTarget>,
}

impl MidiState {
    /// While learning, captures the trigger and stops learning; returns what it was learned for.
    pub fn learn(&mut self, trigger: &MidiTrigger) -> Option<(LearnTarget, MidiBinding)> {
        let target = self.learning.take()?;
        Some((target, MidiBinding::learned(trigger)))
    }

    /// Stops learning for `target` (e.g. because its window closed).
    pub fn stop_learning(&mut self, target: LearnTarget) {
        if self.learning == Some(target) {
            self.learning = None;
        }
    }
}

impl AppState {
    /// What a trigger does (when not learning): "Stop all" if bound to it, then play every sound
    /// bound to it.
    pub fn midi_actions(&self, trigger: &MidiTrigger) -> Vec<Action> {
        let stop_all = self.config.settings.midi_stop_all.as_ref();
        let stop = stop_all.is_some_and(|binding| binding.matches(trigger));
        let plays = self
            .bound_sounds()
            .filter(|(_, binding)| binding.matches(trigger))
            .map(|(path, _)| Action::Play(path.to_owned()));
        stop.then_some(Action::StopAll)
            .into_iter()
            .chain(plays)
            .collect()
    }

    /// The names of the sounds, other than the one at `except`, that a trigger of `binding` would
    /// play as well.
    pub fn sounds_bound_like(&self, binding: &MidiBinding, except: Option<&Path>) -> Vec<String> {
        self.bound_sounds()
            .filter(|(path, other)| Some(*path) != except && other.overlaps(binding))
            .filter_map(|(path, _)| self.library.find(path))
            .map(|(_, sound)| sound.name.clone())
            .collect()
    }

    /// Every sound in the library that has a MIDI binding.
    fn bound_sounds(&self) -> impl Iterator<Item = (&Path, &MidiBinding)> {
        let folders = self.library.folders().iter().zip(&self.config.tabs);
        folders.flat_map(|(folder, tab)| {
            folder.sounds.iter().filter_map(|sound| {
                let binding = tab.sounds.get(&sound.key)?.midi.as_ref()?;
                Some((sound.path.as_path(), binding))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::config::{Config, SoundConfig, TabConfig};
    use crate::midi::MidiControl;

    const MPD: &str = "MPD218:MPD218 MIDI 1";

    fn trigger(control: MidiControl) -> MidiTrigger {
        MidiTrigger {
            port: MPD.to_owned(),
            channel: 1,
            control,
        }
    }

    fn binding(port: Option<&str>, control: MidiControl) -> MidiBinding {
        MidiBinding {
            port: port.map(str::to_owned),
            channel: 1,
            control,
        }
    }

    struct Fixture {
        dir: tempfile::TempDir,
        state: AppState,
    }

    impl Fixture {
        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }
    }

    /// One tab: Airhorn on note 36 from the MPD, Bruh on note 36 from any port, Horn on CC 20,
    /// Crickets unbound. Stop all is note 40.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut tab = TabConfig::for_folder(dir.path().to_owned());
        let bindings = [
            (
                "Airhorn.wav",
                Some(binding(Some(MPD), MidiControl::Note(36))),
            ),
            ("Bruh.wav", Some(binding(None, MidiControl::Note(36)))),
            (
                "Horn.wav",
                Some(binding(None, MidiControl::ControlChange(20))),
            ),
            ("Crickets.wav", None),
        ];
        for (name, midi) in bindings {
            fs::write(dir.path().join(name), b"").expect("write");
            let meta = SoundConfig {
                midi,
                ..SoundConfig::default()
            };
            tab.sounds.insert(name.to_owned(), meta);
        }
        let mut config = Config {
            tabs: vec![tab],
            ..Config::default()
        };
        config.settings.midi_stop_all = Some(binding(None, MidiControl::Note(40)));
        Fixture {
            state: AppState::new(config),
            dir,
        }
    }

    #[test]
    fn triggers_play_every_sound_bound_to_them() {
        let f = fixture();
        assert_eq!(
            f.state.midi_actions(&trigger(MidiControl::Note(36))),
            [
                Action::Play(f.path("Airhorn.wav")),
                Action::Play(f.path("Bruh.wav"))
            ]
        );
        let elsewhere = MidiTrigger {
            port: "BeatStep:BeatStep MIDI 1".to_owned(),
            ..trigger(MidiControl::Note(36))
        };
        assert_eq!(
            f.state.midi_actions(&elsewhere),
            [Action::Play(f.path("Bruh.wav"))]
        );
        assert_eq!(
            f.state
                .midi_actions(&trigger(MidiControl::ControlChange(20))),
            [Action::Play(f.path("Horn.wav"))]
        );
        assert!(
            f.state
                .midi_actions(&trigger(MidiControl::Note(1)))
                .is_empty()
        );
    }

    #[test]
    fn the_stop_all_binding_stops_all() {
        let f = fixture();
        assert_eq!(
            f.state.midi_actions(&trigger(MidiControl::Note(40))),
            [Action::StopAll]
        );
    }

    #[test]
    fn learning_captures_one_trigger() {
        let mut midi = MidiState::default();
        let note = trigger(MidiControl::Note(36));
        assert_eq!(midi.learn(&note), None, "not learning");

        midi.learning = Some(LearnTarget::Sound);
        assert_eq!(
            midi.learn(&note),
            Some((
                LearnTarget::Sound,
                binding(Some(MPD), MidiControl::Note(36))
            ))
        );
        assert_eq!(midi.learning, None);
        assert_eq!(midi.learn(&note), None, "only the first trigger is learned");
    }

    #[test]
    fn learning_stops_only_for_its_target() {
        let mut midi = MidiState {
            learning: Some(LearnTarget::StopAll),
            ..MidiState::default()
        };
        midi.stop_learning(LearnTarget::Sound);
        assert_eq!(midi.learning, Some(LearnTarget::StopAll));
        midi.stop_learning(LearnTarget::StopAll);
        assert_eq!(midi.learning, None);
    }

    #[test]
    fn sounds_with_overlapping_bindings_are_named() {
        let f = fixture();
        let note = binding(Some(MPD), MidiControl::Note(36));
        assert_eq!(f.state.sounds_bound_like(&note, None), ["Airhorn", "Bruh"]);
        let airhorn = f.path("Airhorn.wav");
        assert_eq!(f.state.sounds_bound_like(&note, Some(&airhorn)), ["Bruh"]);
        let other_port = binding(Some("Other:Other"), MidiControl::Note(36));
        assert_eq!(f.state.sounds_bound_like(&other_port, None), ["Bruh"]);
        let free = binding(None, MidiControl::Note(50));
        assert!(f.state.sounds_bound_like(&free, None).is_empty());
    }
}
