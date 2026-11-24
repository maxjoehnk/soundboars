//! Which ports to connect to and which to drop, given what's connected and what's there now.

use super::{MidiDevice, MidiPort};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub connect: Vec<MidiPort>,
    pub disconnect: Vec<MidiPort>,
}

/// Connects to the `available` ports the `device` setting wants that aren't `connected` yet, and
/// drops the connected ones that vanished or aren't wanted any more. Ports are compared by address
/// and name, so a different device that got a vanished one's address is reconnected.
pub fn plan(connected: &[MidiPort], available: &[MidiPort], device: &MidiDevice) -> Plan {
    let wanted: Vec<&MidiPort> = available.iter().filter(|p| device.wants(p)).collect();
    Plan {
        connect: wanted
            .iter()
            .filter(|port| !connected.contains(port))
            .map(|&port| port.clone())
            .collect(),
        disconnect: connected
            .iter()
            .filter(|port| !wanted.contains(port))
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(id: &str, name: &str) -> MidiPort {
        MidiPort {
            id: id.to_owned(),
            name: name.to_owned(),
        }
    }

    fn mpd() -> MidiPort {
        port("20:0", "MPD218:MPD218 MIDI 1")
    }

    fn beatstep() -> MidiPort {
        port("24:0", "BeatStep:BeatStep MIDI 1")
    }

    #[test]
    fn nothing_is_connected_while_off() {
        assert_eq!(
            plan(&[], &[mpd(), beatstep()], &MidiDevice::Off),
            Plan::default()
        );
        assert_eq!(
            plan(&[mpd()], &[mpd()], &MidiDevice::Off),
            Plan {
                connect: vec![],
                disconnect: vec![mpd()],
            }
        );
    }

    #[test]
    fn all_devices_connects_every_new_port() {
        assert_eq!(
            plan(&[mpd()], &[mpd(), beatstep()], &MidiDevice::All),
            Plan {
                connect: vec![beatstep()],
                disconnect: vec![],
            }
        );
    }

    #[test]
    fn a_selected_device_connects_only_that_one_when_it_appears() {
        let device = MidiDevice::Port(mpd().name);
        assert_eq!(plan(&[], &[beatstep()], &device), Plan::default());
        assert_eq!(
            plan(&[], &[beatstep(), mpd()], &device),
            Plan {
                connect: vec![mpd()],
                disconnect: vec![],
            }
        );
        assert_eq!(
            plan(&[mpd()], &[beatstep(), mpd()], &device),
            Plan::default()
        );
    }

    #[test]
    fn vanished_ports_are_dropped() {
        assert_eq!(
            plan(&[mpd(), beatstep()], &[beatstep()], &MidiDevice::All),
            Plan {
                connect: vec![],
                disconnect: vec![mpd()],
            }
        );
    }

    #[test]
    fn a_port_that_reappears_at_another_address_is_reconnected() {
        let replugged = port("28:0", &mpd().name);
        assert_eq!(
            plan(
                &[mpd()],
                std::slice::from_ref(&replugged),
                &MidiDevice::Port(mpd().name)
            ),
            Plan {
                connect: vec![replugged],
                disconnect: vec![mpd()],
            }
        );
    }

    #[test]
    fn changing_the_device_swaps_connections() {
        assert_eq!(
            plan(
                &[mpd()],
                &[mpd(), beatstep()],
                &MidiDevice::Port(beatstep().name)
            ),
            Plan {
                connect: vec![beatstep()],
                disconnect: vec![mpd()],
            }
        );
    }
}
