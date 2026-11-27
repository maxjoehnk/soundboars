//! A model of the parts of the PipeWire graph SoundBoars cares about: apps that record audio,
//! microphones (sources), the ports of nodes and the links between them. Fed from registry,
//! node-info and metadata events; pure and free of PipeWire types.

use std::collections::BTreeMap;

use serde::Deserialize;

/// `media.class` of a stream that records audio, i.e. an app listening to a microphone.
pub const RECORDING_STREAM_CLASS: &str = "Stream/Input/Audio";
/// `media.class`es of sources a microphone can be mixed in from.
pub const SOURCE_CLASSES: [&str; 2] = ["Audio/Source", "Audio/Source/Virtual"];
/// The key of the `default` metadata (subject 0) that names the default source.
pub const DEFAULT_SOURCE_KEY: &str = "default.audio.source";

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

/// A node that captures audio, e.g. a microphone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub node_id: u32,
    /// `node.name`, what the `default` metadata names.
    pub name: String,
    /// What to show the user: `node.description`, else `node.nick`, else `node.name`.
    pub description: String,
    /// `priority.session`; the session manager prefers higher ones as default.
    pub priority: i64,
}

/// The properties of a node that SoundBoars uses, parsed from its full (node-info) properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeProps {
    pub media_class: String,
    pub app_name: String,
    pub process_binary: Option<String>,
    pub media_name: Option<String>,
    pub node_name: String,
    pub description: Option<String>,
    pub priority: i64,
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
            node_name: owned("node.name").unwrap_or_default(),
            description: owned("node.description").or_else(|| owned("node.nick")),
            priority: get("priority.session")
                .and_then(|priority| priority.parse().ok())
                .unwrap_or_default(),
        }
    }

    pub fn is_source(&self) -> bool {
        SOURCE_CLASSES.contains(&self.media_class.as_str())
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

/// The nodes a link connects, parsed from a link's registry properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkProps {
    pub output_node: u32,
    pub input_node: u32,
}

impl LinkProps {
    /// Parses link properties; `None` if a node id is missing.
    pub fn parse<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Option<Self> {
        Some(Self {
            output_node: get("link.output.node")?.parse().ok()?,
            input_node: get("link.input.node")?.parse().ok()?,
        })
    }
}

/// The node name in a `default` metadata value such as `{"name":"alsa_input.usb-mic"}`.
pub fn parse_default_node(json: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct DefaultNode {
        name: String,
    }
    serde_json::from_str::<DefaultNode>(json)
        .ok()
        .map(|node| node.name)
}

/// Whether a PipeWire server of `version` (`1.4.2`) may stop telling clients about metadata
/// changes: before 1.6.9, a client that bound the metadata and left before the server's ping to its
/// owner came back leaked a counter, and from then on no bound client got property changes
/// (PipeWire #5445). An unreadable version counts as affected.
pub fn may_lose_metadata_updates(version: &str) -> bool {
    let mut numbers = version
        .split(|c: char| !c.is_ascii_digit())
        .map(|part| part.parse::<u32>().ok());
    let parsed = (|| Some((numbers.next()??, numbers.next()??, numbers.next()??)))();
    parsed.is_none_or(|version| version < (1, 6, 9))
}

#[derive(Debug, Default)]
pub struct Graph {
    nodes: BTreeMap<u32, NodeProps>,
    ports: BTreeMap<u32, PortProps>,
    links: BTreeMap<u32, LinkProps>,
}

impl Graph {
    /// Adds a node or replaces its properties.
    pub fn set_node(&mut self, id: u32, props: NodeProps) {
        self.nodes.insert(id, props);
    }

    pub fn add_port(&mut self, props: PortProps) {
        self.ports.insert(props.port.id, props);
    }

    pub fn add_link(&mut self, id: u32, props: LinkProps) {
        self.links.insert(id, props);
    }

    /// Removes the node, port or link with this global id; unknown ids are ignored.
    pub fn remove(&mut self, id: u32) {
        self.nodes.remove(&id);
        self.ports.remove(&id);
        self.links.remove(&id);
    }

    /// Whether some other node takes audio from `node_id`, e.g. an app records it.
    pub fn has_consumers(&self, node_id: u32) -> bool {
        self.links
            .values()
            .any(|link| link.output_node == node_id && link.input_node != node_id)
    }

    /// All known sources (microphones and virtual sources), ordered by node id.
    pub fn sources(&self) -> Vec<Source> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.is_source())
            .map(|(&node_id, node)| Source {
                node_id,
                name: node.node_name.clone(),
                description: node
                    .description
                    .clone()
                    .unwrap_or_else(|| node.node_name.clone()),
                priority: node.priority,
            })
            .collect()
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
                node_name: String::new(),
                description: None,
                priority: 0,
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

    fn source(name: &str, description: Option<&str>, priority: &str) -> NodeProps {
        let mut props = vec![
            ("media.class", "Audio/Source"),
            ("node.name", name),
            ("priority.session", priority),
        ];
        props.extend(description.map(|description| ("node.description", description)));
        NodeProps::parse(lookup(&props))
    }

    #[test]
    fn lists_sources_with_description_and_priority() {
        let mut graph = Graph::default();
        graph.set_node(5, source("alsa_input.usb", Some("USB Mic"), "2000"));
        graph.set_node(6, source("bare", None, "oops"));
        graph.set_node(
            7,
            NodeProps::parse(lookup(&[("media.class", "Audio/Sink")])),
        );
        let mut virtual_source = source("virt", None, "1");
        virtual_source.media_class = "Audio/Source/Virtual".to_owned();
        graph.set_node(8, virtual_source);

        let sources = graph.sources();

        assert_eq!(
            sources,
            [
                Source {
                    node_id: 5,
                    name: "alsa_input.usb".to_owned(),
                    description: "USB Mic".to_owned(),
                    priority: 2000,
                },
                Source {
                    node_id: 6,
                    name: "bare".to_owned(),
                    description: "bare".to_owned(),
                    priority: 0,
                },
                Source {
                    node_id: 8,
                    name: "virt".to_owned(),
                    description: "virt".to_owned(),
                    priority: 1,
                },
            ]
        );
        assert!(graph.recording_apps().is_empty());
    }

    #[test]
    fn description_falls_back_to_nick() {
        let props = NodeProps::parse(lookup(&[("node.nick", "Nick")]));
        assert_eq!(props.description.as_deref(), Some("Nick"));
    }

    #[test]
    fn consumers_are_links_out_of_a_node_into_another() {
        let mut graph = Graph::default();
        let link = |output_node, input_node| LinkProps {
            output_node,
            input_node,
        };
        graph.add_link(100, link(1, 2));
        graph.add_link(101, link(3, 3));

        assert!(graph.has_consumers(1));
        assert!(!graph.has_consumers(2));
        assert!(!graph.has_consumers(3));

        graph.remove(100);
        assert!(!graph.has_consumers(1));
    }

    #[test]
    fn parses_link_props() {
        assert_eq!(
            LinkProps::parse(lookup(&[
                ("link.output.node", "50"),
                ("link.input.node", "47")
            ])),
            Some(LinkProps {
                output_node: 50,
                input_node: 47
            })
        );
        assert_eq!(
            LinkProps::parse(lookup(&[("link.output.node", "50")])),
            None
        );
    }

    #[test]
    fn parses_the_default_node_name() {
        assert_eq!(
            parse_default_node(r#"{"name":"alsa_input.usb"}"#).as_deref(),
            Some("alsa_input.usb")
        );
        assert_eq!(parse_default_node("{}"), None);
        assert_eq!(parse_default_node("not json"), None);
    }

    #[test]
    fn pipewire_before_1_6_9_may_lose_metadata_updates() {
        for version in ["1.4.2", "1.6.8", "0.3.80", "garbage", ""] {
            assert!(may_lose_metadata_updates(version), "{version}");
        }
        for version in ["1.6.9", "1.6.12", "1.7.0", "2.0.0"] {
            assert!(!may_lose_metadata_updates(version), "{version}");
        }
    }
}
