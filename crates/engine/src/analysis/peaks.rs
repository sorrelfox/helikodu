// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Waveform peaks for the seek bar: the largest sample of each stretch of a
//! track, in a fixed number of steps.
//!
//! [`PeakBuilder`] keeps the peak of every 10 ms, all channels together, and
//! [`PeakBuilder::finish`] folds them into at most [`WAVEFORM_BINS`] values from
//! 0 (silence) to 255 (full scale), each the largest of the 10 ms peaks it covers.
//! [`waveform_peaks`] decodes a whole source for it.

use helikodu_model::library::MediaSource;

use super::loudness::AnalysisError;
use crate::decode::{DecoderRegistry, PcmBlock};

/// How many values a waveform has at most: more than the seek bar has bars on any
/// screen.
pub const WAVEFORM_BINS: usize = 1024;

/// The stretch of sound each fine peak covers, in hundredths of a second.
const FINE_PER_SECOND: u32 = 100;

/// Collects the peaks of a track's samples; see the module documentation.
#[derive(Debug, Clone)]
pub struct PeakBuilder {
    /// Frames per fine peak.
    step: usize,
    /// Frames into the current fine peak.
    filled: usize,
    current: f32,
    fine: Vec<f32>,
    scratch: Vec<f32>,
}

impl PeakBuilder {
    /// A builder for audio at `rate` Hz.
    pub fn new(rate: u32) -> Self {
        Self {
            step: (rate / FINE_PER_SECOND).max(1) as usize,
            filled: 0,
            current: 0.0,
            fine: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// Adds interleaved samples of `channels` channels.
    pub fn add(&mut self, samples: &[f32], channels: usize) {
        for frame in samples.chunks_exact(channels.max(1)) {
            let peak = frame.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            self.current = self.current.max(peak);
            self.filled += 1;
            if self.filled == self.step {
                self.fine.push(self.current);
                self.current = 0.0;
                self.filled = 0;
            }
        }
    }

    /// Adds a decoded block.
    pub fn add_block(&mut self, block: &PcmBlock) {
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        block.append_f32(&mut scratch);
        self.add(&scratch, usize::from(block.spec().channel_count()));
        self.scratch = scratch;
    }

    /// The peaks in at most `bins` values, or `None` when nothing was added.
    pub fn finish(mut self, bins: usize) -> Option<Vec<u8>> {
        if self.filled > 0 {
            self.fine.push(self.current);
        }
        if self.fine.is_empty() || bins == 0 {
            return None;
        }
        Some(reduce(&self.fine, bins))
    }
}

/// Folds `fine` peaks (linear, 1.0 full scale) into at most `bins` values from 0
/// to 255, each the largest of the peaks it covers.
pub fn reduce(fine: &[f32], bins: usize) -> Vec<u8> {
    let count = fine.len().min(bins);
    (0..count)
        .map(|i| {
            let start = i * fine.len() / count;
            let end = ((i + 1) * fine.len() / count).max(start + 1);
            let peak = fine[start..end].iter().fold(0.0f32, |m, &x| m.max(x));
            (peak.clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect()
}

/// Decodes `source` to its end and returns its waveform peaks (at most `bins`).
/// `cancelled` is asked between blocks.
pub fn waveform_peaks(
    registry: &DecoderRegistry,
    source: &MediaSource,
    bins: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>, AnalysisError> {
    let mut decoder = registry.open(source)?;
    let mut builder = PeakBuilder::new(decoder.spec().rate.hz());
    let mut block = PcmBlock::new();
    loop {
        if cancelled() {
            return Err(AnalysisError::Cancelled);
        }
        if !decoder.read(&mut block)? {
            break;
        }
        builder.add_block(&block);
    }
    builder.finish(bins).ok_or(AnalysisError::Empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_follow_the_loudness_of_each_stretch() {
        // One second at 48 kHz, stereo: a quarter-scale first half, a full-scale
        // spike in the right channel at 0.75 s, silence otherwise.
        let rate = 48_000;
        let mut samples = vec![0.0f32; rate * 2];
        for i in 0..rate / 2 {
            let x = 0.25 * (i as f32 * 0.05).sin().signum();
            samples[i * 2] = x;
        }
        samples[(rate * 3 / 4) * 2 + 1] = -1.0;
        let mut builder = PeakBuilder::new(rate as u32);
        // In uneven pieces, as decoders hand them out.
        for piece in samples.chunks(2 * 999) {
            builder.add(piece, 2);
        }
        let peaks = builder.finish(4).unwrap();
        assert_eq!(peaks, vec![64, 64, 0, 255]);
    }

    #[test]
    fn short_sounds_keep_their_own_resolution() {
        let mut builder = PeakBuilder::new(1_000);
        // 25 ms at 1 kHz: 10 frames per fine peak, so three of them.
        builder.add(&[0.5; 25], 1);
        assert_eq!(builder.finish(WAVEFORM_BINS).unwrap(), vec![128, 128, 128]);
        assert_eq!(PeakBuilder::new(48_000).finish(10), None);
    }

    #[test]
    fn reducing_takes_the_largest() {
        let fine = [0.1, 0.9, 0.2, 0.3, 0.0, 0.4];
        assert_eq!(reduce(&fine, 3), vec![230, 77, 102]);
        assert_eq!(reduce(&fine, 100).len(), 6);
    }
}
