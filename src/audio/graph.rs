//! A model of the parts of the PipeWire graph SoundBoars cares about: apps that record audio, and
//! the ports of nodes. Fed from registry and node-info events; pure and free of PipeWire types.

use std::collections::BTreeMap;

/// `media.class` of a stream that records audio, i.e. an app listening to a microphone.
pub const RECORDING_STREAM_CLASS: &str = "Stream/Input/Audio";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

impl Direction {
    /// Parses PipeWire's `port.direction` value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "in" => Some(Self::In),
            "out" => Some(Self::Out),
            _ => None,
        }
    }
}

/// A port of a node, identified by its global id and its `audio.channel` (`FL`, `FR`, `MONO`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub id: u32,
    pub channel: String,
}

/// An application stream that records audio, e.g. a voice chat app listening to the microphone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingApp {
    pub node_id: u32,
    pub app_name: String,
    pub process_binary: Option<String>,
    pub media_name: Option<String>,
    pub ports: Vec<Port>,
}

/// The properties of a node that SoundBoars uses, parsed from its full (node-info) properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeProps {
    pub media_class: String,
    pub app_name: String,
    pub process_binary: Option<String>,
    pub media_name: Option<String>,
}

impl NodeProps {
    /// Parses node properties; `get` looks up a property by key.
    pub fn parse<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Self {
        let owned = |key: &str| get(key).map(str::to_owned);
        Self {
            media_class: owned("media.class").unwrap_or_default(),
            app_name: owned("application.name")
                .or_else(|| owned("node.name"))
                .unwrap_or_default(),
            process_binary: owned("application.process.binary"),
            media_name: owned("media.name"),
        }
    }
}

/// An audio port and the node it belongs to, parsed from a port's registry properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortProps {
    pub node_id: u32,
    pub direction: Direction,
    pub port: Port,
}

impl PortProps {
    /// Parses port properties; `None` for ports that aren't audio ports of a node.
    pub fn parse<'a>(id: u32, get: impl Fn(&str) -> Option<&'a str>) -> Option<Self> {
        Some(Self {
            node_id: get("node.id")?.parse().ok()?,
            direction: Direction::parse(get("port.direction")?)?,
            port: Port {
                id,
                channel: get("audio.channel")?.to_owned(),
            },
        })
    }
}

#[derive(Debug, Default)]
pub struct Graph {
    nodes: BTreeMap<u32, NodeProps>,
    ports: BTreeMap<u32, PortProps>,
}

impl Graph {
    /// Adds a node or replaces its properties.
    pub fn set_node(&mut self, id: u32, props: NodeProps) {
        self.nodes.insert(id, props);
    }

    pub fn add_port(&mut self, props: PortProps) {
        self.ports.insert(props.port.id, props);
    }

    /// Removes the node or port with this global id; unknown ids are ignored.
    pub fn remove(&mut self, id: u32) {
        self.nodes.remove(&id);
        self.ports.remove(&id);
    }

    /// The ports of `node_id` in `direction`, ordered by port id.
    pub fn ports(&self, node_id: u32, direction: Direction) -> Vec<Port> {
        self.ports
            .values()
            .filter(|props| props.node_id == node_id && props.direction == direction)
            .map(|props| props.port.clone())
            .collect()
    }

    /// All known recording streams with their input ports, ordered by node id.
    pub fn recording_apps(&self) -> Vec<RecordingApp> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.media_class == RECORDING_STREAM_CLASS)
            .map(|(&node_id, node)| RecordingApp {
                node_id,
                app_name: node.app_name.clone(),
                process_binary: node.process_binary.clone(),
                media_name: node.media_name.clone(),
                ports: self.ports(node_id, Direction::In),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(props: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<&'a str> {
        |key| props.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    fn discord() -> NodeProps {
        NodeProps::parse(lookup(&[
            ("media.class", RECORDING_STREAM_CLASS),
            ("application.name", "WEBRTC VoiceEngine"),
            ("application.process.binary", "Discord"),
            ("media.name", "recStream"),
        ]))
    }

    fn port(node_id: u32, id: u32, direction: &str, channel: &str) -> PortProps {
        let node_id = node_id.to_string();
        PortProps::parse(
            id,
            lookup(&[
                ("node.id", &node_id),
                ("port.direction", direction),
                ("audio.channel", channel),
            ]),
        )
        .expect("test port props are complete")
    }

    #[test]
    fn parses_node_props() {
        assert_eq!(
            discord(),
            NodeProps {
                media_class: RECORDING_STREAM_CLASS.to_owned(),
                app_name: "WEBRTC VoiceEngine".to_owned(),
                process_binary: Some("Discord".to_owned()),
                media_name: Some("recStream".to_owned()),
            }
        );
    }

    #[test]
    fn app_name_falls_back_to_node_name() {
        let props = NodeProps::parse(lookup(&[("node.name", "pw-record")]));
        assert_eq!(props.app_name, "pw-record");
        assert_eq!(props.process_binary, None);
    }

    #[test]
    fn ports_without_channel_or_node_are_ignored() {
        assert_eq!(PortProps::parse(1, lookup(&[("node.id", "5")])), None);
        assert_eq!(
            PortProps::parse(
                1,
                lookup(&[("port.direction", "in"), ("audio.channel", "FL")])
            ),
            None
        );
    }

    #[test]
    fn lists_recording_apps_with_their_input_ports() {
        let mut graph = Graph::default();
        graph.set_node(40, discord());
        graph.add_port(port(40, 42, "in", "FR"));
        graph.add_port(port(40, 41, "in", "FL"));
        graph.add_port(port(40, 43, "out", "MONO"));

        let apps = graph.recording_apps();

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].node_id, 40);
        assert_eq!(apps[0].process_binary.as_deref(), Some("Discord"));
        let channels: Vec<_> = apps[0].ports.iter().map(|p| p.channel.as_str()).collect();
        assert_eq!(channels, ["FL", "FR"]);
    }

    #[test]
    fn ignores_nodes_that_are_not_recording_streams() {
        let mut graph = Graph::default();
        let sink = NodeProps::parse(lookup(&[("media.class", "Audio/Sink")]));
        graph.set_node(7, sink);
        assert!(graph.recording_apps().is_empty());
    }

    #[test]
    fn updated_node_props_replace_old_ones() {
        let mut graph = Graph::default();
        graph.set_node(40, discord());
        let mut renamed = discord();
        renamed.media_name = Some("Voice".to_owned());
        graph.set_node(40, renamed);
        assert_eq!(
            graph.recording_apps()[0].media_name.as_deref(),
            Some("Voice")
        );
    }

    #[test]
    fn removing_nodes_and_ports() {
        let mut graph = Graph::default();
        graph.set_node(40, discord());
        graph.add_port(port(40, 41, "in", "FL"));
        graph.add_port(port(40, 42, "in", "FR"));

        graph.remove(41);
        assert_eq!(graph.ports(40, Direction::In).len(), 1);

        graph.remove(40);
        assert!(graph.recording_apps().is_empty());

        graph.remove(999);
    }

    #[test]
    fn ports_are_filtered_by_node_and_direction() {
        let mut graph = Graph::default();
        graph.add_port(port(1, 10, "out", "FL"));
        graph.add_port(port(1, 11, "out", "FR"));
        graph.add_port(port(2, 20, "out", "FL"));
        graph.add_port(port(1, 12, "in", "FL"));

        let ids: Vec<_> = graph
            .ports(1, Direction::Out)
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, [10, 11]);
    }
}
