//! What a sound (or "Stop all") is bound to.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{MidiControl, MidiTrigger, device_name};

/// A note or CC on a channel, optionally only from one port. In the config file:
/// `midi = { port = "MPD218:MPD218 MIDI 1", channel = 1, control = { note = 36 } }`.
///
/// Unlike the other config structs this one has no `#[serde(default)]`: a binding without its
/// channel or control means nothing, while `port` is optional anyway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiBinding {
    /// The port's stable name; `None` matches any port.
    #[serde(default)]
    pub port: Option<String>,
    /// 1–16.
    pub channel: u8,
    pub control: MidiControl,
}

impl MidiBinding {
    /// Binds exactly what was received, including the port.
    pub fn learned(trigger: &MidiTrigger) -> Self {
        Self {
            port: Some(trigger.port.clone()),
            channel: trigger.channel,
            control: trigger.control,
        }
    }

    pub fn matches(&self, trigger: &MidiTrigger) -> bool {
        self.channel == trigger.channel
            && self.control == trigger.control
            && self.port.as_ref().is_none_or(|port| *port == trigger.port)
    }

    /// Whether some trigger would match both bindings.
    pub fn overlaps(&self, other: &Self) -> bool {
        let ports_overlap = match (&self.port, &other.port) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => true,
        };
        self.channel == other.channel && self.control == other.control && ports_overlap
    }
}

/// `Note 36 · ch 1 · MPD218`, or `CC 20 · ch 1` without a port.
impl fmt::Display for MidiBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} · ch {}", self.control, self.channel)?;
        if let Some(port) = &self.port {
            write!(f, " · {}", device_name(port))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MPD: &str = "MPD218:MPD218 MIDI 1";

    fn trigger(port: &str, channel: u8, control: MidiControl) -> MidiTrigger {
        MidiTrigger {
            port: port.to_owned(),
            channel,
            control,
        }
    }

    fn binding(port: Option<&str>, channel: u8, control: MidiControl) -> MidiBinding {
        MidiBinding {
            port: port.map(str::to_owned),
            channel,
            control,
        }
    }

    #[test]
    fn a_binding_with_a_port_matches_only_that_port() {
        let note = MidiControl::Note(36);
        let bound = binding(Some(MPD), 1, note);
        assert!(bound.matches(&trigger(MPD, 1, note)));
        assert!(!bound.matches(&trigger("BeatStep:BeatStep MIDI 1", 1, note)));
        assert!(!bound.matches(&trigger(MPD, 2, note)));
        assert!(!bound.matches(&trigger(MPD, 1, MidiControl::Note(37))));
        assert!(!bound.matches(&trigger(MPD, 1, MidiControl::ControlChange(36))));
    }

    #[test]
    fn a_binding_without_a_port_matches_any_port() {
        let cc = MidiControl::ControlChange(20);
        let bound = binding(None, 10, cc);
        assert!(bound.matches(&trigger(MPD, 10, cc)));
        assert!(bound.matches(&trigger("BeatStep:BeatStep MIDI 1", 10, cc)));
        assert!(!bound.matches(&trigger(MPD, 1, cc)));
    }

    #[test]
    fn learning_keeps_the_port() {
        let learned = MidiBinding::learned(&trigger(MPD, 3, MidiControl::Note(40)));
        assert_eq!(learned, binding(Some(MPD), 3, MidiControl::Note(40)));
    }

    #[test]
    fn bindings_overlap_unless_ports_differ() {
        let note = MidiControl::Note(36);
        let mpd = binding(Some(MPD), 1, note);
        assert!(mpd.overlaps(&mpd));
        assert!(mpd.overlaps(&binding(None, 1, note)));
        assert!(binding(None, 1, note).overlaps(&mpd));
        assert!(!mpd.overlaps(&binding(Some("Other:Other"), 1, note)));
        assert!(!mpd.overlaps(&binding(Some(MPD), 2, note)));
        assert!(!mpd.overlaps(&binding(Some(MPD), 1, MidiControl::ControlChange(36))));
    }

    #[test]
    fn bindings_read_like_the_controller() {
        assert_eq!(
            binding(Some(MPD), 1, MidiControl::Note(36)).to_string(),
            "Note 36 · ch 1 · MPD218"
        );
        assert_eq!(
            binding(None, 1, MidiControl::ControlChange(20)).to_string(),
            "CC 20 · ch 1"
        );
    }

    #[test]
    fn bindings_are_readable_toml() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrapper {
            midi: MidiBinding,
        }
        let wrapper = Wrapper {
            midi: binding(Some(MPD), 1, MidiControl::ControlChange(20)),
        };
        let text = toml::to_string(&wrapper).expect("serialize");
        assert!(text.contains("channel = 1"), "{text}");
        assert!(text.contains("cc = 20"), "{text}");
        assert_eq!(toml::from_str::<Wrapper>(&text).expect("parse"), wrapper);

        let without_port: Wrapper =
            toml::from_str("midi = { channel = 2, control = { note = 36 } }").expect("parse");
        assert_eq!(without_port.midi, binding(None, 2, MidiControl::Note(36)));
    }
}
