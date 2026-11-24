//! Turning raw MIDI messages into [`MidiTrigger`]s.

use std::collections::HashSet;

use super::{MidiControl, MidiTrigger};

const NOTE_ON: u8 = 0x9;
const CONTROL_CHANGE: u8 = 0xB;
/// CC values from here up count as "on" (pressed button, knob past the middle).
const CC_ON: u8 = 64;

/// Parses the messages of one port. A Note On with a velocity above 0 triggers; Note Offs (and
/// Note Ons with velocity 0, which mean the same) don't. A CC triggers when its value rises from
/// below [`CC_ON`] to [`CC_ON`] or above, so a button sending 127/0 triggers once per press and a
/// knob once per turn past the middle. Everything else (clock, SysEx, pitch bend, …) is ignored.
///
/// Running status (messages without their status byte) is ignored, too: midir's ALSA backend
/// always delivers complete messages.
#[derive(Debug)]
pub struct Parser {
    port: String,
    /// The (channel, CC) pairs whose last value was "on".
    controls_on: HashSet<(u8, u8)>,
}

impl Parser {
    pub fn new(port: String) -> Self {
        Self {
            port,
            controls_on: HashSet::new(),
        }
    }

    pub fn parse(&mut self, message: &[u8]) -> Option<MidiTrigger> {
        let (&status, data) = message.split_first()?;
        let is_channel_message = (0x80..0xF0).contains(&status);
        let [first, second] = *data.first_chunk::<2>()?;
        if !is_channel_message || first >= 0x80 || second >= 0x80 {
            return None;
        }
        let channel = status & 0x0F;
        let control = match status >> 4 {
            NOTE_ON if second > 0 => MidiControl::Note(first),
            CONTROL_CHANGE if self.rises(channel, first, second) => {
                MidiControl::ControlChange(first)
            }
            _ => return None,
        };
        Some(MidiTrigger {
            port: self.port.clone(),
            channel: channel + 1,
            control,
        })
    }

    /// Remembers the CC's new value; `true` if it crossed [`CC_ON`] upwards.
    fn rises(&mut self, channel: u8, cc: u8, value: u8) -> bool {
        if value >= CC_ON {
            self.controls_on.insert((channel, cc))
        } else {
            self.controls_on.remove(&(channel, cc));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORT: &str = "MPD218:MPD218 MIDI 1";

    fn parse_all(messages: &[&[u8]]) -> Vec<(u8, MidiControl)> {
        let mut parser = Parser::new(PORT.to_owned());
        messages
            .iter()
            .filter_map(|message| parser.parse(message))
            .inspect(|trigger| assert_eq!(trigger.port, PORT))
            .map(|trigger| (trigger.channel, trigger.control))
            .collect()
    }

    #[test]
    fn note_ons_trigger_with_their_channel() {
        assert_eq!(
            parse_all(&[&[0x90, 36, 100], &[0x99, 42, 1]]),
            [(1, MidiControl::Note(36)), (10, MidiControl::Note(42))]
        );
    }

    #[test]
    fn note_offs_and_silent_note_ons_do_not_trigger() {
        assert!(parse_all(&[&[0x80, 36, 64], &[0x90, 36, 0], &[0x8F, 1, 0]]).is_empty());
    }

    #[test]
    fn control_changes_trigger_when_rising_past_the_middle() {
        let cc = |value| [0xB0, 20, value];
        assert_eq!(
            parse_all(&[
                &cc(0),
                &cc(63),
                &cc(64),
                &cc(127),
                &cc(100),
                &cc(10),
                &cc(127)
            ]),
            [
                (1, MidiControl::ControlChange(20)),
                (1, MidiControl::ControlChange(20))
            ]
        );
    }

    #[test]
    fn a_control_that_starts_high_triggers_once() {
        assert_eq!(
            parse_all(&[&[0xB0, 20, 127], &[0xB0, 20, 127]]),
            [(1, MidiControl::ControlChange(20))]
        );
    }

    #[test]
    fn control_edges_are_tracked_per_channel_and_control() {
        assert_eq!(
            parse_all(&[
                &[0xB0, 20, 127],
                &[0xB1, 20, 127],
                &[0xB0, 21, 127],
                &[0xB0, 20, 127],
            ]),
            [
                (1, MidiControl::ControlChange(20)),
                (2, MidiControl::ControlChange(20)),
                (1, MidiControl::ControlChange(21)),
            ]
        );
    }

    #[test]
    fn running_status_and_junk_are_ignored() {
        assert!(
            parse_all(&[
                &[],
                &[36, 100],          // running status: data bytes only
                &[0x90, 36],         // truncated
                &[0x90, 200, 100],   // a status byte where data belongs
                &[0xF8],             // clock
                &[0xFA],             // start
                &[0xF0, 1, 2, 0xF7], // SysEx
                &[0xE0, 0, 64],      // pitch bend
                &[0xC0, 5],          // program change
                &[0xA0, 36, 100],    // aftertouch
            ])
            .is_empty()
        );
    }

    #[test]
    fn extra_bytes_after_a_message_are_tolerated() {
        assert_eq!(
            parse_all(&[&[0x90, 36, 100, 0]]),
            [(1, MidiControl::Note(36))]
        );
    }
}
