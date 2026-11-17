// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the bridge reports about each stream of a file.

use std::ffi::c_int;

use helikodu_model::audio::MediaTime;

use crate::ffi::{self, ff, media_type};
use crate::metadata::Tags;

/// The kind of a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamKind {
    /// Audio.
    Audio,
    /// Video, including attached cover pictures.
    Video,
    /// Subtitles.
    Subtitle,
    /// Opaque data.
    Data,
    /// An attachment, such as a Matroska font or picture.
    Attachment,
    /// A kind the bridge does not know, with FFmpeg's `AVMediaType` value.
    Other(i32),
}

impl StreamKind {
    pub(crate) fn from_raw(raw: c_int) -> Self {
        match raw {
            media_type::AUDIO => Self::Audio,
            media_type::VIDEO => Self::Video,
            media_type::SUBTITLE => Self::Subtitle,
            media_type::DATA => Self::Data,
            media_type::ATTACHMENT => Self::Attachment,
            other => Self::Other(other),
        }
    }
}

/// A stream's disposition flags (FFmpeg's `AV_DISPOSITION_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Disposition(u32);

impl Disposition {
    pub(crate) fn from_raw(raw: c_int) -> Self {
        Self(u32::from_ne_bytes(raw.to_ne_bytes()))
    }

    fn has(self, flag: c_int) -> bool {
        self.0 & u32::from_ne_bytes(flag.to_ne_bytes()) != 0
    }

    /// The raw flags.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// The stream is marked as the one to play by default.
    pub fn is_default(self) -> bool {
        self.has(ff::AV_DISPOSITION_DEFAULT)
    }

    /// The stream is a dubbed version.
    pub fn is_dub(self) -> bool {
        self.has(ff::AV_DISPOSITION_DUB)
    }

    /// The stream is in the original language.
    pub fn is_original(self) -> bool {
        self.has(ff::AV_DISPOSITION_ORIGINAL)
    }

    /// The stream is a commentary.
    pub fn is_comment(self) -> bool {
        self.has(ff::AV_DISPOSITION_COMMENT)
    }

    /// The stream is a karaoke track.
    pub fn is_karaoke(self) -> bool {
        self.has(ff::AV_DISPOSITION_KARAOKE)
    }

    /// The stream is meant for the hearing impaired.
    pub fn is_hearing_impaired(self) -> bool {
        self.has(ff::AV_DISPOSITION_HEARING_IMPAIRED)
    }

    /// The stream is meant for the visually impaired (an audio description).
    pub fn is_visual_impaired(self) -> bool {
        self.has(ff::AV_DISPOSITION_VISUAL_IMPAIRED)
    }

    /// The stream is a single attached picture, such as cover art.
    pub fn is_attached_picture(self) -> bool {
        self.has(ff::AV_DISPOSITION_ATTACHED_PIC)
    }
}

/// One stream of an opened file.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// The stream's index in the file.
    pub index: usize,
    /// What the stream holds.
    pub kind: StreamKind,
    /// FFmpeg's short name of the codec, such as `flac` or `aac`.
    pub codec: String,
    /// The disposition flags.
    pub disposition: Disposition,
    /// The language tag, if the container gives one.
    pub language: Option<String>,
    /// The stream's title, if the container gives one.
    pub title: Option<String>,
    /// The stream's own tags. Some containers, such as Ogg, keep the file's tags here.
    pub tags: Tags,
    /// The sample rate of an audio stream, in Hz.
    pub sample_rate: Option<u32>,
    /// The channel count of an audio stream.
    pub channels: Option<u16>,
    /// The number of significant bits per sample of lossless audio (24 for a 24-bit
    /// FLAC file, whose samples are decoded into 32-bit words).
    pub bits_per_sample: Option<u32>,
    /// The duration the container states for the stream. It may include the encoder
    /// delay; [`AudioDecoder::duration`](crate::AudioDecoder::duration) corrects for
    /// what it can.
    pub duration: Option<MediaTime>,
}

impl StreamInfo {
    /// Reads the stream's description.
    ///
    /// # Safety
    /// `st` must be a valid stream of an open format context.
    pub(crate) unsafe fn read(index: usize, st: *const ff::AVStream) -> Self {
        // SAFETY: guaranteed by the caller; enum fields are read as integers.
        let (par, disposition, metadata, time_base, duration) = unsafe {
            (
                (*st).codecpar,
                (*st).disposition,
                (*st).metadata,
                (*st).time_base,
                (*st).duration,
            )
        };
        // SAFETY: codecpar of a valid stream is valid.
        let (raw_kind, id, rate, channels, bits) = unsafe {
            (
                ffi::codec_type(par),
                ffi::codec_id(par),
                (*par).sample_rate,
                (*par).ch_layout.nb_channels,
                (*par).bits_per_raw_sample,
            )
        };
        // SAFETY: the stream's dictionary is valid while the context is open.
        let tags = Tags::from_entries(unsafe { ffi::dict_entries(metadata) });
        let kind = StreamKind::from_raw(raw_kind);
        let audio = kind == StreamKind::Audio;
        Self {
            index,
            kind,
            codec: ffi::codec_name(id),
            disposition: Disposition::from_raw(disposition),
            language: tags.get("language").map(str::to_owned),
            title: tags.get("title").map(str::to_owned),
            tags,
            sample_rate: u32::try_from(rate).ok().filter(|&r| audio && r > 0),
            channels: u16::try_from(channels).ok().filter(|&c| audio && c > 0),
            bits_per_sample: u32::try_from(bits).ok().filter(|&b| audio && b > 0),
            duration: ffi::media_time(duration, time_base),
        }
    }

    /// Whether this is an audio stream that can be played (not a picture).
    pub fn is_playable_audio(&self) -> bool {
        self.kind == StreamKind::Audio && !self.disposition.is_attached_picture()
    }
}
