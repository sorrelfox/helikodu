// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a tune ends, and the fade before that.

/// The end of a tune: a fade that starts at one frame and ends at another, where the
/// tune stops. Without an end the tune plays until the library stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Envelope {
    /// The frame the fade starts at, when there is an end.
    fade_start: Option<u64>,
    /// The frame after the last one.
    end: Option<u64>,
}

impl Envelope {
    /// No end: the tune plays until the library stops.
    pub const OPEN: Self = Self {
        fade_start: None,
        end: None,
    };

    /// A tune that plays `length` frames and fades out over the last `fade` of them
    /// (all of them if `fade` is longer).
    pub fn ending_at(length: u64, fade: u64) -> Self {
        Self {
            fade_start: Some(length.saturating_sub(fade)),
            end: Some(length),
        }
    }

    /// The frame after the last, when known.
    pub fn end(&self) -> Option<u64> {
        self.end
    }

    /// How many of `frames` frames from `position` are within the end.
    pub fn room(&self, position: u64, frames: usize) -> usize {
        match self.end {
            Some(end) => {
                let left = end.saturating_sub(position);
                usize::try_from(left).map_or(frames, |left| left.min(frames))
            }
            None => frames,
        }
    }

    /// Applies the fade to `samples`, interleaved frames of `channels` channels that
    /// start at `position`. The gain falls along a raised cosine, to silence at the
    /// end.
    pub fn apply(&self, position: u64, samples: &mut [f32], channels: usize) {
        let (Some(start), Some(end)) = (self.fade_start, self.end) else {
            return;
        };
        let channels = channels.max(1);
        let length = end.saturating_sub(start);
        let frames = samples.len() / channels;
        let last = position + frames as u64;
        if last <= start || length == 0 {
            return;
        }
        for (i, frame) in samples.chunks_exact_mut(channels).enumerate() {
            let at = position + i as u64;
            if at < start {
                continue;
            }
            let x = (at - start) as f64 / length as f64;
            let gain = if x >= 1.0 {
                0.0
            } else {
                0.5 + 0.5 * (std::f64::consts::PI * x).cos()
            };
            for sample in frame {
                *sample = (f64::from(*sample) * gain) as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_envelopes_change_nothing() {
        let mut samples = vec![1.0; 8];
        Envelope::OPEN.apply(0, &mut samples, 2);
        assert!(samples.iter().all(|&s| s == 1.0));
        assert_eq!(Envelope::OPEN.room(10, 5), 5);
        assert_eq!(Envelope::OPEN.end(), None);
    }

    #[test]
    fn fades_to_silence_at_the_end() {
        let envelope = Envelope::ending_at(100, 50);
        assert_eq!(envelope.room(90, 20), 10);
        assert_eq!(envelope.room(100, 20), 0);
        let mut samples = vec![1.0f32; 200];
        envelope.apply(0, &mut samples, 2);
        assert_eq!(samples[98], 1.0);
        assert_eq!(samples[100], 1.0);
        assert!((samples[150] - 0.5).abs() < 1e-6, "{}", samples[150]);
        assert!(samples[198] < 0.01);
        // Monotonic within the fade.
        let left: Vec<f32> = samples[100..].iter().step_by(2).copied().collect();
        assert!(left.windows(2).all(|pair| pair[1] <= pair[0]));
    }

    #[test]
    fn a_fade_longer_than_the_tune_starts_at_once() {
        let envelope = Envelope::ending_at(10, 50);
        let mut samples = vec![1.0f32; 10];
        envelope.apply(0, &mut samples, 1);
        assert_eq!(samples[0], 1.0);
        assert!(samples[9] < 0.1);
    }
}
