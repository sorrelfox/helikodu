// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! TPDF dither for integer output.
//!
//! When processed audio is reduced to 8, 16 or 24 bits, plain rounding leaves an
//! error that follows the signal: distortion, most audible on quiet fades. Adding
//! noise with a triangular distribution of ±1 LSB (the sum of two independent
//! uniform values) before rounding makes the error independent of the signal, so
//! only a constant, faint hiss remains.
//!
//! The stage quantizes itself: its output samples are exact multiples of the target
//! LSB, which the final conversion ([`Encoder`](super::Encoder)) turns into the same
//! integers without further rounding.

use helikodu_model::audio::SampleFormat;

/// TPDF dither and quantization to a signed integer of a given width.
#[derive(Debug, Clone)]
pub struct TpdfDither {
    bits: u32,
    scale: f64,
    rng: Xorshift,
}

impl TpdfDither {
    /// The default seed, so that output is reproducible.
    pub const DEFAULT_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

    /// Dither to `bits` bits (2 to 24), or `None` for other widths: wider integers
    /// carry more precision than the 32-bit float samples have.
    pub fn new(bits: u32, seed: u64) -> Option<Self> {
        (2..=24).contains(&bits).then(|| Self {
            bits,
            scale: f64::from(1u32 << (bits - 1)),
            rng: Xorshift::new(seed),
        })
    }

    /// Dither for output in `format`, or `None` if the format does not need it: 32-bit
    /// integers, floats and DSD.
    pub fn for_format(format: SampleFormat) -> Option<Self> {
        let bits = match format {
            SampleFormat::U8 => 8,
            SampleFormat::S16(_) => 16,
            SampleFormat::S24(_) | SampleFormat::S24Packed(_) => 24,
            _ => return None,
        };
        Self::new(bits, Self::DEFAULT_SEED)
    }

    /// The target width in bits.
    pub fn bits(&self) -> u32 {
        self.bits
    }

    /// One value of triangular noise, in LSB: between −1 and 1, mean 0, variance 1/6.
    pub(crate) fn noise(&mut self) -> f64 {
        self.rng.uniform() - self.rng.uniform()
    }

    /// Dithers and quantizes every sample of `block`. The results are multiples of
    /// the target LSB, clipped to the integer range.
    pub fn apply(&mut self, block: &mut [f32]) {
        let (low, high) = (-self.scale, self.scale - 1.0);
        for sample in block {
            let x = f64::from(*sample);
            let value = if x.is_nan() {
                0.0
            } else {
                (x * self.scale + self.noise()).round().clamp(low, high)
            };
            *sample = (value / self.scale) as f32;
        }
    }
}

/// A small, fast generator (xorshift64*): good enough for noise, and it never
/// allocates or locks.
#[derive(Debug, Clone)]
struct Xorshift(u64);

impl Xorshift {
    fn new(seed: u64) -> Self {
        // The state must not be zero.
        Self(if seed == 0 {
            0x2545_F491_4F6C_DD1D
        } else {
            seed
        })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A uniform value in [0, 1).
    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helikodu_model::audio::Endianness;

    #[test]
    fn noise_is_triangular_with_zero_mean() {
        let mut dither = TpdfDither::new(16, 1).unwrap();
        let n = 400_000;
        let samples: Vec<f64> = (0..n).map(|_| dither.noise()).collect();
        let mean = samples.iter().sum::<f64>() / n as f64;
        let variance = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
        assert!(mean.abs() < 0.005, "{mean}");
        assert!((variance - 1.0 / 6.0).abs() < 0.005, "{variance}");
        assert!(samples.iter().all(|x| x.abs() < 1.0));
        // A triangle on (−1, 1): density 1 − |x|. Compare a histogram of ten bins.
        let mut bins = [0usize; 10];
        for x in &samples {
            bins[((x + 1.0) * 5.0) as usize] += 1;
        }
        for (i, &count) in bins.iter().enumerate() {
            let (a, b) = (-1.0 + i as f64 * 0.2, -1.0 + (i + 1) as f64 * 0.2);
            let cdf = |x: f64| {
                if x < 0.0 {
                    (1.0 + x).powi(2) / 2.0
                } else {
                    1.0 - (1.0 - x).powi(2) / 2.0
                }
            };
            let expected = (cdf(b) - cdf(a)) * n as f64;
            assert!(
                (count as f64 - expected).abs() < expected * 0.03 + 50.0,
                "bin {i}: {count} vs {expected}"
            );
        }
    }

    #[test]
    fn output_is_quantized_and_unbiased() {
        let mut dither = TpdfDither::for_format(SampleFormat::S16(Endianness::Little)).unwrap();
        assert_eq!(dither.bits(), 16);
        // A constant of 0.3 LSB: plain rounding would give 0 every time.
        let lsb = 1.0 / 32768.0;
        let mut block = vec![(0.3 * lsb) as f32; 100_000];
        dither.apply(&mut block);
        let mut sum = 0.0;
        for &s in &block {
            let v = f64::from(s) * 32768.0;
            assert_eq!(v, v.round());
            assert!((-1.0..=2.0).contains(&v));
            sum += v;
        }
        let mean = sum / block.len() as f64;
        assert!((mean - 0.3).abs() < 0.01, "{mean}");
    }

    #[test]
    fn silence_becomes_at_most_one_lsb_of_noise() {
        let mut dither = TpdfDither::new(24, 7).unwrap();
        let mut block = vec![0.0f32; 10_000];
        dither.apply(&mut block);
        let lsb = 1.0 / 8_388_608.0;
        assert!(block.iter().all(|&s| f64::from(s).abs() <= lsb));
        let zeros = block.iter().filter(|&&s| s == 0.0).count();
        // P(|noise| < 0.5) = 0.75.
        assert!((zeros as f64 / 10_000.0 - 0.75).abs() < 0.02);
    }

    #[test]
    fn full_scale_and_nan_stay_in_range() {
        let mut dither = TpdfDither::new(16, 3).unwrap();
        let mut block = [1.0f32, -1.0, 2.0, f32::NAN];
        dither.apply(&mut block);
        assert!(block[0] <= 32767.0 / 32768.0);
        assert!(block[1] >= -1.0);
        assert_eq!(block[2], 32767.0 / 32768.0);
        assert_eq!(block[3], 0.0);
    }

    #[test]
    fn wide_and_float_formats_get_no_dither() {
        assert!(TpdfDither::for_format(SampleFormat::S32(Endianness::Little)).is_none());
        assert!(TpdfDither::for_format(SampleFormat::F32(Endianness::Little)).is_none());
        assert!(TpdfDither::new(32, 1).is_none());
    }
}
