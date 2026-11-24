//! Decodes audio files into the format SoundBoars plays: interleaved stereo f32 at 48 kHz.
//! Decoding is slow; it never runs on a real-time thread (see [`super::loader`]).

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;

use super::format::{CHANNELS, SAMPLE_RATE};

/// A decoded sound: interleaved stereo f32 frames at [`SAMPLE_RATE`]. Shared by both output streams.
pub type Samples = Arc<[f32]>;

/// Input frames per resampler chunk.
const RESAMPLE_CHUNK: usize = 1024;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("could not read the file: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not decode the file: {0}")]
    Format(#[from] SymphoniaError),
    #[error("the file has no audio track")]
    NoAudioTrack,
    #[error("the file contains no audio")]
    NoAudio,
    #[error("the channel layout changes in the middle of the file")]
    ChannelsChanged,
    #[error("could not set up resampling: {0}")]
    ResamplerSetup(#[from] rubato::ResamplerConstructionError),
    #[error("could not resample: {0}")]
    Resample(#[from] rubato::ResampleError),
}

/// Decodes a whole file to interleaved stereo f32 at 48 kHz.
pub fn decode_file(path: &Path) -> Result<Samples, DecodeError> {
    let decoded = decode_interleaved(path)?;
    let stereo = to_stereo(&decoded.samples, decoded.channels);
    Ok(resample(stereo, decoded.rate)?.into())
}

/// Interleaved samples as the file stores them.
struct Decoded {
    samples: Vec<f32>,
    channels: usize,
    rate: u32,
}

fn decode_interleaved(path: &Path) -> Result<Decoded, DecodeError> {
    let source = MediaSourceStream::new(Box::new(File::open(path)?), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        source,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or(DecodeError::NoAudioTrack)?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or(DecodeError::NoAudioTrack)?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())?;

    let mut decoded: Option<Decoded> = None;
    let mut packet_samples = Vec::new();
    while let Some(packet) = next_packet(format.as_mut())? {
        if packet.track_id != track_id {
            continue;
        }
        let buffer = match decoder.decode(&packet) {
            Ok(buffer) => buffer,
            // A damaged packet; skip it like players do.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(error.into()),
        };
        let spec = buffer.spec();
        let (channels, rate) = (spec.channels().count(), spec.rate());
        buffer.copy_to_vec_interleaved(&mut packet_samples);
        let decoded = decoded.get_or_insert_with(|| Decoded {
            samples: Vec::new(),
            channels,
            rate,
        });
        if decoded.channels != channels {
            return Err(DecodeError::ChannelsChanged);
        }
        decoded.samples.extend_from_slice(&packet_samples);
    }
    decoded
        .filter(|decoded| !decoded.samples.is_empty() && decoded.channels > 0)
        .ok_or(DecodeError::NoAudio)
}

/// The next packet, or `None` at the end of the stream.
fn next_packet(format: &mut dyn FormatReader) -> Result<Option<Packet>, DecodeError> {
    match format.next_packet() {
        Ok(packet) => Ok(packet),
        // Some readers report the end of a stream as an unexpected EOF.
        Err(SymphoniaError::IoError(error))
            if error.kind() == std::io::ErrorKind::UnexpectedEof =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

/// Converts interleaved samples with `channels` channels to interleaved stereo. Mono is duplicated;
/// beyond stereo, the first two channels are left and right and the rest (centre, surround, LFE) is
/// averaged and mixed into both sides at half level.
fn to_stereo(samples: &[f32], channels: usize) -> Vec<f32> {
    match channels {
        1 => samples.iter().flat_map(|&s| [s, s]).collect(),
        2 => samples.to_vec(),
        _ => samples
            .chunks_exact(channels)
            .flat_map(|frame| {
                let rest = &frame[2..];
                let extra = 0.5 * rest.iter().sum::<f32>() / rest.len() as f32;
                [frame[0] + extra, frame[1] + extra]
            })
            .collect(),
    }
}

/// Resamples interleaved stereo from `rate` to [`SAMPLE_RATE`].
fn resample(stereo: Vec<f32>, rate: u32) -> Result<Vec<f32>, DecodeError> {
    let frames = stereo.len() / CHANNELS;
    if rate == SAMPLE_RATE || frames == 0 {
        return Ok(stereo);
    }
    let mut resampler = Fft::<f32>::new(
        rate as usize,
        SAMPLE_RATE as usize,
        RESAMPLE_CHUNK,
        CHANNELS,
        FixedSync::Input,
    )?;
    // Can't fail: `stereo` holds exactly `frames * CHANNELS` samples.
    let input = InterleavedSlice::new(&stereo[..], CHANNELS, frames)
        .expect("the slice holds `frames` stereo frames");
    let output = resampler.process_all(&input, frames, None)?;
    Ok(output.take_data())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn left(samples: &[f32]) -> Vec<f32> {
        samples.iter().step_by(CHANNELS).copied().collect()
    }

    /// Frequency by counting rising zero crossings of the loud part; good enough for a clean sine.
    /// Near-silent encoder padding is skipped, its noise would add crossings.
    fn frequency(samples: &[f32]) -> f32 {
        let is_loud = |s: &f32| s.abs() > 0.05;
        let first = samples.iter().position(is_loud).unwrap_or(0);
        let last = samples.iter().rposition(is_loud).unwrap_or(0);
        let samples = &samples[first..=last];
        let rising = samples
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        rising as f32 * SAMPLE_RATE as f32 / samples.len() as f32
    }

    /// Every fixture is a 0.5 s, 440 Hz sine at half amplitude.
    fn assert_is_the_fixture_tone(name: &str) {
        let samples = decode_file(&fixture(name)).expect(name);
        assert_eq!(samples.len() % CHANNELS, 0);
        let seconds = samples.len() as f32 / CHANNELS as f32 / SAMPLE_RATE as f32;
        // Lossy codecs add encoder delay and padding.
        assert!((0.45..0.6).contains(&seconds), "{name}: {seconds} s");
        let frequency = frequency(&left(&samples));
        assert!(
            (430.0..450.0).contains(&frequency),
            "{name}: {frequency} Hz"
        );
        let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!((0.4..0.6).contains(&peak), "{name}: peak {peak}");
    }

    #[test]
    fn decodes_wav() {
        assert_is_the_fixture_tone("tone-44k-mono.wav");
    }

    #[test]
    fn decodes_mp3() {
        assert_is_the_fixture_tone("tone.mp3");
    }

    #[test]
    fn decodes_flac() {
        assert_is_the_fixture_tone("tone.flac");
    }

    #[test]
    fn decodes_ogg_vorbis() {
        assert_is_the_fixture_tone("tone.ogg");
    }

    #[test]
    fn decodes_aac_in_mp4() {
        assert_is_the_fixture_tone("tone.m4a");
    }

    #[test]
    fn mono_44k_becomes_stereo_48k_of_the_same_length() {
        let samples = decode_file(&fixture("tone-44k-mono.wav")).expect("decodes");
        let frames = samples.len() / CHANNELS;
        // 0.5 s at 48 kHz, give or take the resampler's rounding.
        assert!((23_900..=24_100).contains(&frames), "{frames} frames");
        for frame in samples.as_chunks::<CHANNELS>().0 {
            assert_eq!(frame[0], frame[1]);
        }
    }

    #[test]
    fn a_file_that_is_not_audio_is_an_error() {
        let error = decode_file(&fixture("not-audio.mp3")).expect_err("not audio");
        assert!(matches!(error, DecodeError::Format(_)), "{error:?}");
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let error = decode_file(&fixture("missing.wav")).expect_err("missing");
        assert!(matches!(error, DecodeError::Io(_)), "{error:?}");
    }

    #[test]
    fn mono_is_duplicated() {
        assert_eq!(to_stereo(&[0.1, 0.2], 1), [0.1, 0.1, 0.2, 0.2]);
    }

    #[test]
    fn stereo_is_kept() {
        assert_eq!(to_stereo(&[0.1, 0.2], 2), [0.1, 0.2]);
    }

    #[test]
    fn surround_is_downmixed() {
        // FL, FR, FC, LFE: centre and LFE average to 0.4, mixed in at half level.
        let stereo = to_stereo(&[0.1, 0.2, 0.6, 0.2], 4);
        assert_eq!(stereo.len(), 2);
        assert!((stereo[0] - 0.3).abs() < 1e-6);
        assert!((stereo[1] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn audio_at_48k_is_not_resampled() {
        let stereo = vec![0.25; 100];
        assert_eq!(resample(stereo.clone(), SAMPLE_RATE).expect("ok"), stereo);
    }
}
