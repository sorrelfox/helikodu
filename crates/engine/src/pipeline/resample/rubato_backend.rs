// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The [`Resampler`] implemented with rubato. The only file that uses rubato's API.

use std::fmt;

use helikodu_model::audio::SampleRate;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, Fft, FixedAsync, FixedSync, Indexing, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};

use super::{ResampleMethod, Resampler, ResamplerQuality};
use crate::pipeline::PipelineError;

/// The number of input frames per chunk.
const CHUNK_FRAMES: usize = 1024;

/// Builds the resampler for a rate pair.
pub(super) fn build(
    from: SampleRate,
    to: SampleRate,
    channels: usize,
    method: ResampleMethod,
    quality: ResamplerQuality,
) -> Result<Box<dyn Resampler>, PipelineError> {
    let error = |reason: String| PipelineError::Resampler { from, to, reason };
    let (rate_in, rate_out) = (from.hz() as usize, to.hz() as usize);
    let inner: Box<dyn rubato::Resampler<f64>> = match method {
        ResampleMethod::Synchronous => {
            // Longer FFT blocks give a filter closer to the Nyquist frequency.
            let (block, window) = match quality {
                ResamplerQuality::Low => (128, WindowFunction::Hann2),
                ResamplerQuality::Medium => (256, WindowFunction::Blackman2),
                ResamplerQuality::High => (512, WindowFunction::BlackmanHarris2),
                ResamplerQuality::Maximum => (1024, WindowFunction::BlackmanHarris2),
            };
            Box::new(
                Fft::<f64>::new_custom(
                    rate_in,
                    rate_out,
                    CHUNK_FRAMES,
                    (CHUNK_FRAMES / block).max(1),
                    channels,
                    window,
                    FixedSync::Input,
                )
                .map_err(|e| error(e.to_string()))?,
            )
        }
        ResampleMethod::Asynchronous => {
            let (length, window, interpolation) = match quality {
                ResamplerQuality::Low => (64, WindowFunction::Hann2, SincInterpolationType::Linear),
                ResamplerQuality::Medium => (
                    128,
                    WindowFunction::Blackman2,
                    SincInterpolationType::Linear,
                ),
                ResamplerQuality::High => (
                    256,
                    WindowFunction::BlackmanHarris2,
                    SincInterpolationType::Cubic,
                ),
                ResamplerQuality::Maximum => (
                    512,
                    WindowFunction::BlackmanHarris2,
                    SincInterpolationType::Cubic,
                ),
            };
            let parameters = SincInterpolationParameters::new(length, window)
                .oversampling_factor(length)
                .interpolation(interpolation);
            Box::new(
                Async::<f64>::new_sinc(
                    rate_out as f64 / rate_in as f64,
                    1.0,
                    &parameters,
                    CHUNK_FRAMES,
                    channels,
                    FixedAsync::Input,
                )
                .map_err(|e| error(e.to_string()))?,
            )
        }
    };
    Ok(Box::new(RubatoResampler {
        inner,
        channels,
        method,
    }))
}

struct RubatoResampler {
    inner: Box<dyn rubato::Resampler<f64>>,
    channels: usize,
    method: ResampleMethod,
}

impl fmt::Debug for RubatoResampler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RubatoResampler")
            .field("method", &self.method)
            .field("channels", &self.channels)
            .field("ratio", &self.inner.resample_ratio())
            .finish_non_exhaustive()
    }
}

impl Resampler for RubatoResampler {
    fn input_frames(&self) -> usize {
        self.inner.input_frames_next()
    }

    fn output_frames_max(&self) -> usize {
        self.inner.output_frames_max()
    }

    fn output_delay(&self) -> usize {
        self.inner.output_delay()
    }

    fn process_chunk(&mut self, input: &[f64], valid: usize, output: &mut [f64]) -> usize {
        let channels = self.channels;
        let in_frames = input.len() / channels;
        let out_frames = output.len() / channels;
        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: (valid < in_frames).then_some(valid),
            active_channels_mask: None,
        };
        let result = match (
            InterleavedSlice::new(input, channels, in_frames),
            InterleavedSlice::new_mut(output, channels, out_frames),
        ) {
            (Ok(source), Ok(mut target)) => self
                .inner
                .process_into_buffer(&source, &mut target, Some(&indexing))
                .ok(),
            _ => None,
        };
        match result {
            Some((_, produced)) => produced,
            None => {
                // The buffers are sized from the resampler itself, so this does not
                // happen; if it did, a chunk of silence keeps the stream going.
                debug_assert!(false, "resampler refused its own buffer sizes");
                let frames = self.inner.output_frames_next().min(out_frames);
                output[..frames * channels].fill(0.0);
                frames
            }
        }
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}
