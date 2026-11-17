// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! One block of decoded audio.

use helikodu_model::audio::{AudioSpec, ChannelLayout, Endianness, SampleFormat, SampleRate};

/// Interleaved samples, in the format the source decodes to.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockSamples {
    /// Unsigned 8-bit integers; 128 is silence.
    U8(Vec<u8>),
    /// Signed 16-bit integers.
    S16(Vec<i16>),
    /// Signed 32-bit integers. Sources with fewer significant bits (24-bit FLAC,
    /// for example) are in the top bits.
    S32(Vec<i32>),
    /// 32-bit floats, nominally between -1.0 and 1.0.
    F32(Vec<f32>),
}

impl BlockSamples {
    /// The number of samples over all channels.
    pub fn len(&self) -> usize {
        match self {
            Self::U8(v) => v.len(),
            Self::S16(v) => v.len(),
            Self::S32(v) => v.len(),
            Self::F32(v) => v.len(),
        }
    }

    /// Whether there are no samples.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The sample format, native-endian.
    pub fn format(&self) -> SampleFormat {
        match self {
            Self::U8(_) => SampleFormat::U8,
            Self::S16(_) => SampleFormat::S16(Endianness::NATIVE),
            Self::S32(_) => SampleFormat::S32(Endianness::NATIVE),
            Self::F32(_) => SampleFormat::F32(Endianness::NATIVE),
        }
    }

    /// Removes the samples `from .. to`.
    fn drain(&mut self, from: usize, to: usize) {
        match self {
            Self::U8(v) => drop(v.drain(from..to)),
            Self::S16(v) => drop(v.drain(from..to)),
            Self::S32(v) => drop(v.drain(from..to)),
            Self::F32(v) => drop(v.drain(from..to)),
        }
    }
}

/// One block of decoded audio: interleaved samples in canonical (WAVE) channel
/// order, its format, and the position of its first frame.
///
/// Decoders fill a block the caller owns, so its allocation is reused from one
/// block to the next.
#[derive(Debug, Clone, PartialEq)]
pub struct PcmBlock {
    spec: AudioSpec,
    position: u64,
    frames: usize,
    samples: BlockSamples,
}

impl Default for PcmBlock {
    fn default() -> Self {
        Self::new()
    }
}

impl PcmBlock {
    /// An empty block.
    pub fn new() -> Self {
        Self {
            spec: AudioSpec::new(
                SampleFormat::F32(Endianness::NATIVE),
                SampleRate::HZ_44100,
                ChannelLayout::STEREO,
            ),
            position: 0,
            frames: 0,
            samples: BlockSamples::F32(Vec::new()),
        }
    }

    /// Sets the block's contents. The sample format of [`spec`](Self::spec) is that
    /// of `samples`; the number of frames is the number of samples divided by the
    /// channel count of `layout`, which must divide it.
    pub fn set(
        &mut self,
        rate: SampleRate,
        layout: ChannelLayout,
        position: u64,
        samples: BlockSamples,
    ) {
        let channels = usize::from(layout.channel_count());
        debug_assert_eq!(samples.len() % channels, 0, "whole frames only");
        self.spec = AudioSpec::new(samples.format(), rate, layout);
        self.position = position;
        self.frames = samples.len() / channels;
        self.samples = samples;
    }

    /// Takes the samples out, leaving the block empty, so their allocation can be
    /// filled again and handed back with [`set`](Self::set).
    pub fn take_samples(&mut self) -> BlockSamples {
        self.frames = 0;
        std::mem::replace(&mut self.samples, BlockSamples::F32(Vec::new()))
    }

    /// The format of the samples: a native-endian `U8`, `S16`, `S32` or `F32`
    /// matching [`samples`](Self::samples), the rate and the channel layout.
    pub fn spec(&self) -> AudioSpec {
        self.spec
    }

    /// The position of the first frame, in frames from the start of the source.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// The number of frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Whether the block holds no frames.
    pub fn is_empty(&self) -> bool {
        self.frames == 0
    }

    /// The interleaved samples.
    pub fn samples(&self) -> &BlockSamples {
        &self.samples
    }

    /// Appends the samples to `out` as `f32`, scaled to -1.0 .. 1.0: integers are
    /// divided by 2^(bits − 1), so 8-, 16- and 24-bit values convert exactly; the
    /// low 8 bits of full 32-bit values are rounded away.
    pub fn append_f32(&self, out: &mut Vec<f32>) {
        const U8_SCALE: f32 = 1.0 / 128.0;
        const S16_SCALE: f32 = 1.0 / 32_768.0;
        const S32_SCALE: f32 = 1.0 / 2_147_483_648.0;
        match &self.samples {
            BlockSamples::U8(v) => {
                out.extend(v.iter().map(|&x| (f32::from(x) - 128.0) * U8_SCALE));
            }
            BlockSamples::S16(v) => out.extend(v.iter().map(|&x| f32::from(x) * S16_SCALE)),
            #[allow(clippy::cast_precision_loss)]
            BlockSamples::S32(v) => out.extend(v.iter().map(|&x| x as f32 * S32_SCALE)),
            BlockSamples::F32(v) => out.extend_from_slice(v),
        }
    }

    /// Drops the first `frames` frames; the position moves on by as many.
    pub(crate) fn skip_front(&mut self, frames: usize) {
        let frames = frames.min(self.frames);
        let channels = usize::from(self.spec.channel_count());
        self.samples.drain(0, frames * channels);
        self.frames -= frames;
        self.position += frames as u64;
    }

    /// Keeps only the first `frames` frames.
    pub(crate) fn truncate(&mut self, frames: usize) {
        if frames < self.frames {
            let channels = usize::from(self.spec.channel_count());
            self.samples
                .drain(frames * channels, self.frames * channels);
            self.frames = frames;
        }
    }

    /// Sets the position of the first frame.
    pub(crate) fn set_position(&mut self, position: u64) {
        self.position = position;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(samples: BlockSamples) -> PcmBlock {
        let mut b = PcmBlock::new();
        b.set(SampleRate::HZ_48000, ChannelLayout::STEREO, 100, samples);
        b
    }

    #[test]
    fn integer_samples_convert_exactly() {
        let mut out = Vec::new();
        block(BlockSamples::S16(vec![i16::MIN, -1, 0, i16::MAX])).append_f32(&mut out);
        assert_eq!(out, [-1.0, -1.0 / 32_768.0, 0.0, 32_767.0 / 32_768.0]);

        // 24-bit values in the top bits of 32-bit words survive the round trip.
        let ints = [-(1 << 23), -1, 1, (1 << 23) - 1];
        let words: Vec<i32> = ints.iter().map(|&x| x << 8).collect();
        let mut out = Vec::new();
        block(BlockSamples::S32(words)).append_f32(&mut out);
        let back: Vec<i32> = out
            .iter()
            .map(|&x| (f64::from(x) * f64::from(1 << 23)) as i32)
            .collect();
        assert_eq!(back, ints);

        let mut out = Vec::new();
        block(BlockSamples::U8(vec![0, 128, 255, 64])).append_f32(&mut out);
        assert_eq!(out, [-1.0, 0.0, 127.0 / 128.0, -0.5]);
    }

    #[test]
    fn spec_follows_the_samples() {
        let b = block(BlockSamples::S32(vec![0; 10]));
        assert_eq!(b.spec().format, SampleFormat::S32(Endianness::NATIVE));
        assert_eq!(b.frames(), 5);
        assert_eq!(b.position(), 100);
    }

    #[test]
    fn trimming_keeps_frames_whole() {
        let mut b = block(BlockSamples::S16((0..20).collect()));
        b.skip_front(3);
        assert_eq!((b.position(), b.frames()), (103, 7));
        b.truncate(2);
        assert_eq!(b.samples(), &BlockSamples::S16(vec![6, 7, 8, 9]));
        b.truncate(5);
        assert_eq!(b.frames(), 2);
        let taken = b.take_samples();
        assert_eq!(taken.len(), 4);
        assert!(b.is_empty());
    }
}
