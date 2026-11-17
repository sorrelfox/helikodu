// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading a cover's bytes again from where the scan found it, and making a
//! thumbnail of them.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use helikodu_meta::covers::{
    CoverHash, CoverLimits, CoverSource, make_thumbnail, read_embedded_covers,
};

use super::sizes::ThumbnailSize;
use crate::catalog::CoverLocation;

/// The quality of the JPEG thumbnails.
pub(crate) const JPEG_QUALITY: u8 = 88;

/// Why a cover's bytes could not be had, or no thumbnail made of them.
#[derive(Debug, thiserror::Error)]
pub enum CoverReadError {
    /// No location holds a picture with the cover's hash any more (the files
    /// changed or are gone), or the library knows no such cover.
    #[error("the cover is not found where it was")]
    NotFound,
    /// The picture was found but cannot be decoded or encoded.
    #[error(transparent)]
    Picture(#[from] helikodu_meta::covers::CoverError),
    /// The thumbnail could not be written.
    #[error("cannot write the thumbnail: {0}")]
    Io(#[from] std::io::Error),
}

/// The encoded bytes of the cover with `hash`, from the first of `locations` that
/// still holds them. Embedded pictures are read through the tag library (FFmpeg for
/// the formats it lacks), image files directly; both within `limits`. Bytes that do
/// not hash to `hash` are passed over.
pub fn read_cover(
    hash: &CoverHash,
    locations: &[CoverLocation],
    limits: &CoverLimits,
) -> Result<Vec<u8>, CoverReadError> {
    for location in locations {
        let bytes = match location {
            CoverLocation::File { path } => read_limited(path, limits.max_bytes),
            CoverLocation::Embedded { file, index } => embedded(file, *index, hash, limits),
        };
        if let Some(bytes) = bytes
            && CoverHash::of(&bytes) == *hash
        {
            return Ok(bytes);
        }
    }
    Err(CoverReadError::NotFound)
}

/// The picture of `file` at `index`, or else any of its pictures with `hash` (the
/// pictures may have been reordered).
fn embedded(file: &Path, index: u32, hash: &CoverHash, limits: &CoverLimits) -> Option<Vec<u8>> {
    let found = read_embedded_covers(file, limits).ok()?;
    let at = found.covers.iter().position(|c| {
        matches!(c.source(), CoverSource::Embedded { index: i, .. } if *i as u64 == u64::from(index))
    });
    if let Some(at) = at
        && let Ok(bytes) = found.covers[at].bytes()
        && CoverHash::of(bytes) == *hash
    {
        return Some(bytes.to_vec());
    }
    found
        .covers
        .into_iter()
        .filter_map(|c| c.into_bytes().ok())
        .find(|bytes| CoverHash::of(bytes) == *hash)
}

/// The file at `path`, unless it is larger than `max_bytes`.
fn read_limited(path: &Path, max_bytes: usize) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let limit = max_bytes as u64;
    if file.metadata().ok()?.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

/// A thumbnail of the encoded picture `bytes` in `size`, within `limits`: JPEG
/// when the picture is opaque, PNG otherwise. Returns the encoded thumbnail and its
/// file extension.
pub fn render_thumbnail(
    bytes: &[u8],
    size: ThumbnailSize,
    limits: &CoverLimits,
) -> Result<(Vec<u8>, &'static str), CoverReadError> {
    let picture = make_thumbnail(bytes, size.pixels(), limits)?;
    if picture.is_opaque() {
        Ok((picture.encode_jpeg(JPEG_QUALITY)?, "jpg"))
    } else {
        Ok((picture.encode_png()?, "png"))
    }
}
