// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pictures embedded in audio files: lofty first, FFmpeg as the fallback.

mod ffmpeg_pictures;
mod lofty_pictures;

use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::Path;

use super::error::CoverError;
use super::format::PictureFormat;
use super::limits::CoverLimits;
use super::picture_type::PictureType;
use super::select::choose_front;
use super::source::{Cover, CoverSource, EmbeddedTag};

/// The pictures embedded in one audio file.
#[derive(Debug, Clone, Default)]
pub struct EmbeddedCovers {
    /// The pictures within the limits and in a decodable format, in the order
    /// they were read.
    pub covers: Vec<Cover>,
    /// The pictures left out, and why.
    pub skipped: Vec<SkippedPicture>,
    /// Why FFmpeg read the pictures instead of the tag library, when it did.
    pub fallback_reason: Option<String>,
}

impl EmbeddedCovers {
    /// The picture that best serves as the album cover (see [`choose_front`]).
    pub fn front(&self) -> Option<&Cover> {
        choose_front(&self.covers).and_then(|i| self.covers.get(i))
    }

    /// The best picture, consuming the list.
    pub fn into_front(mut self) -> Option<Cover> {
        let i = choose_front(&self.covers)?;
        Some(self.covers.swap_remove(i))
    }
}

/// A picture that was found but left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPicture {
    /// Where it was found.
    pub source: CoverSource,
    /// Its size in bytes.
    pub byte_len: u64,
    /// Why it was left out.
    pub reason: SkipReason,
}

/// Why a picture was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Larger than [`CoverLimits::max_bytes`].
    TooLarge,
    /// Beyond [`CoverLimits::max_count`] pictures in one file.
    TooMany,
    /// Empty.
    Empty,
    /// Not a picture format known here.
    UnknownFormat,
    /// A known format that is not decoded (GIF, BMP, AVIF, …).
    UnsupportedFormat(PictureFormat),
}

/// One picture as a reader found it, before the limits are applied.
struct RawPicture<'a> {
    tag: EmbeddedTag,
    picture_type: Option<PictureType>,
    description: Option<String>,
    data: &'a [u8],
}

/// Applies the limits to the pictures of one file, in order.
struct Collector<'l> {
    limits: &'l CoverLimits,
    out: EmbeddedCovers,
    index: usize,
}

impl<'l> Collector<'l> {
    fn new(limits: &'l CoverLimits) -> Self {
        Self {
            limits,
            out: EmbeddedCovers::default(),
            index: 0,
        }
    }

    fn push(&mut self, picture: RawPicture<'_>) {
        let source = CoverSource::Embedded {
            tag: picture.tag,
            picture_type: picture.picture_type,
            index: self.index,
            description: picture.description.filter(|d| !d.is_empty()),
        };
        self.index += 1;
        let byte_len = picture.data.len() as u64;
        let reason = if picture.data.is_empty() {
            Some(SkipReason::Empty)
        } else if picture.data.len() > self.limits.max_bytes {
            Some(SkipReason::TooLarge)
        } else if self.out.covers.len() >= self.limits.max_count {
            Some(SkipReason::TooMany)
        } else {
            match PictureFormat::sniff(picture.data) {
                None => Some(SkipReason::UnknownFormat),
                Some(f) if !f.is_decodable() => Some(SkipReason::UnsupportedFormat(f)),
                Some(_) => None,
            }
        };
        match reason {
            Some(reason) => self.out.skipped.push(SkippedPicture {
                source,
                byte_len,
                reason,
            }),
            None => self
                .out
                .covers
                .push(Cover::embedded(source, picture.data.to_vec())),
        }
    }

    fn finish(self) -> EmbeddedCovers {
        self.out
    }
}

/// Reads the pictures embedded in the audio file at `path`.
///
/// lofty reads ID3v2 `APIC`, APEv2 cover items, FLAC `PICTURE` blocks, Vorbis
/// `METADATA_BLOCK_PICTURE` comments and MP4 `covr` atoms. When it does not know
/// the format (Matroska, ASF/WMA, …) or cannot parse the file, FFmpeg's attached
/// pictures and Matroska image attachments are read instead.
///
/// Pictures over the byte limit, beyond the count limit, empty, or in a format
/// that is not decoded are listed in [`EmbeddedCovers::skipped`].
pub fn read_embedded_covers(
    path: impl AsRef<Path>,
    limits: &CoverLimits,
) -> Result<EmbeddedCovers, CoverError> {
    read_embedded_covers_from_file(File::open(path)?, limits)
}

/// Reads the pictures embedded in an open file, like [`read_embedded_covers`].
pub fn read_embedded_covers_from_file(
    file: File,
    limits: &CoverLimits,
) -> Result<EmbeddedCovers, CoverError> {
    let mut second = file.try_clone()?;
    let reason = match lofty_pictures::read(file, limits) {
        Ok(covers) => return Ok(covers),
        Err(e) => e,
    };
    second.seek(SeekFrom::Start(0))?;
    match ffmpeg_pictures::read(second, limits) {
        Ok(mut covers) => {
            covers.fallback_reason = Some(reason);
            Ok(covers)
        }
        Err(e) => Err(CoverError::Unreadable(format!(
            "tag library: {reason}; FFmpeg: {e}"
        ))),
    }
}
