// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Conversion at the edges of the pipeline: decoded samples to `f32`, and `f32` to
//! the device's sample format and channel order.
//!
//! Integers become floats as `n / 2^(bits-1)`, which is exact for up to 24
//! significant bits. On the way out, integer formats are rounded to the nearest value
//! and clipped; a sample that came from an integer of the same or fewer bits converts
//! back to exactly that integer, so 16- and 24-bit audio passes unchanged. Dither is a
//! stage of its own ([`TpdfDither`](super::TpdfDither)) that runs before the encoder.
//!
//! Both directions are real-time safe: no allocation after construction.

use helikodu_model::audio::{Endianness, SampleFormat};

/// A decoded sample type that converts to `f32`.
pub trait PcmSample: Copy {
    /// The sample as a float, nominally between -1.0 and 1.0.
    fn to_f32(self) -> f32;
}

impl PcmSample for u8 {
    fn to_f32(self) -> f32 {
        (f32::from(self) - 128.0) / 128.0
    }
}

impl PcmSample for i16 {
    fn to_f32(self) -> f32 {
        f32::from(self) / 32_768.0
    }
}

impl PcmSample for i32 {
    /// Exact for samples with up to 24 significant bits in the top of the word, as
    /// decoders deliver 24-bit audio.
    fn to_f32(self) -> f32 {
        (f64::from(self) / 2_147_483_648.0) as f32
    }
}

impl PcmSample for f32 {
    fn to_f32(self) -> f32 {
        self
    }
}

impl PcmSample for f64 {
    fn to_f32(self) -> f32 {
        self as f32
    }
}

/// Converts decoded samples to `f32`, as many as both slices hold, and returns the
/// number converted.
pub fn to_float<S: PcmSample>(input: &[S], output: &mut [f32]) -> usize {
    let n = input.len().min(output.len());
    for (dst, src) in output[..n].iter_mut().zip(&input[..n]) {
        *dst = src.to_f32();
    }
    n
}

/// Converts interleaved `f32` frames to the bytes of one device format: the final
/// conversion of the pipeline, run by the output backends.
#[derive(Debug, Clone)]
pub struct Encoder {
    format: SampleFormat,
    channels: usize,
    /// For each device channel, the index of the source channel it takes.
    /// `None` when the orders are the same.
    map: Option<Box<[usize]>>,
}

impl Encoder {
    /// An encoder for `channels` channels in `format`, or `None` if the format is
    /// not PCM (DSD needs the raw 1-bit stream).
    pub fn new(format: SampleFormat, channels: usize) -> Option<Self> {
        (!format.is_dsd() && channels > 0).then_some(Self {
            format,
            channels,
            map: None,
        })
    }

    /// Reorders channels on the way out: device channel `i` takes source channel
    /// `map[i]`. Ignored unless `map` is a permutation of the channels.
    pub fn with_channel_map(mut self, map: Vec<usize>) -> Self {
        let mut seen = vec![false; self.channels];
        let valid = map.len() == self.channels
            && map
                .iter()
                .all(|&i| i < self.channels && !std::mem::replace(&mut seen[i], true));
        let identity = map.iter().enumerate().all(|(i, &j)| i == j);
        self.map = (valid && !identity).then(|| map.into_boxed_slice());
        self
    }

    /// Bytes of one encoded frame.
    pub fn bytes_per_frame(&self) -> usize {
        self.format.bytes() * self.channels
    }

    /// Encodes the whole frames of `samples` into the start of `out`, and returns
    /// the number of bytes written. Stops early if `out` is too short.
    pub fn encode(&self, samples: &[f32], out: &mut [u8]) -> usize {
        let bytes = self.format.bytes();
        let frame_bytes = self.bytes_per_frame();
        let mut written = 0;
        for (frame, dst) in samples
            .chunks_exact(self.channels)
            .zip(out.chunks_exact_mut(frame_bytes))
        {
            for (i, sample_dst) in dst.chunks_exact_mut(bytes).enumerate() {
                let source = match &self.map {
                    Some(map) => map[i],
                    None => i,
                };
                write_sample(frame[source], self.format, sample_dst);
            }
            written += frame_bytes;
        }
        written
    }
}

/// `x` scaled to a signed integer of `bits` bits, rounded and clipped. NaN is 0.
fn to_int(x: f32, bits: u32) -> i32 {
    let scale = f64::from(1u32 << (bits - 1));
    // `as` saturates and maps NaN to 0; the clamp keeps +1.0 inside the range.
    (f64::from(x) * scale).round().clamp(-scale, scale - 1.0) as i32
}

fn put<const N: usize>(dst: &mut [u8], le: [u8; N], be: [u8; N], endianness: Endianness) {
    dst.copy_from_slice(match endianness {
        Endianness::Little => &le,
        Endianness::Big => &be,
    });
}

fn write_sample(x: f32, format: SampleFormat, dst: &mut [u8]) {
    match format {
        SampleFormat::U8 => dst[0] = (to_int(x, 8) + 128) as u8,
        SampleFormat::S16(e) => {
            let v = to_int(x, 16) as i16;
            put(dst, v.to_le_bytes(), v.to_be_bytes(), e);
        }
        SampleFormat::S24(e) => {
            // The low three bytes of a 4-byte word, sign-extended.
            let v = to_int(x, 24);
            put(dst, v.to_le_bytes(), v.to_be_bytes(), e);
        }
        SampleFormat::S24Packed(e) => {
            let v = to_int(x, 24);
            let (le, be) = (v.to_le_bytes(), v.to_be_bytes());
            put(dst, [le[0], le[1], le[2]], [be[1], be[2], be[3]], e);
        }
        SampleFormat::S32(e) => {
            let v = to_int(x, 32);
            put(dst, v.to_le_bytes(), v.to_be_bytes(), e);
        }
        SampleFormat::F32(e) => put(dst, x.to_le_bytes(), x.to_be_bytes(), e),
        SampleFormat::F64(e) => {
            let v = f64::from(x);
            put(dst, v.to_le_bytes(), v.to_be_bytes(), e);
        }
        // Never constructed for DSD (see `Encoder::new`); write zeros.
        SampleFormat::DsdU8 | SampleFormat::DsdU16(_) | SampleFormat::DsdU32(_) => dst.fill(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Endianness::{Big, Little};

    fn encode(format: SampleFormat, samples: &[f32]) -> Vec<u8> {
        let encoder = Encoder::new(format, 1).unwrap();
        let mut out = vec![0xAA; samples.len() * format.bytes()];
        assert_eq!(encoder.encode(samples, &mut out), out.len());
        out
    }

    #[test]
    fn integers_round_trip_exactly() {
        let s16: Vec<f32> = [-32768, -1, 0, 1, 12345, 32767]
            .iter()
            .map(|&v| v as f32 / 32768.0)
            .collect();
        assert_eq!(
            encode(SampleFormat::S16(Little), &s16),
            [0x00, 0x80, 0xFF, 0xFF, 0, 0, 1, 0, 0x39, 0x30, 0xFF, 0x7F]
        );
        let s24: Vec<f32> = [-8_388_608, -2, 0x12_3456, 8_388_607]
            .iter()
            .map(|&v| v as f32 / 8_388_608.0)
            .collect();
        assert_eq!(
            encode(SampleFormat::S24Packed(Little), &s24),
            [
                0, 0, 0x80, 0xFE, 0xFF, 0xFF, 0x56, 0x34, 0x12, 0xFF, 0xFF, 0x7F
            ]
        );
        assert_eq!(
            encode(SampleFormat::S24Packed(Big), &s24[2..3]),
            [0x12, 0x34, 0x56]
        );
        assert_eq!(
            encode(SampleFormat::S24(Little), &s24[..2]),
            [0, 0, 0x80, 0xFF, 0xFE, 0xFF, 0xFF, 0xFF]
        );
    }

    #[test]
    fn clips_and_maps_nan_to_zero() {
        assert_eq!(
            encode(SampleFormat::S16(Big), &[1.0, 2.0, -3.0, f32::NAN]),
            [0x7F, 0xFF, 0x7F, 0xFF, 0x80, 0x00, 0, 0]
        );
        assert_eq!(
            encode(SampleFormat::S32(Little), &[1.0, -1.0]),
            [0xFF, 0xFF, 0xFF, 0x7F, 0, 0, 0, 0x80]
        );
        assert_eq!(encode(SampleFormat::U8, &[-1.0, 0.0, 1.0]), [0, 128, 255]);
    }

    #[test]
    fn floats_are_copied() {
        let values = [0.25f32, -0.123_456_79];
        let bytes = encode(SampleFormat::F32(Little), &values);
        assert_eq!(bytes[..4], 0.25f32.to_le_bytes());
        assert_eq!(bytes[4..], values[1].to_le_bytes());
        let bytes = encode(SampleFormat::F64(Big), &values[..1]);
        assert_eq!(bytes, 0.25f64.to_be_bytes());
    }

    #[test]
    fn channel_map_reorders_frames() {
        // Canonical 5.1 (FL FR FC LFE BL BR) to ALSA's (FL FR BL BR FC LFE).
        let encoder = Encoder::new(SampleFormat::U8, 6)
            .unwrap()
            .with_channel_map(vec![0, 1, 4, 5, 2, 3]);
        let frame: Vec<f32> = (0..6).map(|i| i as f32 / 128.0).collect();
        let mut out = [0; 6];
        encoder.encode(&frame, &mut out);
        assert_eq!(out, [128, 129, 132, 133, 130, 131]);
    }

    #[test]
    fn invalid_channel_maps_are_ignored() {
        let encoder = Encoder::new(SampleFormat::U8, 2)
            .unwrap()
            .with_channel_map(vec![0, 0]);
        assert!(encoder.map.is_none());
        let encoder = Encoder::new(SampleFormat::U8, 2)
            .unwrap()
            .with_channel_map(vec![1, 0]);
        assert!(encoder.map.is_some());
    }

    #[test]
    fn short_output_stops_at_a_frame_boundary() {
        let encoder = Encoder::new(SampleFormat::S16(Little), 2).unwrap();
        let mut out = [0; 6];
        assert_eq!(encoder.encode(&[0.0; 8], &mut out), 4);
    }

    #[test]
    fn dsd_is_refused() {
        assert!(Encoder::new(SampleFormat::DsdU8, 2).is_none());
        assert!(Encoder::new(SampleFormat::F32(Little), 0).is_none());
    }

    #[test]
    fn to_float_is_exact_and_inverts_the_encoder() {
        let mut out = [0.0f32; 4];
        assert_eq!(to_float(&[0u8, 128, 255], &mut out), 3);
        assert_eq!(out[..3], [-1.0, 0.0, 127.0 / 128.0]);
        to_float(&[i16::MIN, -1, 0, i16::MAX], &mut out);
        assert_eq!(out, [-1.0, -1.0 / 32768.0, 0.0, 32767.0 / 32768.0]);
        // 24-bit samples in the top of 32-bit words.
        let s24 = [-8_388_608i32, -1, 0x12_3456, 8_388_607];
        let words: Vec<i32> = s24.iter().map(|v| v << 8).collect();
        to_float(&words, &mut out);
        let encoder = Encoder::new(SampleFormat::S24(Little), 1).unwrap();
        let mut bytes = [0u8; 16];
        encoder.encode(&out, &mut bytes);
        let back: Vec<i32> = bytes
            .chunks(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        assert_eq!(back, s24);
        assert_eq!(to_float(&[0.5f64, -0.25], &mut out), 2);
        assert_eq!(out[..2], [0.5, -0.25]);
    }
}
