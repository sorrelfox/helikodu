// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decimation of the 1-bit DSD stream to PCM with one linear-phase FIR filter.
//!
//! The filter is a Kaiser-windowed sinc low-pass (cutoff 40 kHz, β = 8, about
//! 80 dB stop-band attenuation) of `32 × factor` taps, so its length in time is the
//! same at every DSD rate. With 512 taps at DSD64 the pass band is flat to above
//! 20 kHz and the transition band ends near 55 kHz, well below the 88.2 kHz Nyquist
//! frequency of the output, so the steep ultrasonic noise of the modulator does not
//! alias into the audio band.
//!
//! Every input bit is ±1, so the filter is evaluated a byte at a time: a table holds,
//! for each byte position in the filter and each of the 256 byte values, the sum of
//! the eight taps with their signs. One output sample costs `taps / 8` table look-ups.

use super::error::DsdError;

/// The gain applied in the conversion, +6 dB: the DSD 0 dB reference (50 %
/// modulation) corresponds to −6 dBFS in PCM.
pub const DSD_GAIN: f64 = 2.0;

/// Filter taps per bit of decimation.
const TAPS_PER_FACTOR: usize = 32;
/// The −6 dB point of the low-pass filter.
const CUTOFF_HZ: f64 = 40_000.0;
/// The Kaiser window's shape parameter.
const KAISER_BETA: f64 = 8.0;
/// The highest PCM rate produced; the factor grows until the rate is at most this.
const MAX_OUTPUT_RATE: u32 = 192_000;
/// The smallest and largest decimation factors.
const MIN_FACTOR: u32 = 16;
const MAX_FACTOR: u32 = 1024;

/// The decimation filter for one DSD rate.
#[derive(Debug)]
pub(super) struct Decimator {
    /// DSD bits per PCM frame.
    factor: u32,
    /// The PCM rate.
    output_rate: u32,
    /// The filter length in bytes.
    window: usize,
    /// DSD bytes per PCM frame.
    step: usize,
    /// For each byte of the filter, the contribution of each of the 256 byte values.
    table: Vec<[f64; 256]>,
}

impl Decimator {
    pub(super) fn new(dsd_rate: u32) -> Result<Self, DsdError> {
        let factor = std::iter::successors(Some(MIN_FACTOR), |f| f.checked_mul(2))
            .take_while(|&f| f <= MAX_FACTOR)
            .find(|&f| {
                dsd_rate.is_multiple_of(f) && (1..=MAX_OUTPUT_RATE).contains(&(dsd_rate / f))
            })
            .ok_or_else(|| DsdError::unsupported(format!("DSD rate {dsd_rate} Hz")))?;
        let output_rate = dsd_rate / factor;
        // Both are powers of two of moderate size.
        let factor_bits = factor as usize;
        let taps = TAPS_PER_FACTOR * factor_bits;
        let coefficients = low_pass(taps, CUTOFF_HZ / f64::from(dsd_rate));
        let window = taps / 8;
        let table = coefficients
            .as_chunks::<8>()
            .0
            .iter()
            .map(|taps| {
                std::array::from_fn(|value| {
                    // The earliest bit is the most significant one.
                    taps.iter()
                        .enumerate()
                        .map(|(bit, &h)| if value & (0x80 >> bit) != 0 { h } else { -h })
                        .sum()
                })
            })
            .collect();
        Ok(Self {
            factor,
            output_rate,
            window,
            step: factor_bits / 8,
            table,
        })
    }

    /// The PCM rate.
    pub(super) fn output_rate(&self) -> u32 {
        self.output_rate
    }

    /// DSD bits per PCM frame.
    pub(super) fn factor(&self) -> u32 {
        self.factor
    }

    /// The filter length in bytes: the window one output sample reads.
    pub(super) fn window(&self) -> usize {
        self.window
    }

    /// DSD bytes per PCM frame.
    pub(super) fn step(&self) -> usize {
        self.step
    }

    /// The index of the first byte of the window of PCM frame `frame`. The filter is
    /// centred on the frame's first DSD bit (`frame × factor`), the instant the PCM
    /// frame stands for, so the output is not delayed. Negative before the start of
    /// the stream.
    pub(super) fn window_start(&self, frame: u64) -> i64 {
        let frame = i64::try_from(frame).unwrap_or(i64::MAX / 2);
        // Small values: the window and step are at most a few hundred bytes.
        let step = self.step as i64;
        let window = self.window as i64;
        frame.saturating_mul(step) - window / 2
    }

    /// One output sample from `bytes`, which holds at least a window of bytes.
    pub(super) fn sample(&self, bytes: &[u8]) -> f64 {
        bytes[..self.window]
            .iter()
            .zip(&self.table)
            .map(|(&b, row)| row[usize::from(b)])
            .sum()
    }
}

/// A Kaiser-windowed sinc low-pass of `taps` coefficients with its −6 dB point at
/// `cutoff` (a fraction of the sample rate), scaled to a DC gain of [`DSD_GAIN`].
fn low_pass(taps: usize, cutoff: f64) -> Vec<f64> {
    let centre = (taps - 1) as f64 / 2.0;
    let i0_beta = bessel_i0(KAISER_BETA);
    let mut h: Vec<f64> = (0..taps)
        .map(|j| {
            let t = j as f64 - centre;
            let x = 2.0 * cutoff * t;
            let sinc = if x == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            let r = t / centre;
            let window = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / i0_beta;
            sinc * window
        })
        .collect();
    let sum: f64 = h.iter().sum();
    for x in &mut h {
        *x *= DSD_GAIN / sum;
    }
    h
}

/// The modified Bessel function of the first kind, order 0, by its power series.
fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..64 {
        let k = f64::from(k);
        term *= quarter / (k * k);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factors_and_rates() {
        let d = Decimator::new(2_822_400).unwrap();
        assert_eq!((d.factor(), d.output_rate()), (16, 176_400));
        assert_eq!((d.window(), d.step()), (64, 2));
        let d = Decimator::new(5_644_800).unwrap();
        assert_eq!((d.factor(), d.output_rate()), (32, 176_400));
        let d = Decimator::new(11_289_600).unwrap();
        assert_eq!((d.factor(), d.output_rate()), (64, 176_400));
        let d = Decimator::new(3_072_000).unwrap();
        assert_eq!((d.factor(), d.output_rate()), (16, 192_000));
        assert!(Decimator::new(1_000_001).is_err());
        assert!(Decimator::new(0).is_err());
    }

    #[test]
    fn window_is_centred_on_the_frame() {
        let d = Decimator::new(2_822_400).unwrap();
        // Bytes -32 .. 32 for frame 0: 256 bits on either side of bit 0.
        assert_eq!(d.window_start(0), -32);
        assert_eq!(d.window_start(10), -12);
    }

    #[test]
    fn constant_input_gives_the_dc_gain() {
        let d = Decimator::new(2_822_400).unwrap();
        let ones = vec![0xffu8; d.window()];
        assert!((d.sample(&ones) - DSD_GAIN).abs() < 1e-9);
        let idle = vec![super::super::IDLE_BYTE; d.window()];
        assert!(d.sample(&idle).abs() < 0.01);
    }

    /// The filter's gain at `frequency`, in dB relative to its DC gain.
    fn response_db(h: &[f64], rate: f64, frequency: f64) -> f64 {
        let w = std::f64::consts::TAU * frequency / rate;
        let (re, im) = h.iter().enumerate().fold((0.0, 0.0), |(re, im), (n, x)| {
            let a = w * n as f64;
            (re + x * a.cos(), im - x * a.sin())
        });
        20.0 * (re.hypot(im) / DSD_GAIN).log10()
    }

    #[test]
    fn filter_is_flat_in_the_audio_band_and_stops_before_nyquist() {
        let rate = 2_822_400.0;
        let h = low_pass(512, CUTOFF_HZ / rate);
        for f in [100.0, 1000.0, 10_000.0, 20_000.0] {
            let db = response_db(&h, rate, f);
            assert!(db.abs() < 0.1, "{f} Hz: {db} dB");
        }
        // Everything from the output's Nyquist frequency up.
        for f in [
            60_000.0, 88_200.0, 100_000.0, 152_400.0, 176_400.0, 500_000.0,
        ] {
            let db = response_db(&h, rate, f);
            assert!(db < -70.0, "{f} Hz: {db} dB");
        }
    }

    #[test]
    fn bessel() {
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-15);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_4).abs() < 1e-12);
    }
}
