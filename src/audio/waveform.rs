//! Waveform overviews for the trim editor: the minimum and maximum sample of equally long slices
//! ("buckets") of a decoded sound.

use std::time::Duration;

use super::format::{CHANNELS, SAMPLE_RATE};

/// How many buckets a waveform has at most; about one per pixel of a wide editor window.
pub const BUCKETS: usize = 1500;

/// The lowest and highest sample of a bucket, both channels together.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Peak {
    pub min: f32,
    pub max: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Waveform {
    /// [`BUCKETS`] peaks, or one per frame for sounds with fewer frames.
    pub peaks: Vec<Peak>,
    /// The length of the whole (untrimmed) sound.
    pub duration: Duration,
}

impl Waveform {
    /// The waveform of interleaved stereo `samples` at [`SAMPLE_RATE`]. Takes one pass over them;
    /// run it off the UI and real-time threads.
    pub fn of(samples: &[f32]) -> Self {
        let frames = samples.len() / CHANNELS;
        Self {
            peaks: peaks(samples, CHANNELS, BUCKETS),
            duration: Duration::from_secs_f64(frames as f64 / f64::from(SAMPLE_RATE)),
        }
    }
}

/// The peaks of interleaved `samples` with `channels` channels in `buckets` equally long buckets,
/// or one bucket per frame if there are fewer frames. Bucket `i` covers the frames from
/// `i * frames / buckets` up to `(i + 1) * frames / buckets`.
pub fn peaks(samples: &[f32], channels: usize, buckets: usize) -> Vec<Peak> {
    let frames = samples.len() / channels;
    let buckets = buckets.min(frames);
    (0..buckets)
        .map(|bucket| {
            let first = bucket * frames / buckets;
            let end = (bucket + 1) * frames / buckets;
            peak(&samples[first * channels..end * channels])
        })
        .collect()
}

fn peak(samples: &[f32]) -> Peak {
    samples.iter().fold(
        Peak {
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
        },
        |peak, &sample| Peak {
            min: peak.min.min(sample),
            max: peak.max.max(sample),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_sounds_get_the_requested_number_of_buckets() {
        let samples = vec![0.0; 2 * 10_007];
        assert_eq!(peaks(&samples, 2, 100).len(), 100);
    }

    #[test]
    fn buckets_hold_the_min_and_max_of_both_channels() {
        // Four stereo frames in two buckets.
        let samples = [0.1, -0.2, 0.5, 0.0, -0.9, 0.3, 0.2, 0.25];
        assert_eq!(
            peaks(&samples, 2, 2),
            [
                Peak {
                    min: -0.2,
                    max: 0.5
                },
                Peak {
                    min: -0.9,
                    max: 0.3
                }
            ]
        );
    }

    #[test]
    fn short_sounds_get_one_bucket_per_frame() {
        let samples = [0.1, 0.2, -0.3, -0.4, 0.5, 0.6];
        let peaks = peaks(&samples, 2, 1000);
        assert_eq!(peaks.len(), 3);
        assert_eq!(
            peaks[1],
            Peak {
                min: -0.4,
                max: -0.3
            }
        );
    }

    #[test]
    fn an_empty_sound_has_no_peaks() {
        assert!(peaks(&[], 2, 1000).is_empty());
    }

    #[test]
    fn every_frame_lands_in_a_bucket() {
        // One loud frame at the very end must show up in the last bucket.
        let mut samples = vec![0.0; 2 * 1001];
        samples[2 * 1000] = 1.0;
        let peaks = peaks(&samples, 2, 10);
        assert_eq!(peaks[9].max, 1.0);
        assert!(peaks[..9].iter().all(|peak| peak.max == 0.0));
    }

    #[test]
    fn the_waveform_knows_the_sound_length() {
        let samples = vec![0.0; CHANNELS * SAMPLE_RATE as usize * 3 / 2];
        let waveform = Waveform::of(&samples);
        assert_eq!(waveform.duration, Duration::from_millis(1500));
        assert_eq!(waveform.peaks.len(), BUCKETS);
    }
}
