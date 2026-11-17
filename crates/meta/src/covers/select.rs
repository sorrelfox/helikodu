// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Choosing one cover for a track.

use std::path::Path;

use super::embedded::read_embedded_covers;
use super::folder::{FolderArtOptions, find_folder_covers};
use super::limits::CoverLimits;
use super::picture_type::PictureType;
use super::source::Cover;

/// Whether embedded pictures or folder images are preferred.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CoverPreference {
    /// The embedded picture, else a folder image. The default: a picture in the
    /// file belongs to that track, while a folder may hold several releases.
    #[default]
    Embedded,
    /// A folder image, else the embedded picture.
    Folder,
}

/// How [`find_cover`] looks for a cover.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoverOptions {
    /// The limits on every picture.
    pub limits: CoverLimits,
    /// Where and under which names folder images are looked for.
    pub folder: FolderArtOptions,
    /// Embedded pictures or folder images first.
    pub prefer: CoverPreference,
}

/// The index of the picture in `covers` that best serves as the album cover.
///
/// The picture type decides first (see [`PictureType::cover_rank`]: the front
/// cover, then untyped or "other" pictures, then any other type, then the back
/// cover, then file icons). Among pictures of the same rank the largest in bytes
/// wins, as a file with two front covers usually holds a full-size picture and a
/// small one; among equal sizes, the first.
pub fn choose_front(covers: &[Cover]) -> Option<usize> {
    covers
        .iter()
        .enumerate()
        .min_by_key(|(i, c)| {
            (
                PictureType::cover_rank(c.source().picture_type()),
                std::cmp::Reverse(c.byte_len()),
                *i,
            )
        })
        .map(|(i, _)| i)
}

/// The cover of the track at `track`: its best embedded picture and the first
/// folder image in a decodable format, in the order `options.prefer` gives.
///
/// A file whose pictures cannot be read counts as one without pictures, and a
/// folder image that cannot be read or is no JPEG, PNG or WebP is passed over.
/// Folder images have only their first bytes read; the rest is read when the
/// returned cover's bytes are needed.
pub fn find_cover(track: &Path, options: &CoverOptions) -> Option<Cover> {
    let embedded = || {
        read_embedded_covers(track, &options.limits)
            .ok()
            .and_then(|c| c.into_front())
    };
    let folder = || {
        find_folder_covers(track, &options.folder, &options.limits)
            .covers
            .into_iter()
            .find(|c| c.format().is_ok_and(|f| f.is_decodable()))
    };
    match options.prefer {
        CoverPreference::Embedded => embedded().or_else(folder),
        CoverPreference::Folder => folder().or_else(embedded),
    }
}
