//! [`MidiInput`] on the ALSA sequencer through `midir`.

use super::{Connection, MidiError, MidiInput, MidiPort, OnMessage};

/// The ALSA sequencer client name SoundBoars shows up as.
const CLIENT_NAME: &str = "SoundBoars";

/// Lists ports with one sequencer client, opened on first use (and again after it failed); every
/// connection gets a client of its own, as `midir` requires.
#[derive(Default)]
pub struct MidirInput {
    lister: Option<midir::MidiInput>,
}

impl MidiInput for MidirInput {
    fn ports(&mut self) -> Result<Vec<MidiPort>, MidiError> {
        let lister = match &mut self.lister {
            Some(lister) => lister,
            None => self.lister.insert(open()?),
        };
        let ports = lister.ports();
        Ok(ports
            .iter()
            .filter_map(|port| {
                // Fails for a port that vanished since listing; it's gone from the next list.
                let name = lister.port_name(port).ok()?;
                Some(MidiPort {
                    id: port.id(),
                    name: stable_port_name(&name).to_owned(),
                })
            })
            .collect())
    }

    fn connect(
        &mut self,
        port: &MidiPort,
        mut on_message: OnMessage,
    ) -> Result<Connection, MidiError> {
        let error = |reason: String| MidiError::Connect {
            port: port.name.clone(),
            reason,
        };
        let input = open()?;
        let found = input
            .find_port_by_id(&port.id)
            .ok_or_else(|| error("it is gone".to_owned()))?;
        let connection = input
            .connect(
                &found,
                "SoundBoars input",
                move |_timestamp, message, ()| on_message(message),
                (),
            )
            .map_err(|failed| error(failed.to_string()))?;
        Ok(Box::new(connection))
    }
}

fn open() -> Result<midir::MidiInput, MidiError> {
    let mut input =
        midir::MidiInput::new(CLIENT_NAME).map_err(|error| MidiError::Init(error.to_string()))?;
    // SysEx, clock and active sensing never trigger anything.
    input.ignore(midir::Ignore::All);
    Ok(input)
}

/// `midir` names ALSA ports `client:port client-id:port-id`, e.g. `MPD218:MPD218 MIDI 1 20:0`.
/// The ids can change when a device is plugged in again, so bindings and settings use the name
/// without them.
pub fn stable_port_name(name: &str) -> &str {
    let Some((stable, address)) = name.rsplit_once(' ') else {
        return name;
    };
    let is_number = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    match address.split_once(':') {
        Some((client, port)) if is_number(client) && is_number(port) => stable,
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_names_drop_the_sequencer_address() {
        assert_eq!(
            stable_port_name("MPD218:MPD218 MIDI 1 20:0"),
            "MPD218:MPD218 MIDI 1"
        );
        assert_eq!(
            stable_port_name("Midi Through:Midi Through Port-0 14:0"),
            "Midi Through:Midi Through Port-0"
        );
        assert_eq!(
            stable_port_name("MPD218:MPD218 MIDI 1"),
            "MPD218:MPD218 MIDI 1"
        );
        assert_eq!(stable_port_name("Odd:Port 1:x"), "Odd:Port 1:x");
        assert_eq!(stable_port_name("Plain"), "Plain");
    }
}
