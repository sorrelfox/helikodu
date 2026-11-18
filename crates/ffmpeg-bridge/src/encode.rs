// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Encoding PCM into a file for the converter.
//!
//! The formats are a fixed list ([`EncodeFormat`]), each with the encoders it may
//! use and the one muxer that writes it. Muxers that open other files or network
//! resources (`hls`, `segment`, `tee`, `image2`, …) can never be chosen. The output
//! is a [`File`](std::fs::File) the caller opened: FFmpeg gets custom write I/O and
//! no file name, the format context refuses every nested open, and its
//! `protocol_whitelist` is `file`. Options FFmpeg does not take are an error, as
//! when opening a file.

mod audio_encoder;
mod output_io;

use std::ffi::{CStr, c_int};
use std::ptr;

pub use audio_encoder::{AudioEncoder, MAX_FLOAT_SAMPLE};

use crate::ffi::ff;

/// A format the converter can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum EncodeFormat {
    /// Opus in Ogg (`.opus`).
    Opus,
    /// Vorbis in Ogg (`.ogg`).
    Vorbis,
    /// MP3 (`.mp3`).
    Mp3,
    /// AAC in MP4 (`.m4a`).
    Aac,
    /// FLAC (`.flac`).
    Flac,
    /// Apple Lossless in MP4 (`.m4a`).
    Alac,
    /// PCM in WAVE (`.wav`), 16 or 24 bits.
    Wav,
}

impl EncodeFormat {
    /// Every format, in the order the converter lists them.
    pub const ALL: [Self; 7] = [
        Self::Opus,
        Self::Mp3,
        Self::Aac,
        Self::Vorbis,
        Self::Flac,
        Self::Alac,
        Self::Wav,
    ];

    /// The usual name of the format, which is not translated.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Opus => "Opus",
            Self::Vorbis => "Vorbis",
            Self::Mp3 => "MP3",
            Self::Aac => "AAC",
            Self::Flac => "FLAC",
            Self::Alac => "ALAC",
            Self::Wav => "WAV",
        }
    }

    /// A stable identifier, for settings files.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Vorbis => "vorbis",
            Self::Mp3 => "mp3",
            Self::Aac => "aac",
            Self::Flac => "flac",
            Self::Alac => "alac",
            Self::Wav => "wav",
        }
    }

    /// The format with the identifier `id`.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.id() == id)
    }

    /// The file name extension, without the dot.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Vorbis => "ogg",
            Self::Mp3 => "mp3",
            Self::Aac | Self::Alac => "m4a",
            Self::Flac => "flac",
            Self::Wav => "wav",
        }
    }

    /// Whether the format keeps every sample exactly.
    pub const fn is_lossless(self) -> bool {
        matches!(self, Self::Flac | Self::Alac | Self::Wav)
    }

    /// FFmpeg's name of the codec the format carries, as decoders report it
    /// (`opus`, `mp3`, `flac`, `pcm_s16le`, …), for "already in this format".
    pub fn codec_names(self) -> &'static [&'static str] {
        match self {
            Self::Opus => &["opus"],
            Self::Vorbis => &["vorbis"],
            Self::Mp3 => &["mp3", "mp3float"],
            Self::Aac => &["aac"],
            Self::Flac => &["flac"],
            Self::Alac => &["alac"],
            Self::Wav => &["pcm_s16le", "pcm_s24le"],
        }
    }

    /// The most channels the format takes.
    pub const fn max_channels(self) -> u16 {
        match self {
            Self::Mp3 => 2,
            _ => 8,
        }
    }

    /// The encoders the format may use, the preferred one first.
    fn encoders(self, bits: u8) -> &'static [&'static CStr] {
        match self {
            Self::Opus => &[c"libopus", c"opus"],
            Self::Vorbis => &[c"libvorbis"],
            Self::Mp3 => &[c"libmp3lame"],
            Self::Aac => &[c"libfdk_aac", c"aac"],
            Self::Flac => &[c"flac"],
            Self::Alac => &[c"alac"],
            Self::Wav if bits > 16 => &[c"pcm_s24le"],
            Self::Wav => &[c"pcm_s16le"],
        }
    }

    /// The muxer that writes the format.
    fn muxer(self) -> &'static CStr {
        match self {
            Self::Opus => c"opus",
            Self::Vorbis => c"ogg",
            Self::Mp3 => c"mp3",
            Self::Aac | Self::Alac => c"ipod",
            Self::Flac => c"flac",
            Self::Wav => c"wav",
        }
    }
}

/// Whether this FFmpeg build can write a format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncoderAvailability {
    /// It can, with this encoder (FFmpeg's name, such as `libopus`).
    Available {
        /// The encoder's name.
        encoder: String,
    },
    /// No encoder for the format; `wanted` names the encoders looked for.
    NoEncoder {
        /// The encoders looked for, the preferred one first.
        wanted: Vec<String>,
    },
    /// The encoder is there but not the muxer that writes the file.
    NoMuxer {
        /// The muxer's name.
        muxer: String,
    },
}

impl EncoderAvailability {
    /// Whether the format can be written.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

/// Whether this FFmpeg build can write `format`.
pub fn encoder_availability(format: EncodeFormat) -> EncoderAvailability {
    let Some((_, name)) = find_encoder(format, 16) else {
        return EncoderAvailability::NoEncoder {
            wanted: format
                .encoders(16)
                .iter()
                .map(|n| n.to_string_lossy().into_owned())
                .collect(),
        };
    };
    // SAFETY: a NUL-terminated name; the result is a static description or NULL.
    let muxer = unsafe { ff::av_guess_format(format.muxer().as_ptr(), ptr::null(), ptr::null()) };
    if muxer.is_null() {
        return EncoderAvailability::NoMuxer {
            muxer: format.muxer().to_string_lossy().into_owned(),
        };
    }
    EncoderAvailability::Available { encoder: name }
}

/// The sample rates the encoder of `format` takes, in ascending order; `None`
/// when it takes any rate or the format cannot be written.
pub fn supported_sample_rates(format: EncodeFormat) -> Option<Vec<u32>> {
    let (codec, _) = find_encoder(format, 16)?;
    // SAFETY: codec is a valid encoder.
    let mut rates = unsafe { audio_encoder::supported_rates(codec) }?;
    rates.sort_unstable();
    rates.dedup();
    Some(rates)
}

/// The rate of `rates` (as [`supported_sample_rates`] gives them) closest to
/// `wanted`, preferring a higher one at an equal distance; `wanted` itself when
/// any rate is taken.
pub fn nearest_rate(rates: Option<&[u32]>, wanted: u32) -> u32 {
    let Some(rates) = rates.filter(|r| !r.is_empty()) else {
        return wanted;
    };
    if rates.contains(&wanted) {
        return wanted;
    }
    // A higher rate keeps the whole band; take the lowest one above, if any.
    rates
        .iter()
        .copied()
        .filter(|&r| r > wanted)
        .min()
        .or_else(|| rates.iter().copied().max())
        .unwrap_or(wanted)
}

/// The first encoder of `format` this build has, and its name.
fn find_encoder(format: EncodeFormat, bits: u8) -> Option<(*const ff::AVCodec, String)> {
    format.encoders(bits).iter().find_map(|name| {
        // SAFETY: a NUL-terminated name; the result is a static codec or NULL.
        let codec = unsafe { ff::avcodec_find_encoder_by_name(name.as_ptr()) };
        (!codec.is_null()).then(|| (codec, name.to_string_lossy().into_owned()))
    })
}

/// How hard the encoder works for its size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EncodeQuality {
    /// A target bit rate in kbit/s (variable where the encoder varies it).
    Bitrate(u32),
    /// A LAME VBR level, 0 (best) to 9; for other lossy encoders a bit rate is
    /// picked from it.
    Vbr(u8),
    /// Lossless formats: the compression level, if not the encoder's default.
    Lossless(Option<u8>),
}

/// What to encode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncoderConfig {
    /// The format.
    pub format: EncodeFormat,
    /// The sample rate in Hz; must be one the encoder takes.
    pub sample_rate: u32,
    /// The number of channels, 1 to [`EncodeFormat::max_channels`]; written in
    /// WAVE order with FFmpeg's default layout for the count.
    pub channels: u16,
    /// For lossless formats, the bits per sample: 16 or 24.
    pub bits: u8,
    /// The quality.
    pub quality: EncodeQuality,
}

/// Reads `count` values of type `T` from FFmpeg's list at `list`.
///
/// # Safety
/// `list` must be NULL or point to at least `count` values.
unsafe fn read_list<T: Copy>(list: *const std::ffi::c_void, count: c_int) -> Option<Vec<T>> {
    let count = usize::try_from(count).ok()?;
    if list.is_null() {
        return None;
    }
    // SAFETY: guaranteed by the caller.
    Some(unsafe { std::slice::from_raw_parts(list.cast::<T>(), count) }.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rate_prefers_the_next_higher_rate() {
        let opus = [8_000, 12_000, 16_000, 24_000, 48_000];
        assert_eq!(nearest_rate(Some(&opus), 44_100), 48_000);
        assert_eq!(nearest_rate(Some(&opus), 96_000), 48_000);
        assert_eq!(nearest_rate(Some(&opus), 16_000), 16_000);
        assert_eq!(nearest_rate(None, 88_200), 88_200);
    }

    #[test]
    fn identifiers_round_trip() {
        for format in EncodeFormat::ALL {
            assert_eq!(EncodeFormat::from_id(format.id()), Some(format));
        }
    }

    #[test]
    fn flac_and_wav_are_always_there() {
        assert!(encoder_availability(EncodeFormat::Flac).is_available());
        assert!(encoder_availability(EncodeFormat::Wav).is_available());
    }
}
