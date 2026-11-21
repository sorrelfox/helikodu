// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Convolution with an impulse response, by uniformly partitioned FFT convolution
//! (overlap-save with a frequency-domain delay line).
//!
//! The impulse response is cut into partitions of `B` frames, each transformed once
//! when the stage is built (FFT size `2B`). Every `B` input frames, the newest
//! `2B` frames of input are transformed, stored in a ring of spectra, multiplied
//! with the partitions' spectra and summed; the inverse transform gives the next `B`
//! output frames. The stage therefore delays the audio by exactly `B` frames, which
//! it reports as its latency, whatever the length of the impulse response.
//!
//! A mono impulse response is used for every channel; a stereo one gives its left
//! channel to the even channels and its right channel to the odd ones. The
//! unprocessed signal is delayed by the same `B` frames for the wet/dry mix.
//! Everything is allocated when the stage is built.

use std::sync::Arc;

use helikodu_model::settings::ConvolutionSettings;
use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::DspStage;
use super::impulse::Impulse;
use super::plan::ConvolutionPlan;

/// The partition length for `rate` Hz: about 10 ms.
pub fn partition_for(rate: u32) -> usize {
    match rate {
        0..=50_000 => 512,
        50_001..=100_000 => 1024,
        _ => 2048,
    }
}

/// The impulse response's partitions as spectra, for one rate.
struct Kernel {
    /// Per channel of the impulse response, `partitions` spectra of `B + 1` bins.
    spectra: Vec<Box<[Complex<f64>]>>,
    partitions: usize,
}

/// The convolution stage for one rate and channel count.
pub struct Convolver {
    plan: ConvolutionPlan,
    channels: usize,
    block: usize,
    kernel: Kernel,
    forward: Arc<dyn RealToComplex<f64>>,
    inverse: Arc<dyn ComplexToReal<f64>>,
    wet: f64,
    dry: f64,
    /// Per channel: the previous and the current block of input, `2B` frames.
    input: Box<[f64]>,
    /// Per channel: the next `B` output frames.
    output: Box<[f64]>,
    /// Per channel: a ring of `partitions` input spectra.
    history: Box<[Complex<f64>]>,
    /// The ring slot of the newest spectrum.
    newest: usize,
    /// Frames of the current block taken so far.
    fill: usize,
    time: Box<[f64]>,
    spectrum: Box<[Complex<f64>]>,
    sum: Box<[Complex<f64>]>,
    scratch: Box<[Complex<f64>]>,
}

impl std::fmt::Debug for Convolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Convolver")
            .field("plan", &self.plan)
            .field("channels", &self.channels)
            .field("block", &self.block)
            .finish_non_exhaustive()
    }
}

impl Convolver {
    /// The stage for `impulse` with the gain and mix of `settings`, for `channels`
    /// channels at `rate` Hz (the rate of `impulse`).
    pub fn new(
        impulse: &Impulse,
        settings: &ConvolutionSettings,
        rate: u32,
        channels: usize,
    ) -> Self {
        Self::with_partition(impulse, settings, rate, channels, partition_for(rate))
    }

    /// [`new`](Self::new) with partitions of `block` frames.
    pub fn with_partition(
        impulse: &Impulse,
        settings: &ConvolutionSettings,
        rate: u32,
        channels: usize,
        block: usize,
    ) -> Self {
        let channels = channels.max(1);
        let block = block.max(16);
        let size = 2 * block;
        let bins = block + 1;
        let mut planner = RealFftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        let gain_db = if settings.gain_db.is_finite() {
            settings.gain_db
        } else {
            0.0
        };
        let mut scale = 10f64.powf(gain_db / 20.0) / size as f64;
        if settings.normalize {
            let peak = impulse.peak_gain();
            if peak > 1e-12 {
                scale /= peak;
            }
        }
        let taps = impulse.taps().max(1);
        let partitions = taps.div_ceil(block);
        let mut scratch = forward.make_scratch_vec();
        let inverse_scratch = inverse.get_scratch_len();
        if inverse_scratch > scratch.len() {
            scratch.resize(inverse_scratch, Complex::default());
        }
        let mut time = vec![0.0; size];
        let mut spectrum = vec![Complex::default(); bins];
        let mut spectra = Vec::with_capacity(impulse.channels.len().max(1));
        for channel in &impulse.channels {
            let mut parts = vec![Complex::default(); partitions * bins];
            for (p, part) in parts.chunks_exact_mut(bins).enumerate() {
                time.fill(0.0);
                let start = p * block;
                let end = (start + block).min(channel.len());
                if start < end {
                    for (t, x) in time[..end - start].iter_mut().zip(&channel[start..end]) {
                        *t = x * scale;
                    }
                }
                // The lengths are right by construction.
                let _ = forward.process_with_scratch(&mut time, &mut spectrum, &mut scratch);
                part.copy_from_slice(&spectrum);
            }
            spectra.push(parts.into_boxed_slice());
        }
        if spectra.is_empty() {
            spectra.push(vec![Complex::default(); partitions * bins].into_boxed_slice());
        }
        let wet_percent = if settings.wet_percent.is_finite() {
            settings.wet_percent.clamp(0.0, 100.0)
        } else {
            100.0
        };
        let wet = wet_percent / 100.0;
        Self {
            plan: ConvolutionPlan {
                taps,
                ir_channels: impulse.channels.len().min(255) as u8,
                file_rate: impulse.file_rate,
                resampled: impulse.resampled(),
                latency_frames: block,
                latency_ms: block as f64 * 1000.0 / f64::from(rate.max(1)),
                wet_percent,
                gain_db,
                normalized: settings.normalize,
            },
            channels,
            block,
            kernel: Kernel {
                spectra,
                partitions,
            },
            forward,
            inverse,
            wet,
            dry: 1.0 - wet,
            input: vec![0.0; channels * size].into_boxed_slice(),
            output: vec![0.0; channels * block].into_boxed_slice(),
            history: vec![Complex::default(); channels * partitions * bins].into_boxed_slice(),
            newest: 0,
            fill: 0,
            time: time.into_boxed_slice(),
            spectrum: spectrum.into_boxed_slice(),
            sum: vec![Complex::default(); bins].into_boxed_slice(),
            scratch: scratch.into_boxed_slice(),
        }
    }

    /// What the signal path shows.
    pub fn plan(&self) -> ConvolutionPlan {
        self.plan
    }

    /// Convolves the block just completed and makes the next `B` output frames.
    fn run_block(&mut self) {
        let block = self.block;
        let size = 2 * block;
        let bins = block + 1;
        let partitions = self.kernel.partitions;
        self.newest = (self.newest + 1) % partitions;
        for c in 0..self.channels {
            let input = &mut self.input[c * size..(c + 1) * size];
            self.time.copy_from_slice(input);
            // The current block becomes the previous one.
            input.copy_within(block.., 0);
            let _ = self.forward.process_with_scratch(
                &mut self.time,
                &mut self.spectrum,
                &mut self.scratch,
            );
            let ring = &mut self.history[c * partitions * bins..(c + 1) * partitions * bins];
            ring[self.newest * bins..(self.newest + 1) * bins].copy_from_slice(&self.spectrum);
            let kernel = &self.kernel.spectra[c % self.kernel.spectra.len()];
            self.sum.fill(Complex::default());
            for p in 0..partitions {
                let slot = (self.newest + partitions - p) % partitions;
                let x = &ring[slot * bins..(slot + 1) * bins];
                let h = &kernel[p * bins..(p + 1) * bins];
                for ((s, x), h) in self.sum.iter_mut().zip(x).zip(h) {
                    *s += x * h;
                }
            }
            self.sum[0].im = 0.0;
            self.sum[block].im = 0.0;
            let _ =
                self.inverse
                    .process_with_scratch(&mut self.sum, &mut self.time, &mut self.scratch);
            let output = &mut self.output[c * block..(c + 1) * block];
            for (o, t) in output.iter_mut().zip(&self.time[block..]) {
                *o = if t.abs() < 1e-30 { 0.0 } else { *t };
            }
        }
    }
}

impl DspStage for Convolver {
    fn name(&self) -> &'static str {
        "convolution"
    }

    fn latency(&self) -> usize {
        self.block
    }

    fn process(&mut self, block: &mut [f32]) {
        let channels = self.channels;
        let size = 2 * self.block;
        for frame in block.chunks_exact_mut(channels) {
            let at = self.fill;
            for (c, sample) in frame.iter_mut().enumerate() {
                let input = &mut self.input[c * size..(c + 1) * size];
                // The previous block holds the input of `B` frames ago.
                let dry = input[at];
                input[self.block + at] = f64::from(*sample);
                let wet = self.output[c * self.block + at];
                *sample = (wet * self.wet + dry * self.dry) as f32;
            }
            self.fill += 1;
            if self.fill == self.block {
                self.fill = 0;
                self.run_block();
            }
        }
    }

    fn reset(&mut self) {
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.history.fill(Complex::default());
        self.newest = 0;
        self.fill = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn impulse(channels: Vec<Vec<f64>>) -> Impulse {
        Impulse {
            path: PathBuf::new(),
            file_rate: 48_000,
            rate: 48_000,
            channels,
        }
    }

    fn plain() -> ConvolutionSettings {
        ConvolutionSettings {
            enabled: true,
            normalize: false,
            ..ConvolutionSettings::default()
        }
    }

    /// The direct convolution of `x` with `h`.
    fn direct(x: &[f64], h: &[f64]) -> Vec<f64> {
        (0..x.len())
            .map(|n| {
                (0..h.len())
                    .filter(|k| *k <= n)
                    .map(|k| h[k] * x[n - k])
                    .sum()
            })
            .collect()
    }

    #[test]
    fn it_matches_direct_convolution_after_its_latency() {
        let h: Vec<f64> = (0..300)
            .map(|i| ((i as f64) * 0.13).sin() * 0.97f64.powi(i))
            .collect();
        let mut stage =
            Convolver::with_partition(&impulse(vec![h.clone()]), &plain(), 48_000, 1, 64);
        assert_eq!(stage.latency(), 64);
        assert_eq!(stage.plan().taps, 300);
        let x: Vec<f64> = (0..2000)
            .map(|i| ((i * 7919) % 101) as f64 / 101.0 - 0.5)
            .collect();
        let mut block: Vec<f32> = x.iter().map(|v| *v as f32).collect();
        // Odd block lengths, to cross the partitions anywhere.
        for chunk in block.chunks_mut(37) {
            stage.process(chunk);
        }
        let x32: Vec<f64> = x.iter().map(|v| f64::from(*v as f32)).collect();
        let expected = direct(&x32, &h);
        for n in 64..2000 {
            let got = f64::from(block[n]);
            assert!(
                (got - expected[n - 64]).abs() < 1e-5,
                "{n}: {got} vs {}",
                expected[n - 64]
            );
        }
        assert!(block[..64].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn an_impulse_gives_the_response_and_stereo_responses_stay_apart() {
        let left = vec![1.0, 0.5, 0.25];
        let right = vec![0.0, -1.0];
        let mut stage =
            Convolver::with_partition(&impulse(vec![left, right]), &plain(), 48_000, 2, 16);
        let mut block = vec![0.0f32; 2 * 64];
        block[0] = 1.0;
        block[1] = 1.0;
        stage.process(&mut block);
        let l: Vec<f32> = block.iter().step_by(2).copied().collect();
        let r: Vec<f32> = block.iter().skip(1).step_by(2).copied().collect();
        assert!((l[16] - 1.0).abs() < 1e-6 && (l[17] - 0.5).abs() < 1e-6);
        assert!((l[18] - 0.25).abs() < 1e-6 && l[19].abs() < 1e-6);
        assert!(r[16].abs() < 1e-6 && (r[17] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalisation_and_the_dry_share() {
        // A response that boosts by 6 dB everywhere is brought back to unity.
        let settings = ConvolutionSettings {
            normalize: true,
            ..plain()
        };
        let mut stage =
            Convolver::with_partition(&impulse(vec![vec![2.0]]), &settings, 48_000, 1, 32);
        let mut block = vec![0.5f32; 256];
        stage.process(&mut block);
        assert!((block[200] - 0.5).abs() < 1e-6);
        // Half wet, with a response that inverts: wet and dry cancel, delayed alike.
        let settings = ConvolutionSettings {
            wet_percent: 50.0,
            ..plain()
        };
        let mut stage =
            Convolver::with_partition(&impulse(vec![vec![-1.0]]), &settings, 48_000, 1, 32);
        let mut block: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin()).collect();
        stage.process(&mut block);
        assert!(block.iter().all(|s| s.abs() < 1e-6));
        // The gain applies to the processed signal.
        let settings = ConvolutionSettings {
            gain_db: -6.0206,
            ..plain()
        };
        let mut stage =
            Convolver::with_partition(&impulse(vec![vec![1.0]]), &settings, 48_000, 1, 32);
        let mut block = vec![1.0f32; 128];
        stage.process(&mut block);
        assert!((block[100] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn silence_after_sound_stays_clean() {
        let h: Vec<f64> = (0..1000).map(|i| 0.99f64.powi(i)).collect();
        let mut stage = Convolver::with_partition(&impulse(vec![h]), &plain(), 48_000, 2, 128);
        let mut block = vec![0.5f32; 2 * 1024];
        stage.process(&mut block);
        let mut quiet = vec![0.0f32; 2 * 48_000];
        stage.process(&mut quiet);
        assert!(quiet[2 * 47_000..].iter().all(|s| *s == 0.0));
        assert!(quiet.iter().all(|s| s.is_finite()));
        stage.reset();
        let mut again = vec![0.0f32; 2 * 512];
        stage.process(&mut again);
        assert!(again.iter().all(|s| *s == 0.0));
    }
}
