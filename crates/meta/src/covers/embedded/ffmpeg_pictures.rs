// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Embedded pictures through FFmpeg: attached-picture streams and Matroska image
//! attachments. The bridge copies the bytes and never decodes them.

use std::fs::File;
use std::path::Path;

use helikodu_ffmpeg_bridge::{MediaFile, PictureLimits, PictureSource};

use super::{Collector, EmbeddedCovers, RawPicture};
use crate::covers::limits::CoverLimits;
use crate::covers::picture_type::PictureType;
use crate::covers::source::EmbeddedTag;

/// Reads the pictures of `file` through FFmpeg.
///
/// The bridge leaves out pictures over the byte limit without saying so, and
/// stops at the count limit; one more than the count limit is asked for, so
/// that a file with too many pictures is reported.
pub(super) fn read(
    file: File,
    limits: &CoverLimits,
) -> Result<EmbeddedCovers, helikodu_ffmpeg_bridge::Error> {
    let media = MediaFile::open_file(file)?;
    let pictures = media.attached_pictures(&PictureLimits {
        max_bytes: limits.max_bytes,
        max_count: limits.max_count.saturating_add(1),
    });
    let mut c = Collector::new(limits);
    for picture in &pictures {
        let tag = match picture.source {
            PictureSource::AttachedPicture => EmbeddedTag::FfmpegAttachedPicture,
            PictureSource::Attachment => EmbeddedTag::MatroskaAttachment,
        };
        // FFmpeg names the ID3v2/FLAC picture type in the description; for a
        // Matroska attachment (which FFmpeg may also present as an attached
        // picture) the description is its file name.
        let picture_type = picture
            .description
            .as_deref()
            .and_then(|d| PictureType::from_ffmpeg_name(d).or_else(|| attachment_type(d)));
        c.push(RawPicture {
            tag,
            picture_type,
            description: picture.description.clone(),
            data: &picture.data,
        });
    }
    Ok(c.finish())
}

/// The picture type of a Matroska attachment by its file name: the Matroska
/// specification names the front cover `cover.*`, its small version
/// `small_cover.*`, and the landscape versions `cover_land.*` and
/// `small_cover_land.*`.
fn attachment_type(file_name: &str) -> Option<PictureType> {
    let stem = Path::new(file_name).file_stem()?.to_str()?;
    if stem.eq_ignore_ascii_case("cover") || stem.eq_ignore_ascii_case("cover_land") {
        Some(PictureType::FRONT_COVER)
    } else if stem.eq_ignore_ascii_case("small_cover")
        || stem.eq_ignore_ascii_case("small_cover_land")
    {
        Some(PictureType::FILE_ICON)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matroska_attachment_names() {
        assert_eq!(attachment_type("cover.jpg"), Some(PictureType::FRONT_COVER));
        assert_eq!(attachment_type("Cover.PNG"), Some(PictureType::FRONT_COVER));
        assert_eq!(
            attachment_type("small_cover.jpg"),
            Some(PictureType::FILE_ICON)
        );
        assert_eq!(attachment_type("booklet.jpg"), None);
    }
}
