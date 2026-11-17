// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The covers of one file, as a worker reports them: the best embedded picture and
//! the first usable folder image.
//!
//! Each picture is hashed and its header is checked within the picture limits
//! ([`CoverLimits`]): the format must be JPEG, PNG or WebP and the declared size
//! within the pixel limit. No pixel is decoded here, and the picture's bytes are not
//! sent to the scanner: the thumbnail cache reads them again when it needs them, so
//! a scan of a large library moves hashes, not megabytes of pictures, through the
//! pipes and into the database.
//!
//! A folder image serves every track in its folder, so its hash is remembered by
//! path, size and modification time for the life of the worker; the folder is still
//! listed for every file, which is cheap.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use super::message::{CoverOrigin, CoverReport};
use crate::covers::{
    Cover, CoverHash, CoverLimits, CoverSource, FolderArtOptions, PictureType, SkipReason,
    find_folder_covers, read_embedded_covers,
};

/// The most folder images remembered at once.
const MAX_REMEMBERED: usize = 256;

/// The best embedded picture of the file at `path` whose header is good. Pictures
/// left out, and ones whose header is bad, are noted in `notes`.
pub(super) fn embedded_cover(
    path: &Path,
    limits: &CoverLimits,
    notes: &mut Vec<String>,
) -> Option<CoverReport> {
    let found = match read_embedded_covers(path, limits) {
        Ok(found) => found,
        Err(e) => {
            notes.push(format!("embedded pictures: {e}"));
            return None;
        }
    };
    for skipped in &found.skipped {
        let why = match skipped.reason {
            SkipReason::TooLarge => format!("{} bytes is over the limit", skipped.byte_len),
            SkipReason::TooMany => "too many pictures".to_owned(),
            SkipReason::Empty => "empty".to_owned(),
            SkipReason::UnknownFormat => "not a known picture format".to_owned(),
            SkipReason::UnsupportedFormat(format) => {
                format!("{} is not supported", format.mime_type())
            }
        };
        notes.push(format!(
            "embedded picture {}: skipped: {why}",
            index_of(&skipped.source)
        ));
    }
    // The order of `choose_front`: picture type, then the larger picture, then the
    // first.
    let mut order: Vec<&Cover> = found.covers.iter().collect();
    order.sort_by_key(|c| {
        (
            PictureType::cover_rank(c.source().picture_type()),
            std::cmp::Reverse(c.byte_len()),
        )
    });
    for cover in order {
        let CoverSource::Embedded {
            tag, picture_type, ..
        } = cover.source()
        else {
            continue;
        };
        let index = index_of(cover.source());
        match report(cover, limits) {
            Ok((hash, info)) => {
                return Some(CoverReport {
                    hash,
                    origin: CoverOrigin::Embedded {
                        tag: *tag,
                        picture_type: *picture_type,
                        index,
                    },
                    format: info.format,
                    width: info.width,
                    height: info.height,
                    byte_len: cover.byte_len(),
                });
            }
            Err(e) => notes.push(format!("embedded picture {index}: {e}")),
        }
    }
    None
}

/// The first folder image for the file at `path` in a decodable format whose header
/// is good, looked for under the default names and `album`. Images whose header is
/// bad are noted in `notes`.
pub(super) fn folder_cover(
    path: &Path,
    album: Option<&str>,
    limits: &CoverLimits,
    notes: &mut Vec<String>,
) -> Option<CoverReport> {
    let options = FolderArtOptions {
        album_title: album.map(str::to_owned),
        ..FolderArtOptions::default()
    };
    let found = find_folder_covers(path, &options, limits);
    for skipped in &found.skipped {
        if let Some(image) = skipped.source.path() {
            notes.push(format!(
                "folder image {}: skipped: {} bytes is over the limit",
                image.display(),
                skipped.byte_len
            ));
        }
    }
    for cover in &found.covers {
        let Some(image) = cover.source().path() else {
            continue;
        };
        if !cover.format().is_ok_and(|f| f.is_decodable()) {
            continue;
        }
        match remembered_report(image, cover, limits) {
            Ok(report) => return Some(report),
            Err(e) => notes.push(format!("folder image {}: {e}", image.display())),
        }
    }
    None
}

/// What is remembered of one folder image.
#[derive(Debug, Clone)]
struct Remembered {
    size: u64,
    mtime_ns: i128,
    result: Result<CoverReport, String>,
}

static REMEMBERED: Mutex<Option<HashMap<PathBuf, Remembered>>> = Mutex::new(None);

/// The report of the folder image `cover` at `image`, remembered by its path, size
/// and modification time.
fn remembered_report(
    image: &Path,
    cover: &Cover,
    limits: &CoverLimits,
) -> Result<CoverReport, String> {
    let metadata = fs::metadata(image).map_err(|e| e.to_string())?;
    let size = metadata.len();
    let mtime_ns = i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
    {
        let guard = REMEMBERED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = guard.as_ref().and_then(|map| map.get(image))
            && known.size == size
            && known.mtime_ns == mtime_ns
        {
            return known.result.clone();
        }
    }
    let result = report(cover, limits)
        .map(|(hash, info)| CoverReport {
            hash,
            origin: CoverOrigin::Folder {
                path: image.to_path_buf(),
            },
            format: info.format,
            width: info.width,
            height: info.height,
            byte_len: cover.byte_len(),
        })
        .map_err(|e| e.to_string());
    let mut guard = REMEMBERED.lock().unwrap_or_else(PoisonError::into_inner);
    let map = guard.get_or_insert_with(HashMap::new);
    if map.len() >= MAX_REMEMBERED {
        map.clear();
    }
    map.insert(
        image.to_path_buf(),
        Remembered {
            size,
            mtime_ns,
            result: result.clone(),
        },
    );
    result
}

/// The hash and header of `cover`; loads its bytes.
fn report(
    cover: &Cover,
    limits: &CoverLimits,
) -> Result<(CoverHash, crate::covers::PictureInfo), crate::covers::CoverError> {
    let info = cover.probe(limits)?;
    Ok((cover.hash()?, info))
}

fn index_of(source: &CoverSource) -> u32 {
    match source {
        CoverSource::Embedded { index, .. } => u32::try_from(*index).unwrap_or(u32::MAX),
        CoverSource::Folder { .. } => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::covers::{PictureFormat, RgbaPicture};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("hk-worker-covers-{}", std::process::id()))
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png(side: u32) -> Vec<u8> {
        RgbaPicture::new(side, side, vec![200; (side * side * 4) as usize])
            .unwrap()
            .encode_png()
            .unwrap()
    }

    #[test]
    fn a_folder_image_is_reported_and_remembered() {
        let dir = scratch("folder");
        let bytes = png(8);
        fs::write(dir.join("Cover.PNG"), &bytes).unwrap();
        let mut notes = Vec::new();
        let first = folder_cover(
            &dir.join("01.flac"),
            None,
            &CoverLimits::default(),
            &mut notes,
        )
        .unwrap();
        assert_eq!(first.hash, CoverHash::of(&bytes));
        assert_eq!(first.format, PictureFormat::Png);
        assert_eq!((first.width, first.height), (8, 8));
        assert_eq!(first.byte_len, bytes.len() as u64);
        assert_eq!(
            first.origin,
            CoverOrigin::Folder {
                path: dir.join("Cover.PNG")
            }
        );
        let second = folder_cover(
            &dir.join("02.flac"),
            None,
            &CoverLimits::default(),
            &mut notes,
        )
        .unwrap();
        assert_eq!(second, first);
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn a_broken_folder_image_is_passed_over_and_noted() {
        let dir = scratch("broken");
        // A PNG signature, then garbage.
        let mut broken = b"\x89PNG\r\n\x1a\n".to_vec();
        broken.extend_from_slice(&[0xab; 64]);
        fs::write(dir.join("cover.png"), &broken).unwrap();
        let good = png(4);
        fs::write(dir.join("folder.png"), &good).unwrap();
        let mut notes = Vec::new();
        let found = folder_cover(
            &dir.join("a.flac"),
            None,
            &CoverLimits::default(),
            &mut notes,
        )
        .unwrap();
        assert_eq!(found.hash, CoverHash::of(&good));
        assert_eq!(notes.len(), 1, "{notes:?}");
    }

    #[test]
    fn a_picture_over_the_pixel_limit_is_refused() {
        let dir = scratch("huge");
        fs::write(dir.join("cover.png"), png(64)).unwrap();
        let limits = CoverLimits {
            max_pixels: 1000,
            ..CoverLimits::default()
        };
        let mut notes = Vec::new();
        assert_eq!(
            folder_cover(&dir.join("a.flac"), None, &limits, &mut notes),
            None
        );
        assert_eq!(notes.len(), 1, "{notes:?}");
    }

    #[test]
    fn a_file_without_pictures_has_no_embedded_cover() {
        let dir = scratch("none");
        let path = dir.join("noise.flac");
        fs::write(&path, [0x5a; 512]).unwrap();
        let mut notes = Vec::new();
        assert_eq!(
            embedded_cover(&path, &CoverLimits::default(), &mut notes),
            None
        );
    }
}
