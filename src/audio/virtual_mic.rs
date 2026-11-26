//! The virtual microphone "SoundBoars Microphone": a null-audio-sink adapter node with the source
//! class, so call apps can pick it as their input device. [`super::PipeWireService`] links the
//! inject stream and the user's real microphone into its input ports; PipeWire mixes them and
//! offers the sum on its capture ports.

use pipewire as pw;
use pw::core::CoreRc;
use pw::node::Node;
use pw::properties::properties;

pub const VIRTUAL_MIC_NODE_NAME: &str = "soundboars-mic";
pub const VIRTUAL_MIC_DESCRIPTION: &str = "SoundBoars Microphone";

/// Creates the node in the PipeWire server. It lives as long as the returned proxy
/// (`object.linger = false`).
pub fn create(core: &CoreRc) -> Result<Node, pw::Error> {
    let props = properties! {
        *pw::keys::FACTORY_NAME => "support.null-audio-sink",
        *pw::keys::NODE_NAME => VIRTUAL_MIC_NODE_NAME,
        *pw::keys::NODE_DESCRIPTION => VIRTUAL_MIC_DESCRIPTION,
        *pw::keys::MEDIA_CLASS => "Audio/Source/Virtual",
        "audio.position" => "[FL,FR]",
        *pw::keys::OBJECT_LINGER => "false",
    };
    core.create_object::<Node>("adapter", &props)
}
