//! Everything that talks to PipeWire: the graph model, the inject stream and the thread that owns
//! them.

mod format;
pub mod graph;
mod inject;
pub mod link_plan;
mod service;
pub mod sine;

pub use format::{CHANNELS, SAMPLE_RATE};
pub use graph::{Port, RecordingApp};
pub use inject::INJECT_NODE_NAME;
pub use service::{Event, PipeWireService};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("PipeWire: {0}")]
    PipeWire(#[from] pipewire::Error),
    #[error("could not build the stream's audio format")]
    Format,
    #[error("could not start the PipeWire thread: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the PipeWire thread stopped unexpectedly")]
    ThreadGone,
}
