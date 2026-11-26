//! Global hotkeys and MIDI triggers. They are handled on their own `triggers` thread, not in the
//! frame loop, so they play while the window gets no frames (hidden, minimized, on another
//! workspace). The UI keeps a [`TriggerTable`] up to date; the thread resolves every [`Trigger`]
//! against it and tells the audio engine, or hands the trigger to the UI while a MIDI binding is
//! being learned.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, PoisonError, RwLock, mpsc};
use std::thread;

use crate::audio::PlayRequest;
use crate::midi::{MidiBinding, MidiTrigger};

use super::AppState;
use super::hotkeys::{self, Target};
use super::services::Player;

/// Something the user pressed or hit outside the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// The global shortcut with this id was pressed.
    Hotkey(String),
    Midi(MidiTrigger),
}

/// What a trigger makes the audio engine do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Play(PlayRequest),
    StopAll,
}

impl Command {
    fn run(&self, player: &dyn Player) {
        match self {
            Self::Play(request) => player.play(request.clone()),
            Self::StopAll => player.stop_all(),
        }
    }
}

/// What to do about a trigger.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// Tell the audio engine; nothing to do if empty.
    Run(Vec<Command>),
    /// A MIDI binding is being learned: the UI captures the trigger.
    Learn(MidiTrigger),
}

/// What every trigger does, resolved from the config and the library (see
/// [`AppState::trigger_table`]), so the `triggers` thread needs nothing else.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriggerTable {
    /// By shortcut id.
    hotkeys: HashMap<String, Command>,
    /// "Stop all" first, so a trigger bound to it and to sounds stops before it plays.
    midi: Vec<(MidiBinding, Command)>,
    learning_midi: bool,
    /// A hotkey is being recorded: presses of the global shortcuts don't play meanwhile.
    recording_hotkey: bool,
}

impl TriggerTable {
    pub fn resolve(&self, trigger: &Trigger) -> Resolution {
        match trigger {
            Trigger::Hotkey(_) if self.recording_hotkey => Resolution::Run(Vec::new()),
            Trigger::Hotkey(id) => {
                Resolution::Run(self.hotkeys.get(id).cloned().into_iter().collect())
            }
            Trigger::Midi(trigger) if self.learning_midi => Resolution::Learn(trigger.clone()),
            Trigger::Midi(trigger) => Resolution::Run(
                self.midi
                    .iter()
                    .filter(|(binding, _)| binding.matches(trigger))
                    .map(|(_, command)| command.clone())
                    .collect(),
            ),
        }
    }
}

impl AppState {
    /// What hotkeys and MIDI triggers do right now: play their clips with the clips' volume and
    /// trim, or "Stop all"; while learning or recording, capture instead.
    pub fn trigger_table(&self) -> TriggerTable {
        let hotkeys = hotkeys::bindings(&self.config)
            .into_iter()
            .filter_map(|(binding, target)| Some((binding.id, self.command(&target)?)))
            .collect();
        let stop_all = self.config.settings.midi_stop_all.clone();
        let stop_all = stop_all.map(|binding| (binding, Command::StopAll));
        let plays = self.bound_clips().filter_map(|(clip, binding)| {
            let request = self.play_request(&clip)?;
            Some((binding.clone(), Command::Play(request)))
        });
        TriggerTable {
            hotkeys,
            midi: stop_all.into_iter().chain(plays).collect(),
            learning_midi: self.midi.learning.is_some(),
            recording_hotkey: self.recording_stop_all
                || self.editor.as_ref().is_some_and(|editor| editor.recording),
        }
    }

    /// `None` for a clip that isn't in the library (anymore).
    fn command(&self, target: &Target) -> Option<Command> {
        match target {
            Target::Play(clip) => self.play_request(clip).map(Command::Play),
            Target::StopAll => Some(Command::StopAll),
        }
    }
}

/// The app's handle on the `triggers` thread, which runs until every [`Trigger`] sender is gone.
pub struct Dispatcher {
    table: Arc<RwLock<TriggerTable>>,
    learned: mpsc::Receiver<MidiTrigger>,
}

impl Dispatcher {
    /// Handles what arrives on `triggers` with `player` (`None` without audio: triggers do
    /// nothing). `wake` is called after every trigger that did something, so the UI shows it.
    pub fn start(
        triggers: mpsc::Receiver<Trigger>,
        player: Option<Box<dyn Player>>,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        let table = Arc::<RwLock<TriggerTable>>::default();
        let (learned_tx, learned) = mpsc::channel();
        let shared = Arc::clone(&table);
        thread::Builder::new()
            .name("triggers".to_owned())
            .spawn(move || {
                for trigger in triggers {
                    let table = shared.read().unwrap_or_else(PoisonError::into_inner);
                    let resolution = table.resolve(&trigger);
                    drop(table);
                    if handle(resolution, player.as_deref(), &learned_tx) {
                        wake();
                    }
                }
            })?;
        Ok(Self { table, learned })
    }

    /// Triggers do what `table` says from now on.
    pub fn set_table(&self, table: TriggerTable) {
        *self.table.write().unwrap_or_else(PoisonError::into_inner) = table;
    }

    /// The triggers captured for learning since the last call.
    pub fn learned(&self) -> Vec<MidiTrigger> {
        self.learned.try_iter().collect()
    }
}

/// Carries out `resolution`; returns whether it did anything.
fn handle(
    resolution: Resolution,
    player: Option<&dyn Player>,
    learned: &mpsc::Sender<MidiTrigger>,
) -> bool {
    match resolution {
        Resolution::Run(commands) => {
            if let Some(player) = player {
                commands.iter().for_each(|command| command.run(player));
            }
            !commands.is_empty()
        }
        // Fails only if the app is gone.
        Resolution::Learn(trigger) => learned.send(trigger).is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::app::LearnTarget;
    use crate::audio::{Destination, Trim};
    use crate::config::{ClipConfig, Config, SoundConfig, TabConfig};
    use crate::midi::MidiControl;

    const MPD: &str = "MPD218:MPD218 MIDI 1";

    fn midi(control: MidiControl) -> Trigger {
        Trigger::Midi(MidiTrigger {
            port: MPD.to_owned(),
            channel: 1,
            control,
        })
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
        fn play(&self, name: &str) -> Command {
            Command::Play(PlayRequest {
                path: self.dir.path().join(name),
                clip: 0,
                trim: None,
                volume: 1.0,
                destination: Destination::Everywhere,
            })
        }
    }

    /// One tab: Airhorn on note 36 from the MPD and the shortcut `play-airhorn` (trimmed, half
    /// volume), Bruh on note 36 from any port, Horn on CC 20, Crickets unbound. Stop all is note 40
    /// and the shortcut `stop-all`. `Gone.wav` is configured but not in the folder.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut tab = TabConfig::for_folder(dir.path().to_owned());
        let sounds = [
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
        for (name, midi) in sounds {
            fs::write(dir.path().join(name), b"").expect("write");
            let meta = SoundConfig {
                midi,
                ..SoundConfig::default()
            };
            tab.sounds.insert(name.to_owned(), meta);
        }
        let airhorn = tab.sounds.get_mut("Airhorn.wav").expect("inserted");
        airhorn.shortcut_id = Some("play-airhorn".to_owned());
        airhorn.volume = 0.5;
        airhorn.start_ms = 100;
        tab.sounds.insert(
            "Gone.wav".to_owned(),
            SoundConfig {
                shortcut_id: Some("play-gone".to_owned()),
                midi: Some(binding(None, MidiControl::Note(50))),
                ..SoundConfig::default()
            },
        );
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

    fn airhorn(f: &Fixture) -> Command {
        Command::Play(PlayRequest {
            path: f.dir.path().join("Airhorn.wav"),
            clip: 0,
            trim: Some(Trim {
                start_ms: 100,
                end_ms: None,
            }),
            volume: 0.5,
            destination: Destination::Everywhere,
        })
    }

    fn run(commands: Vec<Command>) -> Resolution {
        Resolution::Run(commands)
    }

    #[test]
    fn hotkeys_play_their_sound_with_its_volume_and_trim() {
        let f = fixture();
        let table = f.state.trigger_table();
        let hotkey = |id: &str| table.resolve(&Trigger::Hotkey(id.to_owned()));
        assert_eq!(hotkey("play-airhorn"), run(vec![airhorn(&f)]));
        assert_eq!(hotkey("stop-all"), run(vec![Command::StopAll]));
        assert_eq!(hotkey("unknown"), run(vec![]));
        assert_eq!(hotkey("play-gone"), run(vec![]), "not in the library");
    }

    #[test]
    fn midi_triggers_play_every_sound_bound_to_them() {
        let f = fixture();
        let table = f.state.trigger_table();
        assert_eq!(
            table.resolve(&midi(MidiControl::Note(36))),
            run(vec![airhorn(&f), f.play("Bruh.wav")])
        );
        let elsewhere = Trigger::Midi(MidiTrigger {
            port: "BeatStep:BeatStep MIDI 1".to_owned(),
            channel: 1,
            control: MidiControl::Note(36),
        });
        assert_eq!(table.resolve(&elsewhere), run(vec![f.play("Bruh.wav")]));
        assert_eq!(
            table.resolve(&midi(MidiControl::ControlChange(20))),
            run(vec![f.play("Horn.wav")])
        );
        assert_eq!(table.resolve(&midi(MidiControl::Note(1))), run(vec![]));
        assert_eq!(table.resolve(&midi(MidiControl::Note(50))), run(vec![]));
    }

    #[test]
    fn more_clips_are_triggered_with_their_own_trim_and_volume() {
        let mut f = fixture();
        let crickets = f.state.config.tabs[0].sounds.get_mut("Crickets.wav");
        crickets.expect("configured").clips.push(ClipConfig {
            id: 1,
            config: SoundConfig {
                volume: 0.25,
                end_ms: Some(400),
                shortcut_id: Some("play-crickets-2".to_owned()),
                midi: Some(binding(None, MidiControl::Note(60))),
                ..SoundConfig::default()
            },
        });
        let clip = Command::Play(PlayRequest {
            path: f.dir.path().join("Crickets.wav"),
            clip: 1,
            trim: Some(Trim {
                start_ms: 0,
                end_ms: Some(400),
            }),
            volume: 0.25,
            destination: Destination::Everywhere,
        });
        let table = f.state.trigger_table();
        assert_eq!(
            table.resolve(&Trigger::Hotkey("play-crickets-2".to_owned())),
            run(vec![clip.clone()])
        );
        assert_eq!(table.resolve(&midi(MidiControl::Note(60))), run(vec![clip]));
    }

    #[test]
    fn the_midi_stop_all_binding_stops_before_playing() {
        let mut f = fixture();
        let horn = f.state.config.tabs[0].sounds.get_mut("Horn.wav");
        horn.expect("configured").midi = Some(binding(None, MidiControl::Note(40)));
        assert_eq!(
            f.state
                .trigger_table()
                .resolve(&midi(MidiControl::Note(40))),
            run(vec![Command::StopAll, f.play("Horn.wav")])
        );
    }

    #[test]
    fn while_learning_midi_triggers_go_to_the_ui() {
        let mut f = fixture();
        f.state.midi.learning = Some(LearnTarget::Sound);
        let table = f.state.trigger_table();
        let note = midi(MidiControl::Note(36));
        let Trigger::Midi(trigger) = note.clone() else {
            unreachable!("a MIDI trigger");
        };
        assert_eq!(table.resolve(&note), Resolution::Learn(trigger));
        assert_eq!(
            table.resolve(&Trigger::Hotkey("play-airhorn".to_owned())),
            run(vec![airhorn(&f)]),
            "hotkeys still play"
        );
    }

    #[test]
    fn while_recording_a_hotkey_shortcuts_do_nothing() {
        let mut f = fixture();
        f.state.recording_stop_all = true;
        let table = f.state.trigger_table();
        let press = Trigger::Hotkey("play-airhorn".to_owned());
        assert_eq!(table.resolve(&press), run(vec![]));
        f.state.recording_stop_all = false;
        assert_eq!(
            f.state.trigger_table().resolve(&press),
            run(vec![airhorn(&f)])
        );
    }

    /// Records what it's told; shared with the test.
    #[derive(Clone, Default)]
    struct FakePlayer(Arc<Mutex<Vec<Command>>>);

    impl FakePlayer {
        fn commands(&self) -> Vec<Command> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Player for FakePlayer {
        fn play(&self, request: PlayRequest) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Command::Play(request));
        }
        fn stop_all(&self) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Command::StopAll);
        }
    }

    fn wait_for(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_thread_plays_triggers_and_hands_learned_ones_to_the_ui() {
        let mut f = fixture();
        let player = FakePlayer::default();
        let (triggers, received) = mpsc::channel();
        let wakes = Arc::new(Mutex::new(0));
        let woken = Arc::clone(&wakes);
        let dispatcher = Dispatcher::start(received, Some(Box::new(player.clone())), move || {
            *woken.lock().unwrap_or_else(PoisonError::into_inner) += 1
        })
        .expect("thread starts");
        dispatcher.set_table(f.state.trigger_table());

        triggers
            .send(Trigger::Hotkey("unknown".to_owned()))
            .expect("running");
        triggers
            .send(Trigger::Hotkey("play-airhorn".to_owned()))
            .expect("running");
        wait_for(|| !player.commands().is_empty());
        assert_eq!(player.commands(), [airhorn(&f)]);
        assert_eq!(*wakes.lock().unwrap_or_else(PoisonError::into_inner), 1);

        f.state.midi.learning = Some(LearnTarget::StopAll);
        dispatcher.set_table(f.state.trigger_table());
        triggers.send(midi(MidiControl::Note(40))).expect("running");
        let mut got = Vec::new();
        wait_for(|| {
            got.extend(dispatcher.learned());
            !got.is_empty()
        });
        assert_eq!(got.len(), 1);
        assert_eq!(player.commands(), [airhorn(&f)], "learning doesn't stop");
    }
}
