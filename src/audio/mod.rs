//! Audio: decoding and caching sounds, mixing them on real-time threads, and everything that talks
//! to PipeWire (the graph model, the output streams and the thread that owns them).
//! [`AudioEngine`] is the API the rest of the app uses.

mod controller;
pub mod decode;
mod engine;
mod format;
pub mod graph;
pub mod link_plan;
pub mod loader;
pub mod mixer;
mod output;
mod service;
pub mod sine;
mod virtual_mic;
pub mod waveform;

pub use controller::{Destination, EngineEvent, PlayRequest, Playing, Trim};
pub use engine::{AudioEngine, PlayHandle};
pub use format::{CHANNELS, SAMPLE_RATE};
pub use graph::{Port, RecordingApp};
pub use mixer::{Retrigger, VoiceId};
pub use output::{INJECT_NODE_NAME, LOCAL_NODE_NAME, Render};
pub use service::{Event, PipeWireService, Renders};
pub use virtual_mic::{VIRTUAL_MIC_DESCRIPTION, VIRTUAL_MIC_NODE_NAME};
pub use waveform::{Peak, Waveform};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("PipeWire: {0}")]
    PipeWire(#[from] pipewire::Error),
    #[error("could not build the stream's audio format")]
    Format,
    #[error("could not start a thread: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the PipeWire thread stopped unexpectedly")]
    ThreadGone,
}
