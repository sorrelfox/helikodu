// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The source signals every encoder starts from, written as WAV files.
//!
//! The signal is computed with basic IEEE-754 arithmetic only (no `sin`,
//! `exp` or other library functions, whose last bit may differ between C
//! libraries), so the sources are bit-identical on every machine.

use super::recipe::{Encoding, Signal, Source};

/// Peak level of the sweep: -12 dBFS.
const SWEEP_LEVEL: f64 = 0.25;
/// Peak level of the noise: -40 dBFS.
const NOISE_LEVEL: f64 = 0.01;
const SWEEP_START_HZ: f64 = 20.0;
const SWEEP_END_HZ: f64 = 20_000.0;

/// The interleaved samples of a source, in the range [-1, 1].
pub(crate) fn render(source: &Source) -> Vec<f64> {
    let channels = usize::from(source.channels);
    let frames = source.frames;
    let mut samples = Vec::with_capacity(usize::try_from(frames).unwrap_or(0) * channels);
    let mut noise = XorShift::new(source.seed);
    let rate = f64::from(source.sample_rate);
    // The sweep ends at the end of the source or at 0.45 of the sample rate,
    // whichever is lower, so it never aliases.
    let end_hz = SWEEP_END_HZ.min(rate * 0.45);
    let duration = to_f64(frames) / rate;
    for frame in 0..frames {
        for channel in 0..channels {
            let value = match source.signal {
                Signal::Silence => 0.0,
                Signal::SweepNoise => {
                    // Each channel starts the sweep at a different point, so a
                    // swapped or dropped channel is visible in the output.
                    let offset = duration * to_f64(channel as u64) / to_f64(channels as u64);
                    let mut time = to_f64(frame) / rate + offset;
                    if time >= duration {
                        time -= duration;
                    }
                    // Linear sweep: phase (in turns) = f0·t + (f1 − f0)·t² / (2T).
                    let turns = SWEEP_START_HZ * time
                        + (end_hz - SWEEP_START_HZ) * time * time / (2.0 * duration);
                    SWEEP_LEVEL * sin_turns(turns) + NOISE_LEVEL * noise.next_signed()
                }
            };
            samples.push(value);
        }
    }
    samples
}

/// The bytes of a WAV file holding `samples` (interleaved) for `source`.
///
/// Files with more than two channels or more than 16 bits use
/// `WAVE_FORMAT_EXTENSIBLE`, as Microsoft's specification asks.
pub(crate) fn wav(source: &Source, samples: &[f64]) -> Vec<u8> {
    let channels = source.channels;
    let bytes_per_sample: u16 = match source.encoding {
        Encoding::S16 => 2,
        Encoding::S24 => 3,
        Encoding::S32 | Encoding::F32 => 4,
    };
    let block_align = channels * bytes_per_sample;
    let data_len = u32::try_from(samples.len() * usize::from(bytes_per_sample)).unwrap_or(u32::MAX);
    let extensible = channels > 2 || bytes_per_sample > 2;
    let fmt_len: u32 = if extensible { 40 } else { 16 };

    let mut out = Vec::with_capacity(data_len as usize + 80);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(4 + 8 + fmt_len + 8 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&fmt_len.to_le_bytes());
    let float = matches!(source.encoding, Encoding::F32);
    let tag: u16 = if extensible {
        0xFFFE
    } else if float {
        3
    } else {
        1
    };
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&source.sample_rate.to_le_bytes());
    out.extend_from_slice(&(source.sample_rate * u32::from(block_align)).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&(bytes_per_sample * 8).to_le_bytes());
    if extensible {
        out.extend_from_slice(&22u16.to_le_bytes());
        out.extend_from_slice(&(bytes_per_sample * 8).to_le_bytes());
        out.extend_from_slice(&channel_mask(channels).to_le_bytes());
        // KSDATAFORMAT_SUBTYPE_PCM or _IEEE_FLOAT.
        out.extend_from_slice(&(if float { 3u32 } else { 1u32 }).to_le_bytes());
        out.extend_from_slice(&[
            0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
        ]);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &value in samples {
        let value = value.clamp(-1.0, 1.0);
        match source.encoding {
            Encoding::S16 => out.extend_from_slice(&quantize(value, 32_767.0).to_le_bytes()[..2]),
            Encoding::S24 => {
                out.extend_from_slice(&quantize(value, 8_388_607.0).to_le_bytes()[..3])
            }
            Encoding::S32 => {
                out.extend_from_slice(&quantize(value, 2_147_483_647.0).to_le_bytes()[..4]);
            }
            Encoding::F32 => out.extend_from_slice(&(value as f32).to_le_bytes()),
        }
    }
    out
}

/// The standard WAV speaker mask for the first `channels` speakers
/// (5.1 is front left, front right, centre, LFE, back left, back right).
fn channel_mask(channels: u16) -> u32 {
    match channels {
        1 => 0x4,
        2 => 0x3,
        6 => 0x3F,
        8 => 0x63F,
        n => (1u32 << n.min(18)) - 1,
    }
}

fn quantize(value: f64, scale: f64) -> i32 {
    (value * scale).round() as i32
}

fn to_f64(value: u64) -> f64 {
    value as f64
}

/// sin(2π·turns), from a polynomial, accurate to about 1e-9.
fn sin_turns(turns: f64) -> f64 {
    // Reduce to [-0.5, 0.5) turns, then to [-0.25, 0.25] using sin(π − x) = sin(x).
    let mut x = turns - turns.floor();
    if x >= 0.5 {
        x -= 1.0;
    }
    if x > 0.25 {
        x = 0.5 - x;
    } else if x < -0.25 {
        x = -0.5 - x;
    }
    let r = x * std::f64::consts::TAU;
    let r2 = r * r;
    // Taylor series up to r^15; |r| ≤ π/2 keeps the error below 1e-9.
    let mut term = r;
    let mut sum = r;
    for k in 1..8u32 {
        let k = f64::from(k);
        term = -term * r2 / ((2.0 * k) * (2.0 * k + 1.0));
        sum += term;
    }
    sum
}

/// A small fixed-seed generator (xorshift64*), the same on every machine.
#[derive(Debug)]
struct XorShift(u64);

impl XorShift {
    fn new(seed: u64) -> Self {
        // A zero state would stay zero forever.
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [-1, 1).
    fn next_signed(&mut self) -> f64 {
        to_f64(self.next_u64() >> 11) / to_f64(1 << 52) - 1.0
    }
}

/// `length` bytes of fixed-seed noise, for broken files.
pub(crate) fn noise_bytes(seed: u64, length: usize) -> Vec<u8> {
    let mut generator = XorShift::new(seed);
    let mut out = Vec::with_capacity(length + 8);
    while out.len() < length {
        out.extend_from_slice(&generator.next_u64().to_le_bytes());
    }
    out.truncate(length);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_matches_library() {
        for step in -1000..1000 {
            let turns = f64::from(step) / 337.0;
            let expected = (turns * std::f64::consts::TAU).sin();
            assert!((sin_turns(turns) - expected).abs() < 1e-9, "{turns}");
        }
    }

    #[test]
    fn noise_is_in_range() {
        let mut generator = XorShift::new(1);
        for _ in 0..10_000 {
            let value = generator.next_signed();
            assert!((-1.0..1.0).contains(&value));
        }
    }
}
