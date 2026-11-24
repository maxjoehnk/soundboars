//! MIDI triggers: listening to MIDI input ports (`midir`, ALSA sequencer), turning their messages
//! into [`MidiTrigger`]s, matching them against [`MidiBinding`]s, and following devices as they
//! are plugged in and out. [`MidiService`] is the API the app uses.

mod binding;
pub mod hotplug;
pub mod message;
mod midir_input;
mod service;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use binding::MidiBinding;
pub use midir_input::{MidirInput, stable_port_name};
pub use service::{HOTPLUG_POLL, MidiEvent, MidiService};

/// What on a controller was hit: a key/pad or a knob/button sending control changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MidiControl {
    Note(u8),
    #[serde(rename = "cc")]
    ControlChange(u8),
}

impl fmt::Display for MidiControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Note(note) => write!(f, "Note {note}"),
            Self::ControlChange(cc) => write!(f, "CC {cc}"),
        }
    }
}

/// A note or control change that should trigger something, as it arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiTrigger {
    /// The input port's [stable name](stable_port_name).
    pub port: String,
    /// 1–16, as devices and manuals count them.
    pub channel: u8,
    pub control: MidiControl,
}

/// A MIDI input port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MidiPort {
    /// Identifies the port while it exists (the ALSA sequencer address, e.g. `20:0`).
    pub id: String,
    /// The [stable name](stable_port_name), e.g. `MPD218:MPD218 MIDI 1`.
    pub name: String,
}

/// Which input ports to listen to (the "MIDI input" setting).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MidiDevice {
    /// MIDI is off.
    #[default]
    Off,
    /// Every input port there is.
    All,
    /// The ports with this [stable name](stable_port_name).
    Port(String),
}

impl MidiDevice {
    pub fn wants(&self, port: &MidiPort) -> bool {
        match self {
            Self::Off => false,
            Self::All => true,
            Self::Port(name) => *name == port.name,
        }
    }
}

/// Called with the raw bytes of every message a connected port receives (on a MIDI thread).
pub type OnMessage = Box<dyn FnMut(&[u8]) + Send>;
/// An open connection to an input port; dropping it disconnects.
pub type Connection = Box<dyn Send>;

/// The system's MIDI inputs (the ALSA sequencer through `midir`, or a fake in tests).
pub trait MidiInput: Send {
    /// The input ports there are right now.
    fn ports(&mut self) -> Result<Vec<MidiPort>, MidiError>;
    fn connect(&mut self, port: &MidiPort, on_message: OnMessage) -> Result<Connection, MidiError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
pub enum MidiError {
    #[error("could not open the ALSA sequencer (/dev/snd/seq): {0}")]
    Init(String),
    #[error("could not connect to MIDI input {port}: {reason}")]
    Connect { port: String, reason: String },
}

/// The short name to show for a port: its device (the ALSA client name before the `:`).
pub fn device_name(port: &str) -> &str {
    port.split_once(':').map_or(port, |(device, _)| device)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str) -> MidiPort {
        MidiPort {
            id: "20:0".to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn the_device_setting_picks_ports() {
        let mpd = port("MPD218:MPD218 MIDI 1");
        assert!(!MidiDevice::Off.wants(&mpd));
        assert!(MidiDevice::All.wants(&mpd));
        assert!(MidiDevice::Port("MPD218:MPD218 MIDI 1".to_owned()).wants(&mpd));
        assert!(!MidiDevice::Port("BeatStep:BeatStep MIDI 1".to_owned()).wants(&mpd));
    }

    #[test]
    fn device_names_are_the_client_part() {
        assert_eq!(device_name("MPD218:MPD218 MIDI 1"), "MPD218");
        assert_eq!(device_name("Odd port"), "Odd port");
    }
}
