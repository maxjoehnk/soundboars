//! A sine tone generator, used to test injection independently of the mixer.

use std::f32::consts::TAU;

use super::format::{CHANNELS, SAMPLE_RATE};

#[derive(Debug, Clone)]
pub struct Sine {
    phase: f32,
    step: f32,
    amplitude: f32,
}

impl Sine {
    pub fn new(frequency: f32, amplitude: f32) -> Self {
        Self {
            phase: 0.0,
            step: TAU * frequency / SAMPLE_RATE as f32,
            amplitude,
        }
    }

    /// Fills interleaved frames (all channels get the same sample). Allocation-free.
    pub fn render(&mut self, frames: &mut [f32]) {
        for frame in frames.as_chunks_mut::<CHANNELS>().0 {
            frame.fill(self.phase.sin() * self.amplitude);
            self.phase = (self.phase + self.step) % TAU;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_identical_channels_within_amplitude() {
        let mut sine = Sine::new(440.0, 0.5);
        let mut buffer = vec![0.0; 480 * CHANNELS];
        sine.render(&mut buffer);
        for frame in buffer.as_chunks::<CHANNELS>().0 {
            assert_eq!(frame[0], frame[1]);
            assert!(frame[0].abs() <= 0.5);
        }
    }

    #[test]
    fn has_the_requested_frequency() {
        let mut sine = Sine::new(440.0, 1.0);
        // One second, rendered in small chunks like the audio callback would.
        let mut second = vec![0.0; SAMPLE_RATE as usize * CHANNELS];
        for chunk in second.chunks_mut(256 * CHANNELS) {
            sine.render(chunk);
        }
        let left: Vec<f32> = second.iter().step_by(CHANNELS).copied().collect();
        let rising_zero_crossings = left
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        assert!((439..=441).contains(&rising_zero_crossings));
    }
}
