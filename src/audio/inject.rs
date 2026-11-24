//! The `soundboars-inject` output stream. It is never autoconnected; the session links it to the
//! chosen app's recording stream.

use std::io::Cursor;

use pipewire as pw;
use pw::core::CoreRc;
use pw::properties::properties;
use pw::spa;
use pw::stream::{Stream, StreamFlags, StreamListener, StreamRc};
use spa::param::audio::{AudioFormat, AudioInfoRaw, MAX_CHANNELS};
use spa::pod::serialize::PodSerializer;
use spa::pod::{Object, Pod, Value};

use super::AudioError;
use super::format::{CHANNELS, SAMPLE_RATE};

pub const INJECT_NODE_NAME: &str = "soundboars-inject";

const FRAME_BYTES: usize = CHANNELS * size_of::<f32>();

/// Fills interleaved stereo f32 frames. Runs on PipeWire's real-time data thread: it must not
/// allocate, lock, log or block.
pub type Render = Box<dyn FnMut(&mut [f32]) + Send>;

pub struct InjectStream {
    stream: StreamRc,
    _listener: StreamListener<Render>,
}

impl InjectStream {
    pub fn connect(core: &CoreRc, render: Render) -> Result<Self, AudioError> {
        let stream = StreamRc::new(core.clone(), INJECT_NODE_NAME, stream_properties())?;
        let listener = stream
            .add_local_listener_with_user_data(render)
            .process(process)
            .register()?;
        let format = format_pod()?;
        let format = Pod::from_bytes(&format).ok_or(AudioError::Format)?;
        // No AUTOCONNECT: the session decides where this stream goes.
        stream.connect(
            spa::utils::Direction::Output,
            None,
            StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut [format],
        )?;
        Ok(Self {
            stream,
            _listener: listener,
        })
    }

    pub fn stream(&self) -> &StreamRc {
        &self.stream
    }
}

fn stream_properties() -> pw::properties::PropertiesBox {
    properties! {
        *pw::keys::NODE_NAME => INJECT_NODE_NAME,
        *pw::keys::NODE_DESCRIPTION => "SoundBoars",
        *pw::keys::APP_NAME => "SoundBoars",
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_CLASS => "Stream/Output/Audio",
        *pw::keys::NODE_AUTOCONNECT => "false",
    }
}

/// The only format we offer: F32 interleaved stereo (FL, FR) at 48 kHz.
fn format_pod() -> Result<Vec<u8>, AudioError> {
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    info.set_rate(SAMPLE_RATE);
    info.set_channels(CHANNELS as u32);
    let mut position = [0; MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);

    let object = Value::Object(Object {
        type_: spa::sys::SPA_TYPE_OBJECT_Format,
        id: spa::sys::SPA_PARAM_EnumFormat,
        properties: info.into(),
    });
    let (cursor, _) = PodSerializer::serialize(Cursor::new(Vec::new()), &object)
        .map_err(|_| AudioError::Format)?;
    Ok(cursor.into_inner())
}

/// Real-time callback: hands the next buffer to `render`. Allocation-free.
fn process(stream: &Stream, render: &mut Render) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let requested_frames = buffer.requested() as usize;
    let Some(data) = buffer.datas_mut().first_mut() else {
        return;
    };
    let frames = data
        .data()
        .map_or(0, |bytes| render_into(bytes, requested_frames, render));
    let chunk = data.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = FRAME_BYTES as i32;
    *chunk.size_mut() = (frames * FRAME_BYTES) as u32;
}

/// Renders into a mapped buffer and returns the number of frames written. `requested_frames == 0`
/// means PipeWire didn't say how many it wants, so the whole buffer is filled.
fn render_into(bytes: &mut [u8], requested_frames: usize, render: &mut Render) -> usize {
    let usable = bytes.len() - bytes.len() % FRAME_BYTES;
    let Ok(samples) = bytemuck::try_cast_slice_mut::<u8, f32>(&mut bytes[..usable]) else {
        return 0; // Misaligned buffer; PipeWire maps buffers aligned, so this shouldn't happen.
    };
    let capacity = samples.len() / CHANNELS;
    let frames = match requested_frames {
        0 => capacity,
        requested => requested.min(capacity),
    };
    render(&mut samples[..frames * CHANNELS]);
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(value: f32) -> Render {
        Box::new(move |frames: &mut [f32]| frames.fill(value))
    }

    fn aligned_bytes(frames: usize) -> Vec<f32> {
        vec![0.0; frames * CHANNELS]
    }

    #[test]
    fn fills_only_the_requested_frames() {
        let mut buffer = aligned_bytes(8);
        let frames = render_into(bytemuck::cast_slice_mut(&mut buffer), 3, &mut constant(1.0));
        assert_eq!(frames, 3);
        assert_eq!(buffer.iter().filter(|&&s| s == 1.0).count(), 3 * CHANNELS);
    }

    #[test]
    fn fills_the_whole_buffer_when_nothing_was_requested() {
        let mut buffer = aligned_bytes(8);
        let frames = render_into(bytemuck::cast_slice_mut(&mut buffer), 0, &mut constant(1.0));
        assert_eq!(frames, 8);
    }

    #[test]
    fn never_renders_past_the_buffer() {
        let mut buffer = aligned_bytes(4);
        let frames = render_into(
            bytemuck::cast_slice_mut(&mut buffer),
            100,
            &mut constant(1.0),
        );
        assert_eq!(frames, 4);
    }

    #[test]
    fn format_pod_is_valid() {
        let bytes = format_pod().expect("format serializes");
        assert!(Pod::from_bytes(&bytes).is_some());
    }
}
