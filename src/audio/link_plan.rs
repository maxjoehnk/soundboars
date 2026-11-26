//! Decides which ports get linked: our output ports to a target app's input ports, and the inject
//! stream and the user's microphone to the virtual microphone.

use super::graph::{Direction, Graph, Port, Source};
use super::virtual_mic::VIRTUAL_MIC_NODE_NAME;

/// Channel name PipeWire uses for single-channel ports.
const MONO: &str = "MONO";
/// The channels of the virtual microphone, in order.
const STEREO: [&str; 2] = ["FL", "FR"];

/// One link from one of our output ports to one of the target's input ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkPair {
    pub output_port: u32,
    pub input_port: u32,
}

/// A link between the ports of two nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeLink {
    pub output_node: u32,
    pub input_node: u32,
    pub ports: LinkPair,
}

impl NodeLink {
    /// Links from `output_node` to `input_node` through `pairs` of their ports.
    pub fn all(
        output_node: u32,
        input_node: u32,
        pairs: Vec<LinkPair>,
    ) -> impl Iterator<Item = Self> {
        pairs.into_iter().map(move |ports| Self {
            output_node,
            input_node,
            ports,
        })
    }
}

/// The links into the virtual microphone `virtual_mic`: the inject stream's channels go to the
/// matching channels, and the microphone (see [`choose_microphone`]) is added while someone
/// records the virtual microphone. Without a recorder the real microphone stays closed, so
/// desktops don't show it as in use.
pub fn plan_virtual_mic(
    graph: &Graph,
    inject: Option<u32>,
    virtual_mic: u32,
    default_source: Option<&str>,
) -> Vec<NodeLink> {
    let inputs = graph.ports(virtual_mic, Direction::In);
    let sounds = inject.into_iter().flat_map(|inject| {
        let pairs = plan_links(&graph.ports(inject, Direction::Out), &inputs);
        NodeLink::all(inject, virtual_mic, pairs)
    });
    let mut links: Vec<NodeLink> = sounds.collect();
    let sources = graph.sources();
    let microphone = choose_microphone(&sources, default_source);
    if let (true, Some(mic)) = (graph.has_consumers(virtual_mic), microphone) {
        let pairs = plan_mic_links(&graph.ports(mic.node_id, Direction::Out), &inputs);
        links.extend(NodeLink::all(mic.node_id, virtual_mic, pairs));
    }
    links
}

/// The microphone to mix into the virtual microphone: the default source named by the `default`
/// metadata, else the source with the highest `priority.session` (the lowest node id among
/// equals). Never a virtual microphone of SoundBoars, which would feed itself.
pub fn choose_microphone<'a>(sources: &'a [Source], default: Option<&str>) -> Option<&'a Source> {
    let candidates = || {
        sources
            .iter()
            .filter(|source| source.name != VIRTUAL_MIC_NODE_NAME)
    };
    let default = default.and_then(|name| candidates().find(|source| source.name == name));
    default.or_else(|| {
        candidates().reduce(|best, source| {
            if source.priority > best.priority {
                source
            } else {
                best
            }
        })
    })
}

/// Maps a microphone's output ports onto the virtual microphone's FL/FR `inputs` by channel. A
/// mono microphone (a `MONO` port or a single port) goes to both. If no channel matches (e.g.
/// `AUX0`, `AUX1`), the first two ports go to FL and FR, in order.
pub fn plan_mic_links(outputs: &[Port], inputs: &[Port]) -> Vec<LinkPair> {
    let is_single_port = outputs.len() == 1;
    let by_channel: Vec<LinkPair> = outputs
        .iter()
        .flat_map(|output| {
            inputs
                .iter()
                .filter(move |input| {
                    is_single_port || is_mono(output) || output.channel == input.channel
                })
                .map(|input| LinkPair {
                    output_port: output.id,
                    input_port: input.id,
                })
        })
        .collect();
    if !by_channel.is_empty() {
        return by_channel;
    }
    STEREO
        .iter()
        .zip(outputs)
        .filter_map(|(channel, output)| {
            let input = inputs.iter().find(|input| input.channel == *channel)?;
            Some(LinkPair {
                output_port: output.id,
                input_port: input.id,
            })
        })
        .collect()
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

    fn source(node_id: u32, name: &str, priority: i64) -> Source {
        Source {
            node_id,
            name: name.to_owned(),
            description: name.to_owned(),
            priority,
        }
    }

    fn virtual_mic_inputs() -> Vec<Port> {
        vec![port(20, "FL"), port(21, "FR")]
    }

    #[test]
    fn mono_mic_goes_to_both_channels() {
        assert_eq!(
            plan_mic_links(&[port(5, "MONO")], &virtual_mic_inputs()),
            [pair(5, 20), pair(5, 21)]
        );
    }

    #[test]
    fn single_port_mic_goes_to_both_channels() {
        assert_eq!(
            plan_mic_links(&[port(5, "AUX0")], &virtual_mic_inputs()),
            [pair(5, 20), pair(5, 21)]
        );
    }

    #[test]
    fn stereo_mic_is_linked_channel_by_channel() {
        assert_eq!(
            plan_mic_links(&[port(6, "FR"), port(5, "FL")], &virtual_mic_inputs()),
            [pair(6, 21), pair(5, 20)]
        );
    }

    #[test]
    fn mic_without_matching_channels_maps_its_first_two_ports_in_order() {
        let outputs = [port(5, "AUX0"), port(6, "AUX1"), port(7, "AUX2")];
        assert_eq!(
            plan_mic_links(&outputs, &[port(21, "FR"), port(20, "FL")]),
            [pair(5, 20), pair(6, 21)]
        );
    }

    #[test]
    fn mic_without_ports_gets_no_links() {
        assert!(plan_mic_links(&[], &virtual_mic_inputs()).is_empty());
        assert!(plan_mic_links(&[port(5, "MONO")], &[]).is_empty());
    }

    #[test]
    fn the_default_source_is_the_microphone() {
        let sources = [source(1, "usb", 2000), source(2, "headset", 1000)];
        assert_eq!(
            choose_microphone(&sources, Some("headset")),
            Some(&sources[1])
        );
    }

    #[test]
    fn without_a_known_default_the_highest_priority_source_is_the_microphone() {
        let sources = [
            source(1, "a", 1000),
            source(2, "b", 2000),
            source(3, "c", 2000),
        ];
        assert_eq!(choose_microphone(&sources, None), Some(&sources[1]));
        assert_eq!(
            choose_microphone(&sources, Some("headless-sink")),
            Some(&sources[1])
        );
    }

    #[test]
    fn our_virtual_mic_is_never_the_microphone() {
        let sources = [source(1, VIRTUAL_MIC_NODE_NAME, 3000), source(2, "usb", 0)];
        assert_eq!(
            choose_microphone(&sources, Some(VIRTUAL_MIC_NODE_NAME)),
            Some(&sources[1])
        );
        assert_eq!(choose_microphone(&sources[..1], None), None);
        assert_eq!(choose_microphone(&[], Some("usb")), None);
    }

    mod virtual_mic {
        use super::*;
        use crate::audio::graph::{LinkProps, NodeProps, PortProps};

        const INJECT: u32 = 10;
        const MIC: u32 = 30;
        const OTHER_MIC: u32 = 40;
        const VIRTUAL_MIC: u32 = 50;
        const RECORDER: u32 = 60;

        fn add_port(graph: &mut Graph, node_id: u32, id: u32, direction: Direction, channel: &str) {
            graph.add_port(PortProps {
                node_id,
                direction,
                port: port(id, channel),
            });
        }

        fn add_source(graph: &mut Graph, node_id: u32, name: &str) {
            let props = [("media.class", "Audio/Source"), ("node.name", name)];
            let props =
                NodeProps::parse(|key| props.iter().find(|(k, _)| *k == key).map(|(_, v)| *v));
            graph.set_node(node_id, props);
        }

        /// Inject stream (FL 11, FR 12), mono mic (31), stereo mic (41, 42), virtual mic (in 51,
        /// 52; out 53, 54).
        fn graph() -> Graph {
            let mut graph = Graph::default();
            add_port(&mut graph, INJECT, 11, Direction::Out, "FL");
            add_port(&mut graph, INJECT, 12, Direction::Out, "FR");
            add_source(&mut graph, MIC, "mic");
            add_port(&mut graph, MIC, 31, Direction::Out, "MONO");
            add_source(&mut graph, OTHER_MIC, "other");
            add_port(&mut graph, OTHER_MIC, 41, Direction::Out, "FL");
            add_port(&mut graph, OTHER_MIC, 42, Direction::Out, "FR");
            add_source(&mut graph, VIRTUAL_MIC, VIRTUAL_MIC_NODE_NAME);
            add_port(&mut graph, VIRTUAL_MIC, 51, Direction::In, "FL");
            add_port(&mut graph, VIRTUAL_MIC, 52, Direction::In, "FR");
            add_port(&mut graph, VIRTUAL_MIC, 53, Direction::Out, "FL");
            add_port(&mut graph, VIRTUAL_MIC, 54, Direction::Out, "FR");
            graph
        }

        fn record(graph: &mut Graph) {
            graph.add_link(
                99,
                LinkProps {
                    output_node: VIRTUAL_MIC,
                    input_node: RECORDER,
                },
            );
        }

        fn link(output_node: u32, output_port: u32, input_port: u32) -> NodeLink {
            NodeLink {
                output_node,
                input_node: VIRTUAL_MIC,
                ports: pair(output_port, input_port),
            }
        }

        #[test]
        fn without_recorder_only_the_sounds_go_in() {
            let plan = plan_virtual_mic(&graph(), Some(INJECT), VIRTUAL_MIC, Some("mic"));
            assert_eq!(plan, [link(INJECT, 11, 51), link(INJECT, 12, 52)]);
        }

        #[test]
        fn while_recorded_the_default_mic_goes_in_too() {
            let mut graph = graph();
            record(&mut graph);
            let plan = plan_virtual_mic(&graph, Some(INJECT), VIRTUAL_MIC, Some("mic"));
            assert_eq!(
                plan,
                [
                    link(INJECT, 11, 51),
                    link(INJECT, 12, 52),
                    link(MIC, 31, 51),
                    link(MIC, 31, 52)
                ]
            );
        }

        #[test]
        fn a_new_default_replaces_the_mic() {
            let mut graph = graph();
            record(&mut graph);
            let plan = plan_virtual_mic(&graph, None, VIRTUAL_MIC, Some("other"));
            assert_eq!(plan, [link(OTHER_MIC, 41, 51), link(OTHER_MIC, 42, 52)]);
        }

        #[test]
        fn our_own_node_as_default_falls_back_to_another_source() {
            let mut graph = graph();
            record(&mut graph);
            let plan = plan_virtual_mic(&graph, None, VIRTUAL_MIC, Some(VIRTUAL_MIC_NODE_NAME));
            assert!(
                plan.iter()
                    .all(|link| link.output_node != VIRTUAL_MIC && link.output_node == MIC),
                "{plan:?}"
            );
            assert_eq!(plan.len(), 2);
        }

        #[test]
        fn nothing_before_the_virtual_mic_has_ports() {
            let mut graph = graph();
            record(&mut graph);
            assert!(plan_virtual_mic(&graph, Some(INJECT), 77, Some("mic")).is_empty());
        }
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
