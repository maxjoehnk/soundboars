//! Decides which of our output ports get linked to which input ports of a target node.

use super::graph::Port;

/// Channel name PipeWire uses for single-channel ports.
const MONO: &str = "MONO";

/// One link from one of our output ports to one of the target's input ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkPair {
    pub output_port: u32,
    pub input_port: u32,
}

/// Maps `outputs` onto `inputs` by channel: FL→FL, FR→FR, … A mono target (a `MONO` port or a
/// single input port) receives every output channel; PipeWire sums them. Input ports whose channel
/// has no matching output stay unlinked.
pub fn plan_links(outputs: &[Port], inputs: &[Port]) -> Vec<LinkPair> {
    let is_single_port = inputs.len() == 1;
    inputs
        .iter()
        .flat_map(|input| {
            outputs
                .iter()
                .filter(move |output| {
                    is_single_port || is_mono(input) || output.channel == input.channel
                })
                .map(move |output| LinkPair {
                    output_port: output.id,
                    input_port: input.id,
                })
        })
        .collect()
}

/// Whether a target with these input ports gets several of our channels summed into one port (see
/// [`plan_links`]), which doubles the level of a centred sound.
pub fn sums_channels(inputs: &[Port]) -> bool {
    inputs.len() == 1 || inputs.iter().any(is_mono)
}

fn is_mono(port: &Port) -> bool {
    port.channel == MONO
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(id: u32, channel: &str) -> Port {
        Port {
            id,
            channel: channel.to_owned(),
        }
    }

    fn pair(output_port: u32, input_port: u32) -> LinkPair {
        LinkPair {
            output_port,
            input_port,
        }
    }

    fn stereo_outputs() -> Vec<Port> {
        vec![port(1, "FL"), port(2, "FR")]
    }

    #[test]
    fn stereo_target_is_linked_channel_by_channel() {
        let inputs = [port(10, "FL"), port(11, "FR")];
        assert_eq!(
            plan_links(&stereo_outputs(), &inputs),
            [pair(1, 10), pair(2, 11)]
        );
    }

    #[test]
    fn input_port_order_does_not_matter() {
        let inputs = [port(11, "FR"), port(10, "FL")];
        assert_eq!(
            plan_links(&stereo_outputs(), &inputs),
            [pair(2, 11), pair(1, 10)]
        );
    }

    #[test]
    fn mono_target_gets_both_channels() {
        let inputs = [port(10, "MONO")];
        assert_eq!(
            plan_links(&stereo_outputs(), &inputs),
            [pair(1, 10), pair(2, 10)]
        );
    }

    #[test]
    fn single_port_target_gets_both_channels_whatever_its_channel_name() {
        let inputs = [port(10, "AUX0")];
        assert_eq!(
            plan_links(&stereo_outputs(), &inputs),
            [pair(1, 10), pair(2, 10)]
        );
    }

    #[test]
    fn unmatched_channels_stay_unlinked() {
        let inputs = [port(10, "FL"), port(11, "FR"), port(12, "LFE")];
        assert_eq!(
            plan_links(&stereo_outputs(), &inputs),
            [pair(1, 10), pair(2, 11)]
        );
    }

    #[test]
    fn target_without_ports_gets_no_links() {
        assert!(plan_links(&stereo_outputs(), &[]).is_empty());
    }

    #[test]
    fn no_outputs_means_no_links() {
        assert!(plan_links(&[], &[port(10, "FL"), port(11, "FR")]).is_empty());
    }

    #[test]
    fn target_seen_with_one_port_so_far_is_treated_as_mono() {
        // Ports show up one by one in the registry. Until the second port of a stereo target
        // arrives it looks mono; the link reconciler replaces the links once it does.
        let inputs = [port(10, "FL")];
        assert_eq!(
            plan_links(&stereo_outputs(), &inputs),
            [pair(1, 10), pair(2, 10)]
        );
    }

    #[test]
    fn mono_and_single_port_targets_sum_channels() {
        assert!(sums_channels(&[port(10, "MONO")]));
        assert!(sums_channels(&[port(10, "AUX0")]));
        assert!(sums_channels(&[port(10, "MONO"), port(11, "FL")]));
        assert!(!sums_channels(&[port(10, "FL"), port(11, "FR")]));
        assert!(!sums_channels(&[]));
    }
}
