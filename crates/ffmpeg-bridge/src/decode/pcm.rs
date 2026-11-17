// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoded audio as interleaved PCM, and the conversion from FFmpeg's frames.
//!
//! Integer samples keep their exact values: 8-, 16- and 32-bit integers are copied
//! as they are (24-bit sources arrive as 32-bit words with the sample in the top 24
//! bits). Floating-point samples become `f32`.

use std::ffi::c_int;
use std::ptr;

use helikodu_model::audio::{AudioSpec, ChannelLayout, Endianness, SampleFormat, SampleRate};

use crate::error::Error;
use crate::ffi::{self, channel_order, ff, sample_format as fmt};

/// Interleaved samples of one block, in one of the output formats.
#[derive(Debug, Clone, PartialEq)]
pub enum PcmSamples {
    /// Unsigned 8-bit integers, 128 is silence.
    U8(Vec<u8>),
    /// Signed 16-bit integers.
    S16(Vec<i16>),
    /// Signed 32-bit integers. Sources with fewer significant bits are in the top
    /// bits, as FFmpeg decodes them.
    S32(Vec<i32>),
    /// 32-bit floats, nominally between -1.0 and 1.0.
    F32(Vec<f32>),
}

impl PcmSamples {
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
}

/// One block of decoded audio.
#[derive(Debug, Clone, PartialEq)]
pub struct PcmBuffer {
    spec: AudioSpec,
    position: u64,
    frames: usize,
    samples: PcmSamples,
}

impl PcmBuffer {
    pub(crate) fn new(spec: AudioSpec) -> Self {
        Self {
            spec,
            position: 0,
            frames: 0,
            samples: PcmSamples::F32(Vec::new()),
        }
    }

    /// The format of the samples. Its sample format is always native-endian
    /// [`SampleFormat::U8`], `S16`, `S32` or `F32`, matching [`samples`](Self::samples).
    pub fn spec(&self) -> AudioSpec {
        self.spec
    }

    /// The index of the block's first frame in the stream, counted from the first
    /// frame of audio after the encoder delay.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// The number of frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The interleaved samples.
    pub fn samples(&self) -> &PcmSamples {
        &self.samples
    }

    /// Takes the samples out, leaving an empty buffer.
    pub fn take_samples(&mut self) -> PcmSamples {
        self.frames = 0;
        std::mem::replace(&mut self.samples, PcmSamples::F32(Vec::new()))
    }
}

/// How a frame's samples are converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Conversion {
    U8,
    S16,
    S32,
    F32,
    /// `f64` narrowed to `f32`.
    F64,
    /// `i64` reduced to its top 32 bits.
    S64,
}

impl Conversion {
    /// The conversion of an `AVSampleFormat` and whether it is planar.
    fn of(format: c_int) -> Option<(Self, bool)> {
        Some(match format {
            fmt::U8 => (Self::U8, false),
            fmt::U8P => (Self::U8, true),
            fmt::S16 => (Self::S16, false),
            fmt::S16P => (Self::S16, true),
            fmt::S32 => (Self::S32, false),
            fmt::S32P => (Self::S32, true),
            fmt::FLT => (Self::F32, false),
            fmt::FLTP => (Self::F32, true),
            fmt::DBL => (Self::F64, false),
            fmt::DBLP => (Self::F64, true),
            fmt::S64 => (Self::S64, false),
            fmt::S64P => (Self::S64, true),
            _ => return None,
        })
    }

    fn output(self) -> SampleFormat {
        match self {
            Self::U8 => SampleFormat::U8,
            Self::S16 => SampleFormat::S16(Endianness::NATIVE),
            Self::S32 | Self::S64 => SampleFormat::S32(Endianness::NATIVE),
            Self::F32 | Self::F64 => SampleFormat::F32(Endianness::NATIVE),
        }
    }
}

/// The channel layout of an `AVChannelLayout`.
///
/// # Safety
/// `layout` must point to a valid channel layout.
unsafe fn channel_layout(layout: *const ff::AVChannelLayout) -> Option<ChannelLayout> {
    // SAFETY: guaranteed by the caller; the order is read as an integer.
    let (order, count) = unsafe {
        (
            ffi::enum_field(ptr::addr_of!((*layout).order)),
            (*layout).nb_channels,
        )
    };
    let count = u16::try_from(count).ok().filter(|&c| c > 0)?;
    if order == channel_order::NATIVE {
        // SAFETY: with the native order the union holds the mask.
        let mask = unsafe { (*layout).u.mask };
        if let Some(layout) = u32::try_from(mask).ok().and_then(ChannelLayout::from_mask)
            && layout.channel_count() == count
        {
            return Some(layout);
        }
        // Positions the model does not name (wide, surround direct, ...).
        return ChannelLayout::unspecified(count);
    }
    if order == ff::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC as c_int {
        return ChannelLayout::default_for_count(count);
    }
    // Custom orders and Ambisonics: only the count is meaningful here.
    ChannelLayout::unspecified(count)
}

/// The output spec for audio of `format`, `rate` and `layout`.
///
/// # Safety
/// `layout` must point to a valid channel layout.
pub(crate) unsafe fn spec_of(
    format: c_int,
    rate: c_int,
    layout: *const ff::AVChannelLayout,
) -> Result<AudioSpec, Error> {
    let (conversion, _) =
        Conversion::of(format).ok_or(Error::UnsupportedSampleFormat { format })?;
    let rate =
        u32::try_from(rate)
            .ok()
            .and_then(SampleRate::new)
            .ok_or(Error::InvalidAudioFormat {
                reason: "no sample rate",
            })?;
    // SAFETY: guaranteed by the caller.
    let layout = unsafe { channel_layout(layout) }.ok_or(Error::InvalidAudioFormat {
        reason: "no channels",
    })?;
    Ok(AudioSpec::new(conversion.output(), rate, layout))
}

/// Copies frames `skip .. skip + take` of `frame` into `out`, interleaved, and sets
/// its spec and position.
///
/// # Safety
/// `frame` must be a decoded audio frame, and `skip + take` at most its
/// `nb_samples`.
pub(crate) unsafe fn convert_frame(
    frame: &ff::AVFrame,
    skip: usize,
    take: usize,
    position: u64,
    out: &mut PcmBuffer,
) -> Result<(), Error> {
    let (conversion, planar) =
        Conversion::of(frame.format).ok_or(Error::UnsupportedSampleFormat {
            format: frame.format,
        })?;
    // SAFETY: the frame's layout is valid.
    let spec = unsafe { spec_of(frame.format, frame.sample_rate, &frame.ch_layout) }?;
    let channels = usize::from(spec.channel_count());
    let planes = if planar { channels } else { 1 };
    if frame.extended_data.is_null() {
        return Err(Error::InvalidAudioFormat {
            reason: "frame without data",
        });
    }
    // SAFETY: an audio frame has one data pointer per plane in extended_data.
    let planes: Vec<*const u8> = (0..planes)
        .map(|p| unsafe { (*frame.extended_data.add(p)).cast_const() })
        .collect();
    if planes.iter().any(|p| p.is_null()) {
        return Err(Error::InvalidAudioFormat {
            reason: "frame without data",
        });
    }
    let source = Planes {
        planes: &planes,
        planar,
        channels,
        skip,
        take,
    };
    // Reuse the buffer's allocation when the format stays the same.
    let samples = std::mem::replace(&mut out.samples, PcmSamples::F32(Vec::new()));
    // SAFETY: the planes hold nb_samples frames of the frame's format, and
    // skip + take <= nb_samples is guaranteed by the caller.
    out.samples = unsafe {
        match conversion {
            Conversion::U8 => PcmSamples::U8(source.interleave(reuse_u8(samples), |x: u8| x)),
            Conversion::S16 => PcmSamples::S16(source.interleave(reuse_s16(samples), |x: i16| x)),
            Conversion::S32 => PcmSamples::S32(source.interleave(reuse_s32(samples), |x: i32| x)),
            Conversion::S64 => PcmSamples::S32(source.interleave(reuse_s32(samples), |x: i64| {
                // The top 32 bits always fit.
                i32::try_from(x >> 32).unwrap_or_default()
            })),
            Conversion::F32 => PcmSamples::F32(source.interleave(reuse_f32(samples), |x: f32| x)),
            #[allow(clippy::cast_possible_truncation)]
            Conversion::F64 => {
                PcmSamples::F32(source.interleave(reuse_f32(samples), |x: f64| x as f32))
            }
        }
    };
    out.spec = spec;
    out.position = position;
    out.frames = take;
    Ok(())
}

fn reuse_u8(s: PcmSamples) -> Vec<u8> {
    if let PcmSamples::U8(v) = s {
        v
    } else {
        Vec::new()
    }
}
fn reuse_s16(s: PcmSamples) -> Vec<i16> {
    if let PcmSamples::S16(v) = s {
        v
    } else {
        Vec::new()
    }
}
fn reuse_s32(s: PcmSamples) -> Vec<i32> {
    if let PcmSamples::S32(v) = s {
        v
    } else {
        Vec::new()
    }
}
fn reuse_f32(s: PcmSamples) -> Vec<f32> {
    if let PcmSamples::F32(v) = s {
        v
    } else {
        Vec::new()
    }
}

/// The sample planes of one frame and the part of it to copy.
struct Planes<'a> {
    planes: &'a [*const u8],
    planar: bool,
    channels: usize,
    skip: usize,
    take: usize,
}

impl Planes<'_> {
    /// Interleaves the frames `skip .. skip + take` into `out` (cleared first),
    /// mapping each sample with `map`.
    ///
    /// # Safety
    /// Each plane must hold at least `skip + take` frames of `T` (one channel per
    /// plane when planar, all channels interleaved otherwise), suitably aligned,
    /// which FFmpeg guarantees for its sample buffers.
    unsafe fn interleave<T: Copy, U>(&self, mut out: Vec<U>, map: impl Fn(T) -> U) -> Vec<U> {
        out.clear();
        out.reserve(self.take * self.channels);
        if self.planar {
            let slices: Vec<&[T]> = self
                .planes
                .iter()
                // SAFETY: guaranteed by the caller.
                .map(|&p| unsafe { std::slice::from_raw_parts(p.cast::<T>(), self.skip + self.take) })
                .collect();
            for i in self.skip..self.skip + self.take {
                out.extend(slices.iter().map(|s| map(s[i])));
            }
        } else {
            let len = (self.skip + self.take) * self.channels;
            // SAFETY: guaranteed by the caller.
            let all = unsafe { std::slice::from_raw_parts(self.planes[0].cast::<T>(), len) };
            out.extend(all[self.skip * self.channels..].iter().map(|&x| map(x)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaves_planar_and_packed() {
        let left: [i16; 4] = [1, 2, 3, 4];
        let right: [i16; 4] = [-1, -2, -3, -4];
        let planes = [left.as_ptr().cast::<u8>(), right.as_ptr().cast::<u8>()];
        let source = Planes {
            planes: &planes,
            planar: true,
            channels: 2,
            skip: 1,
            take: 2,
        };
        // SAFETY: both planes hold 4 aligned i16 values.
        let out = unsafe { source.interleave(Vec::new(), |x: i16| x) };
        assert_eq!(out, [2, -2, 3, -3]);

        let packed: [i16; 6] = [1, -1, 2, -2, 3, -3];
        let planes = [packed.as_ptr().cast::<u8>()];
        let source = Planes {
            planes: &planes,
            planar: false,
            channels: 2,
            skip: 2,
            take: 1,
        };
        // SAFETY: the plane holds 3 frames of 2 aligned i16 values.
        let out = unsafe { source.interleave(Vec::new(), |x: i16| x) };
        assert_eq!(out, [3, -3]);
    }

    #[test]
    fn integer_formats_stay_integer() {
        assert_eq!(
            Conversion::of(fmt::S32P).map(|(c, _)| c.output()),
            Some(SampleFormat::S32(Endianness::NATIVE))
        );
        assert_eq!(
            Conversion::of(fmt::DBL).map(|(c, _)| c.output()),
            Some(SampleFormat::F32(Endianness::NATIVE))
        );
        assert_eq!(Conversion::of(9999), None);
    }
}
