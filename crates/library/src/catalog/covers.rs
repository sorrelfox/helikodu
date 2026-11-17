// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Covers in the database: one row per content hash, the embedded cover of each
//! track, the folder image of each folder, and where the bytes of a cover can be read
//! again.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use helikodu_meta::covers::{CoverHash, PictureFormat};
use helikodu_meta::worker::{CoverOrigin, CoverReport};
use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension, params};

use super::ids::{AlbumId, sql_track_id};
use super::track_record::TrackCovers;
use crate::db::{path_from_blob, path_to_blob};

/// `covers.source` of a picture embedded in an audio file.
const SOURCE_EMBEDDED: i64 = 0;
/// `covers.source` of an image file.
const SOURCE_FILE: i64 = 1;

/// A cover as the library knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverInfo {
    /// The content hash of the encoded picture: the key of the thumbnail cache.
    pub hash: CoverHash,
    /// The width in pixels.
    pub width: Option<u32>,
    /// The height in pixels.
    pub height: Option<u32>,
    /// The size of the encoded picture in bytes.
    pub byte_len: Option<u64>,
}

/// Where the bytes of a cover can be read.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CoverLocation {
    /// A picture embedded in an audio file.
    Embedded {
        /// The audio file.
        file: PathBuf,
        /// The picture's position among the file's pictures (see
        /// [`CoverSource::Embedded`](helikodu_meta::covers::CoverSource::Embedded)).
        index: u32,
    },
    /// An image file.
    File {
        /// The image file.
        path: PathBuf,
    },
}

/// The most places [`cover_locations`] returns.
const MAX_LOCATIONS: i64 = 8;

/// The name stored in `covers.format`.
fn format_name(format: PictureFormat) -> &'static str {
    match format {
        PictureFormat::Jpeg => "jpeg",
        PictureFormat::Png => "png",
        PictureFormat::WebP => "webp",
        other => other.extension(),
    }
}

/// The row of the cover `report`, added if its hash is new. An embedded cover was
/// found in the audio file `file`.
pub(crate) fn upsert_cover(
    c: &Connection,
    report: &CoverReport,
    file: &Path,
) -> rusqlite::Result<i64> {
    let hash = report.hash.as_bytes().as_slice();
    let found: Option<i64> = c
        .prepare_cached("SELECT id FROM covers WHERE hash = ?1")?
        .query_row([hash], |row| row.get(0))
        .optional()?;
    if let Some(id) = found {
        return Ok(id);
    }
    let (source, path, index) = match &report.origin {
        CoverOrigin::Embedded { index, .. } => (SOURCE_EMBEDDED, file, Some(*index)),
        CoverOrigin::Folder { path } => (SOURCE_FILE, path.as_path(), None),
    };
    c.prepare_cached(
        "INSERT INTO covers (hash, source, source_path, source_index, width, height, format,
                             byte_len)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
    )?
    .query_row(
        params![
            hash,
            source,
            path_to_blob(path),
            index,
            report.width,
            report.height,
            format_name(report.format),
            i64::try_from(report.byte_len).unwrap_or(i64::MAX),
        ],
        |row| row.get(0),
    )
}

/// Records the covers found for the file of `track` (at `file`, in the folder row
/// `dir`): the track's embedded cover, and the folder's image.
pub(crate) fn write_track_covers(
    c: &Connection,
    track: i64,
    dir: i64,
    file: &Path,
    covers: &TrackCovers,
) -> rusqlite::Result<()> {
    let (cover, index) = match &covers.embedded {
        Some(report) => {
            let index = match report.origin {
                CoverOrigin::Embedded { index, .. } => Some(index),
                CoverOrigin::Folder { .. } => None,
            };
            (Some(upsert_cover(c, report, file)?), index)
        }
        None => (None, None),
    };
    c.prepare_cached(
        "UPDATE tracks SET cover_id = ?2, cover_index = ?3
         WHERE id = ?1 AND (cover_id IS NOT ?2 OR cover_index IS NOT ?3)",
    )?
    .execute(params![track, cover, index])?;
    let (folder, path) = match &covers.folder {
        Some(report) => {
            let path = match &report.origin {
                CoverOrigin::Folder { path } => Some(path_to_blob(path)),
                CoverOrigin::Embedded { .. } => None,
            };
            (Some(upsert_cover(c, report, file)?), path)
        }
        None => (None, None),
    };
    c.prepare_cached(
        "UPDATE dirs SET cover_id = ?2, cover_path = ?3
         WHERE id = ?1 AND (cover_id IS NOT ?2 OR cover_path IS NOT ?3)",
    )?
    .execute(params![dir, folder, path])?;
    Ok(())
}

fn cover_info(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<CoverInfo>> {
    let hash: Vec<u8> = row.get(0)?;
    let Some(hash) = CoverHash::from_bytes(&hash) else {
        return Ok(None);
    };
    let byte_len: Option<i64> = row.get(3)?;
    Ok(Some(CoverInfo {
        hash,
        width: row.get(1)?,
        height: row.get(2)?,
        byte_len: byte_len.and_then(|n| u64::try_from(n).ok()),
    }))
}

/// The cover row `id`.
fn cover_by_id(c: &Connection, id: Option<i64>) -> rusqlite::Result<Option<CoverInfo>> {
    let Some(id) = id else {
        return Ok(None);
    };
    Ok(
        c.prepare_cached("SELECT hash, width, height, byte_len FROM covers WHERE id = ?1")?
            .query_row([id], cover_info)
            .optional()?
            .flatten(),
    )
}

/// The cover of `album`.
pub(crate) fn album_cover(c: &Connection, album: AlbumId) -> rusqlite::Result<Option<CoverInfo>> {
    let id: Option<i64> = c
        .prepare_cached("SELECT cover_id FROM albums WHERE id = ?1")?
        .query_row([album.0], |row| row.get(0))
        .optional()?
        .flatten();
    cover_by_id(c, id)
}

/// The cover of `track`: its embedded picture, else its folder's image, else its
/// album's cover.
pub(crate) fn track_cover(c: &Connection, track: TrackId) -> rusqlite::Result<Option<CoverInfo>> {
    let id: Option<i64> = c
        .prepare_cached(
            "SELECT coalesce(t.cover_id, d.cover_id, a.cover_id)
             FROM tracks t JOIN files f ON f.id = t.file_id JOIN dirs d ON d.id = f.dir_id
             LEFT JOIN albums a ON a.id = t.album_id
             WHERE t.id = ?1",
        )?
        .query_row([sql_track_id(track)], |row| row.get(0))
        .optional()?
        .flatten();
    cover_by_id(c, id)
}

/// A `covers` row: id, source, source path and picture index.
type CoverRow = (i64, i64, Option<Vec<u8>>, Option<u32>);

/// The places where the cover with `hash` can be read: where it was first seen,
/// then other tracks and folders that have it. Empty when the library has no such
/// cover.
pub(crate) fn cover_locations(
    c: &Connection,
    hash: &CoverHash,
) -> rusqlite::Result<Vec<CoverLocation>> {
    let found: Option<CoverRow> = c
        .prepare_cached("SELECT id, source, source_path, source_index FROM covers WHERE hash = ?1")?
        .query_row([hash.as_bytes().as_slice()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;
    let Some((id, source, path, index)) = found else {
        return Ok(Vec::new());
    };
    let mut out: Vec<CoverLocation> = Vec::new();
    let mut push = |location: CoverLocation| {
        if !out.contains(&location) {
            out.push(location);
        }
    };
    if let Some(path) = path {
        let path = path_from_blob(path);
        push(match source {
            SOURCE_EMBEDDED => CoverLocation::Embedded {
                file: path,
                index: index.unwrap_or(0),
            },
            _ => CoverLocation::File { path },
        });
    }
    let mut tracks = c.prepare_cached(
        "SELECT d.path, f.name, t.cover_index FROM tracks t
         JOIN files f ON f.id = t.file_id JOIN dirs d ON d.id = f.dir_id
         WHERE t.cover_id = ?1 AND f.missing_since IS NULL
         GROUP BY f.id LIMIT ?2",
    )?;
    let rows = tracks.query_map(params![id, MAX_LOCATIONS], |row| {
        Ok((
            path_from_blob(row.get(0)?).join(path_from_blob(row.get(1)?)),
            row.get::<_, Option<u32>>(2)?,
        ))
    })?;
    for row in rows {
        let (file, index) = row?;
        push(CoverLocation::Embedded {
            file,
            index: index.unwrap_or(0),
        });
    }
    let mut dirs = c.prepare_cached(
        "SELECT DISTINCT cover_path FROM dirs WHERE cover_id = ?1 AND cover_path IS NOT NULL
         LIMIT ?2",
    )?;
    let rows = dirs.query_map(params![id, MAX_LOCATIONS], |row| row.get::<_, Vec<u8>>(0))?;
    for row in rows {
        push(CoverLocation::File {
            path: path_from_blob(row?),
        });
    }
    Ok(out)
}

/// The hashes of every cover in the library.
pub(crate) fn cover_hashes(c: &Connection) -> rusqlite::Result<HashSet<CoverHash>> {
    let mut statement = c.prepare("SELECT hash FROM covers")?;
    let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
    let mut out = HashSet::new();
    for row in rows {
        if let Some(hash) = CoverHash::from_bytes(&row?) {
            out.insert(hash);
        }
    }
    Ok(out)
}
