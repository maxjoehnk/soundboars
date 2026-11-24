//! The `midi` thread: follows the "MIDI input" setting and the ports coming and going, and keeps
//! the wanted ports connected. Triggers are sent straight from the MIDI threads that receive them.

use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use super::message::Parser;
use super::{Connection, MidiDevice, MidiError, MidiInput, MidiPort, MidiTrigger, hotplug};

/// How often to look for ports that appeared or vanished. ALSA can announce this, but `midir`
/// doesn't expose it; listing the ports is cheap.
pub const HOTPLUG_POLL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiEvent {
    /// The names of the input ports there are, whenever that changes.
    Ports(Vec<String>),
    /// The names of the ports listened to, whenever that changes.
    Connected(Vec<String>),
    Trigger(MidiTrigger),
    /// Reported once; again only after the same thing worked in between.
    Error(String),
}

/// Wakes the UI so it handles new events right away, even while idle or unfocused.
type Wake = Arc<dyn Fn() + Send + Sync>;

/// The app's handle on the `midi` thread. Every call returns right away; dropping it stops the
/// thread, which closes the connections.
pub struct MidiService {
    devices: mpsc::Sender<MidiDevice>,
    events: mpsc::Receiver<MidiEvent>,
}

impl MidiService {
    /// Starts with MIDI [off](MidiDevice::Off) until [`Self::select`] is called. `wake` is called
    /// after every event.
    pub fn start(
        input: Box<dyn MidiInput>,
        poll_interval: Duration,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let (devices, device_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let scanner = Scanner {
            input,
            events: event_tx,
            wake: Arc::new(wake),
            connections: Vec::new(),
            ports: None,
            connected: Vec::new(),
            reported: HashSet::new(),
        };
        thread::Builder::new()
            .name("midi".to_owned())
            .spawn(move || run(scanner, &device_rx, poll_interval))?;
        Ok(Self { devices, events })
    }

    /// Listens to these ports from now on.
    pub fn select(&self, device: MidiDevice) {
        // Fails only if the thread is gone, which it never leaves on its own.
        let _ = self.devices.send(device);
    }

    /// The events since the last call.
    pub fn poll(&self) -> Vec<MidiEvent> {
        self.events.try_iter().collect()
    }
}

fn run(mut scanner: Scanner, devices: &mpsc::Receiver<MidiDevice>, poll_interval: Duration) {
    let mut device = MidiDevice::Off;
    loop {
        match devices.recv_timeout(poll_interval) {
            Ok(selected) => device = selected,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        scanner.scan(&device);
    }
}

struct Scanner {
    input: Box<dyn MidiInput>,
    events: mpsc::Sender<MidiEvent>,
    wake: Wake,
    connections: Vec<(MidiPort, Connection)>,
    /// What was reported last.
    ports: Option<Vec<String>>,
    connected: Vec<String>,
    /// Errors that were reported and haven't been resolved since.
    reported: HashSet<MidiError>,
}

impl Scanner {
    fn scan(&mut self, device: &MidiDevice) {
        match self.input.ports() {
            Ok(available) => {
                self.reported
                    .retain(|error| !matches!(error, MidiError::Init(_)));
                self.report_ports(&available);
                self.follow(&available, device);
            }
            Err(error) => {
                self.connections.clear();
                // Without MIDI in use, a missing sequencer is no news.
                if *device != MidiDevice::Off {
                    self.report_error(error);
                }
            }
        }
        self.report_connected();
    }

    /// Connects and disconnects as the device setting and the `available` ports say.
    fn follow(&mut self, available: &[MidiPort], device: &MidiDevice) {
        let connected: Vec<MidiPort> = self.connections.iter().map(|(p, _)| p.clone()).collect();
        let plan = hotplug::plan(&connected, available, device);
        self.connections
            .retain(|(port, _)| !plan.disconnect.contains(port));
        for port in plan.connect {
            match self.input.connect(&port, self.on_message(&port)) {
                Ok(connection) => {
                    tracing::info!("listening to MIDI input {}", port.name);
                    self.reported.retain(|error| !is_about(error, &port));
                    self.connections.push((port, connection));
                }
                Err(error) => self.report_error(error),
            }
        }
    }

    /// Parses the port's messages and sends the triggers.
    fn on_message(&self, port: &MidiPort) -> super::OnMessage {
        let mut parser = Parser::new(port.name.clone());
        let events = self.events.clone();
        let wake = Arc::clone(&self.wake);
        Box::new(move |message| {
            if let Some(trigger) = parser.parse(message)
                && events.send(MidiEvent::Trigger(trigger)).is_ok()
            {
                wake();
            }
        })
    }

    fn report_ports(&mut self, available: &[MidiPort]) {
        let mut names: Vec<String> = Vec::new();
        for port in available {
            if !names.contains(&port.name) {
                names.push(port.name.clone());
            }
        }
        if self.ports.as_ref() != Some(&names) {
            self.ports = Some(names.clone());
            self.send(MidiEvent::Ports(names));
        }
    }

    fn report_connected(&mut self) {
        let names: Vec<String> = self
            .connections
            .iter()
            .map(|(port, _)| port.name.clone())
            .collect();
        if names != self.connected {
            self.connected = names.clone();
            self.send(MidiEvent::Connected(names));
        }
    }

    fn report_error(&mut self, error: MidiError) {
        if self.reported.insert(error.clone()) {
            self.send(MidiEvent::Error(error.to_string()));
        }
    }

    fn send(&self, event: MidiEvent) {
        // Fails only if the app (and with it the receiver) is gone.
        if self.events.send(event).is_ok() {
            (self.wake)();
        }
    }
}

fn is_about(error: &MidiError, port: &MidiPort) -> bool {
    matches!(error, MidiError::Connect { port: name, .. } if *name == port.name)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, PoisonError};
    use std::time::Instant;

    use super::*;
    use crate::midi::{MidiControl, OnMessage};

    /// Ports, connections and failures as the test sets them up.
    #[derive(Default)]
    struct World {
        ports: Vec<MidiPort>,
        /// The open connections' callbacks by port id; a dropped connection removes its entry.
        listeners: Vec<(String, Arc<Mutex<OnMessage>>)>,
        no_sequencer: bool,
        refuse: bool,
    }

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<World>>);

    impl Fake {
        fn with<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
            f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
        }

        fn send(&self, port_id: &str, message: &[u8]) {
            let listener = self.with(|world| {
                let found = world.listeners.iter().find(|(id, _)| id == port_id);
                found.map(|(_, listener)| Arc::clone(listener))
            });
            let listener = listener.expect("connected");
            (listener.lock().unwrap_or_else(PoisonError::into_inner))(message);
        }

        fn listening(&self) -> Vec<String> {
            self.with(|world| world.listeners.iter().map(|(id, _)| id.clone()).collect())
        }
    }

    struct FakeConnection {
        fake: Fake,
        listener: Arc<Mutex<OnMessage>>,
    }

    impl Drop for FakeConnection {
        fn drop(&mut self) {
            self.fake.with(|world| {
                world
                    .listeners
                    .retain(|(_, listener)| !Arc::ptr_eq(listener, &self.listener))
            });
        }
    }

    impl MidiInput for Fake {
        fn ports(&mut self) -> Result<Vec<MidiPort>, MidiError> {
            self.with(|world| {
                if world.no_sequencer {
                    Err(MidiError::Init("no such file".to_owned()))
                } else {
                    Ok(world.ports.clone())
                }
            })
        }

        fn connect(
            &mut self,
            port: &MidiPort,
            on_message: OnMessage,
        ) -> Result<Connection, MidiError> {
            if self.with(|world| world.refuse) {
                return Err(MidiError::Connect {
                    port: port.name.clone(),
                    reason: "busy".to_owned(),
                });
            }
            let listener = Arc::new(Mutex::new(on_message));
            self.with(|world| {
                world
                    .listeners
                    .push((port.id.clone(), Arc::clone(&listener)))
            });
            Ok(Box::new(FakeConnection {
                fake: self.clone(),
                listener,
            }))
        }
    }

    const POLL: Duration = Duration::from_millis(20);

    fn port(id: &str, name: &str) -> MidiPort {
        MidiPort {
            id: id.to_owned(),
            name: name.to_owned(),
        }
    }

    fn start(fake: &Fake) -> MidiService {
        MidiService::start(Box::new(fake.clone()), POLL, || {}).expect("thread starts")
    }

    /// Collects events until one satisfies `done`; returns all of them.
    fn events_until(service: &MidiService, done: impl Fn(&MidiEvent) -> bool) -> Vec<MidiEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        loop {
            let events = service.poll();
            let finished = events.iter().any(&done);
            seen.extend(events);
            if finished {
                return seen;
            }
            assert!(Instant::now() < deadline, "only got {seen:?}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn connected(names: &[&str]) -> MidiEvent {
        MidiEvent::Connected(names.iter().map(|name| (*name).to_owned()).collect())
    }

    #[test]
    fn listens_to_the_selected_port_and_sends_its_triggers() {
        let fake = Fake::default();
        fake.with(|world| world.ports = vec![port("20:0", "MPD"), port("24:0", "BeatStep")]);
        let service = start(&fake);
        service.select(MidiDevice::Port("MPD".to_owned()));

        let events = events_until(&service, |event| *event == connected(&["MPD"]));
        assert!(
            events.contains(&MidiEvent::Ports(vec![
                "MPD".to_owned(),
                "BeatStep".to_owned()
            ])),
            "{events:?}"
        );
        assert_eq!(fake.listening(), ["20:0"]);

        fake.send("20:0", &[0x80, 36, 0]);
        fake.send("20:0", &[0x90, 36, 100]);
        let events = events_until(&service, |event| matches!(event, MidiEvent::Trigger(_)));
        assert_eq!(
            events,
            [MidiEvent::Trigger(MidiTrigger {
                port: "MPD".to_owned(),
                channel: 1,
                control: MidiControl::Note(36),
            })]
        );
    }

    #[test]
    fn follows_ports_coming_and_going() {
        let fake = Fake::default();
        let service = start(&fake);
        service.select(MidiDevice::All);
        events_until(&service, |event| *event == MidiEvent::Ports(vec![]));

        fake.with(|world| world.ports = vec![port("20:0", "MPD")]);
        events_until(&service, |event| *event == connected(&["MPD"]));
        assert_eq!(fake.listening(), ["20:0"]);

        fake.with(|world| world.ports.clear());
        events_until(&service, |event| *event == connected(&[]));
        assert!(fake.listening().is_empty());

        service.select(MidiDevice::Off);
        fake.with(|world| world.ports = vec![port("21:0", "MPD")]);
        events_until(&service, |event| {
            *event == MidiEvent::Ports(vec!["MPD".to_owned()])
        });
        assert!(fake.listening().is_empty());
    }

    #[test]
    fn connect_errors_are_reported_once() {
        let fake = Fake::default();
        fake.with(|world| {
            world.ports = vec![port("20:0", "MPD")];
            world.refuse = true;
        });
        let service = start(&fake);
        service.select(MidiDevice::All);
        events_until(&service, |event| matches!(event, MidiEvent::Error(_)));
        thread::sleep(POLL * 10);
        let later = service.poll();
        assert!(
            !later
                .iter()
                .any(|event| matches!(event, MidiEvent::Error(_))),
            "{later:?}"
        );

        // After it worked, the same failure is news again.
        fake.with(|world| world.refuse = false);
        events_until(&service, |event| *event == connected(&["MPD"]));
        fake.with(|world| {
            world.refuse = true;
            world.ports.clear();
        });
        events_until(&service, |event| *event == connected(&[]));
        fake.with(|world| world.ports = vec![port("20:0", "MPD")]);
        let events = events_until(&service, |event| matches!(event, MidiEvent::Error(_)));
        assert!(
            events.contains(&MidiEvent::Error(
                "could not connect to MIDI input MPD: busy".to_owned()
            )),
            "{events:?}"
        );
    }

    #[test]
    fn a_missing_sequencer_is_only_reported_when_midi_is_used() {
        let fake = Fake::default();
        fake.with(|world| world.no_sequencer = true);
        let service = start(&fake);
        service.select(MidiDevice::Off);
        thread::sleep(POLL * 5);
        assert!(service.poll().is_empty());

        service.select(MidiDevice::All);
        let events = events_until(&service, |event| matches!(event, MidiEvent::Error(_)));
        assert_eq!(
            events,
            [MidiEvent::Error(
                "could not open the ALSA sequencer (/dev/snd/seq): no such file".to_owned()
            )]
        );
    }
}
