// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sample-rate conversion.
//!
//! The work is done by rubato, behind the crate's own `Resampler` trait: rubato's
//! interface has changed often, so its version is pinned and only
//! `resample/rubato_backend.rs` knows it. Rate pairs with a small common divisor
//! (44.1 ↔ 48 kHz and every other usual pair) use its FFT-based synchronous
//! resampler; other pairs use its windowed-sinc resampler.
//!
//! [`ResampleStage`] turns the resampler's fixed chunks into a stream: it takes
//! blocks of any length, removes the resampler's start-up delay so that output is
//! aligned with input, and at the end of a source emits the tail, so that `n` input
//! frames become `ceil(n × to / from)` output frames. Its buffers are allocated once,
//! when it is built; processing never allocates.

mod rubato_backend;

use std::fmt;

use helikodu_model::audio::SampleRate;

use super::PipelineError;

/// How much effort the resampler spends on quality.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ResamplerQuality {
    /// Short filters; for slow machines.
    Low,
    /// A balance of cost and quality.
    Medium,
    /// Transparent for listening; the default.
    #[default]
    High,
    /// The longest filters.
    Maximum,
}

impl ResamplerQuality {
    /// The name shown in the signal path.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Maximum => "maximum",
        }
    }
}

impl fmt::Display for ResamplerQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which kind of resampler converts a rate pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResampleMethod {
    /// FFT-based, for a fixed ratio of small integers.
    Synchronous,
    /// Windowed-sinc interpolation, for any ratio.
    Asynchronous,
}

impl ResampleMethod {
    /// The largest number of frames in the smallest exact block of a rate pair
    /// (`rate / gcd`) for which the synchronous resampler is used.
    const MAX_SYNC_BLOCK: u32 = 2048;

    /// The method for converting `from` to `to`.
    pub fn for_rates(from: SampleRate, to: SampleRate) -> Self {
        let gcd = gcd(from.hz(), to.hz());
        if from.hz() / gcd <= Self::MAX_SYNC_BLOCK && to.hz() / gcd <= Self::MAX_SYNC_BLOCK {
            Self::Synchronous
        } else {
            Self::Asynchronous
        }
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// A resampler that converts fixed chunks of interleaved 64-bit frames.
///
/// The crate's seam between the pipeline and the resampling library.
pub(crate) trait Resampler: Send + fmt::Debug {
    /// The number of input frames each chunk takes. Constant for one resampler.
    fn input_frames(&self) -> usize;

    /// The largest number of frames one chunk can produce.
    fn output_frames_max(&self) -> usize;

    /// The delay, in output frames, before input appears in the output.
    fn output_delay(&self) -> usize;

    /// Resamples one chunk. `input` holds [`input_frames`](Self::input_frames)
    /// frames, of which the first `valid` are audio and the rest count as silence.
    /// `output` holds at least [`output_frames_max`](Self::output_frames_max)
    /// frames. Returns the number of frames produced.
    fn process_chunk(&mut self, input: &[f64], valid: usize, output: &mut [f64]) -> usize;

    /// Clears all state, as if just built.
    fn reset(&mut self);
}

/// The resampling stage: a resampler fed with blocks of any length.
#[derive(Debug)]
pub struct ResampleStage {
    resampler: Box<dyn Resampler>,
    from: SampleRate,
    to: SampleRate,
    method: ResampleMethod,
    quality: ResamplerQuality,
    channels: usize,
    /// One input chunk, filled up to `fill` frames.
    input: Box<[f64]>,
    fill: usize,
    /// One output chunk.
    chunk: Box<[f64]>,
    /// Start-up delay frames still to drop.
    skip: usize,
    frames_in: u64,
    frames_out: u64,
    finishing: bool,
}

impl ResampleStage {
    /// A stage that converts `channels` channels from `from` to `to`.
    pub fn new(
        from: SampleRate,
        to: SampleRate,
        channels: usize,
        quality: ResamplerQuality,
    ) -> Result<Self, PipelineError> {
        if channels == 0 {
            return Err(PipelineError::InvalidShape("no channels"));
        }
        let method = ResampleMethod::for_rates(from, to);
        let resampler = rubato_backend::build(from, to, channels, method, quality)?;
        let input = vec![0.0; resampler.input_frames() * channels].into_boxed_slice();
        let chunk = vec![0.0; resampler.output_frames_max() * channels].into_boxed_slice();
        let skip = resampler.output_delay();
        Ok(Self {
            resampler,
            from,
            to,
            method,
            quality,
            channels,
            input,
            fill: 0,
            chunk,
            skip,
            frames_in: 0,
            frames_out: 0,
            finishing: false,
        })
    }

    /// The source rate.
    pub fn from_rate(&self) -> SampleRate {
        self.from
    }

    /// The target rate.
    pub fn to_rate(&self) -> SampleRate {
        self.to
    }

    /// The kind of resampler in use.
    pub fn method(&self) -> ResampleMethod {
        self.method
    }

    /// The quality preset.
    pub fn quality(&self) -> ResamplerQuality {
        self.quality
    }

    /// The number of channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The output space, in frames, that is always enough for [`process`](Self::process)
    /// to take `input_frames` frames.
    pub fn max_output_frames(&self, input_frames: usize) -> usize {
        let chunk_in = self.resampler.input_frames();
        // With up to `chunk_in - 1` frames already waiting, this many chunks can run.
        input_frames.div_ceil(chunk_in) * self.resampler.output_frames_max()
    }

    /// How many input frames [`process`](Self::process) takes, at least, when given
    /// room for `output_frames` frames.
    pub fn accepts(&self, output_frames: usize) -> usize {
        let chunk_in = self.resampler.input_frames();
        let chunks = output_frames / self.resampler.output_frames_max();
        chunk_in - self.fill + chunks * chunk_in
    }

    /// Resamples the interleaved frames of `input` into `output` and returns the
    /// number of input frames taken and output frames written.
    ///
    /// All input is taken when `output` has room for
    /// [`max_output_frames`](Self::max_output_frames) of it; with less room, what
    /// is not taken must be passed again.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> (usize, usize) {
        if self.finishing {
            self.reset();
        }
        let channels = self.channels;
        let chunk_in = self.resampler.input_frames();
        let room = self.resampler.output_frames_max();
        let in_frames = input.len() / channels;
        let out_frames = output.len() / channels;
        let (mut taken, mut written) = (0, 0);
        loop {
            if self.fill == chunk_in {
                if out_frames - written < room {
                    break;
                }
                written += self.run_chunk(chunk_in, &mut output[written * channels..], usize::MAX);
                continue;
            }
            if taken == in_frames {
                break;
            }
            let n = (chunk_in - self.fill).min(in_frames - taken);
            let src = &input[taken * channels..(taken + n) * channels];
            let dst = &mut self.input[self.fill * channels..(self.fill + n) * channels];
            for (d, s) in dst.iter_mut().zip(src) {
                *d = f64::from(*s);
            }
            self.fill += n;
            taken += n;
        }
        self.frames_in += taken as u64;
        (taken, written)
    }

    /// Ends the source: emits the frames still inside the resampler into `output`
    /// and returns how many were written. Call again until it returns 0; then the
    /// output has `ceil(n × to / from)` frames for `n` input frames in all.
    ///
    /// `output` needs room for at least [`max_output_frames(1)`](Self::max_output_frames)
    /// frames to make progress. The next [`process`](Self::process) starts a new
    /// stream.
    pub fn finish(&mut self, output: &mut [f32]) -> usize {
        self.finishing = true;
        let expected = u64::try_from(
            (u128::from(self.frames_in) * u128::from(self.to.hz()))
                .div_ceil(u128::from(self.from.hz())),
        )
        .unwrap_or(u64::MAX);
        let channels = self.channels;
        let room = self.resampler.output_frames_max();
        let out_frames = output.len() / channels;
        let mut written = 0;
        while self.frames_out < expected && out_frames - written >= room {
            let limit = usize::try_from(expected - self.frames_out).unwrap_or(usize::MAX);
            let valid = self.fill;
            written += self.run_chunk(valid, &mut output[written * channels..], limit);
        }
        written
    }

    /// Clears all state for a new stream.
    pub fn reset(&mut self) {
        self.resampler.reset();
        self.fill = 0;
        self.skip = self.resampler.output_delay();
        self.frames_in = 0;
        self.frames_out = 0;
        self.finishing = false;
    }

    /// Runs one chunk with `valid` input frames and writes at most `limit` frames of
    /// its output, after the start-up delay, to `output`.
    fn run_chunk(&mut self, valid: usize, output: &mut [f32], limit: usize) -> usize {
        let produced = self
            .resampler
            .process_chunk(&self.input, valid, &mut self.chunk);
        self.fill = 0;
        let skip = self.skip.min(produced);
        self.skip -= skip;
        let n = (produced - skip).min(limit);
        let channels = self.channels;
        let src = &self.chunk[skip * channels..(skip + n) * channels];
        for (d, s) in output[..n * channels].iter_mut().zip(src) {
            *d = *s as f32;
        }
        self.frames_out += n as u64;
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(hz: u32) -> SampleRate {
        SampleRate::new(hz).unwrap()
    }

    #[test]
    fn method_depends_on_the_ratio() {
        assert_eq!(
            ResampleMethod::for_rates(rate(44_100), rate(48_000)),
            ResampleMethod::Synchronous
        );
        assert_eq!(
            ResampleMethod::for_rates(rate(96_000), rate(44_100)),
            ResampleMethod::Synchronous
        );
        assert_eq!(
            ResampleMethod::for_rates(rate(44_100), rate(44_056)),
            ResampleMethod::Asynchronous
        );
    }

    /// Runs `input` through a stage in uneven blocks and returns the output.
    pub(super) fn run(stage: &mut ResampleStage, input: &[f32], block: usize) -> Vec<f32> {
        let channels = stage.channels();
        let mut out = vec![0.0; stage.max_output_frames(block) * channels];
        let mut result = Vec::new();
        for piece in input.chunks(block * channels) {
            let (taken, written) = stage.process(piece, &mut out);
            assert_eq!(taken * channels, piece.len());
            result.extend_from_slice(&out[..written * channels]);
        }
        loop {
            let written = stage.finish(&mut out);
            if written == 0 {
                break;
            }
            result.extend_from_slice(&out[..written * channels]);
        }
        result
    }

    #[test]
    fn length_is_exact_for_both_methods() {
        for (from, to) in [(44_100, 48_000), (48_000, 44_100), (44_100, 44_056)] {
            let mut stage =
                ResampleStage::new(rate(from), rate(to), 2, ResamplerQuality::Medium).unwrap();
            for frames in [0usize, 1, 999, 10_007] {
                let input = vec![0.25f32; frames * 2];
                let out = run(&mut stage, &input, 333);
                let expected = (frames as u64 * u64::from(to)).div_ceil(u64::from(from));
                assert_eq!(out.len() as u64, expected * 2, "{from} -> {to}, {frames}");
                stage.reset();
            }
        }
    }

    #[test]
    fn short_output_is_taken_again() {
        let mut stage =
            ResampleStage::new(rate(44_100), rate(48_000), 1, ResamplerQuality::Low).unwrap();
        let input = vec![0.0f32; 5000];
        let mut out = vec![0.0f32; 10];
        let (taken, written) = stage.process(&input, &mut out);
        assert_eq!(written, 0);
        // One chunk fits into the resampler's input, but there is no room to run it.
        assert!(taken > 0 && taken < input.len());
        assert_eq!(stage.accepts(0), 0);
        let mut out = vec![0.0f32; stage.max_output_frames(input.len() - taken)];
        let (rest, _) = stage.process(&input[taken..], &mut out);
        assert_eq!(taken + rest, input.len());
    }

    #[test]
    fn no_channels_is_an_error() {
        assert!(ResampleStage::new(rate(44_100), rate(48_000), 0, ResamplerQuality::High).is_err());
    }
}
