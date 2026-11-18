// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the cover manager needs to know about every album: where its files are,
//! which of them carry an embedded cover and which one, the image its folder has,
//! and its cover's size. Read in one pass over the tracks.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use helikodu_meta::covers::CoverHash;
use helikodu_model::library::TrackId;
use rusqlite::Connection;

use crate::Library;
use crate::catalog::ids::track_id_from_sql;
use crate::catalog::{AlbumId, CoverInfo};
use crate::db::{DbError, path_from_blob};

/// One album's covers, as the files and folders hold them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumCoverFacts {
    /// The album.
    pub album: AlbumId,
    /// The folders its tracks are in, in path order.
    pub folders: Vec<PathBuf>,
    /// Its tracks that are whole files, with their file, in path order. Only
    /// these get pictures written into them.
    pub files: Vec<(TrackId, PathBuf)>,
    /// The tracks in all, sections of a file included.
    pub tracks: usize,
    /// The tracks with an embedded cover.
    pub embedded: usize,
    /// The different embedded covers among the tracks.
    pub distinct_embedded: usize,
    /// The image file a folder of the album has, as the scanner found it.
    pub folder_image: Option<PathBuf>,
    /// The album's cover (embedded or from the folder), with its size.
    pub cover: Option<CoverInfo>,
    /// The most common embedded cover, with its size.
    pub embedded_cover: Option<CoverInfo>,
    /// The folder image, with its size.
    pub folder_cover: Option<CoverInfo>,
}

impl AlbumCoverFacts {
    /// Whether the files carry a cover.
    pub fn in_files(&self) -> bool {
        self.embedded > 0
    }

    /// Whether every track carries the same embedded cover (or none does).
    pub fn same_in_every_file(&self) -> bool {
        self.embedded == 0 || (self.embedded == self.tracks && self.distinct_embedded == 1)
    }
}

#[derive(Default)]
struct Gather {
    folders: Vec<PathBuf>,
    files: Vec<(TrackId, PathBuf)>,
    tracks: usize,
    embedded: HashMap<i64, usize>,
    with_embedded: usize,
    folder_image: Option<PathBuf>,
    folder_cover: Option<i64>,
    cover: Option<i64>,
}

fn read(c: &Connection) -> rusqlite::Result<Vec<AlbumCoverFacts>> {
    let mut covers: HashMap<i64, CoverInfo> = HashMap::new();
    {
        let mut stmt = c.prepare("SELECT id, hash, width, height, byte_len FROM covers")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: Vec<u8> = row.get(1)?;
            let Some(hash) = CoverHash::from_bytes(&hash) else {
                continue;
            };
            let byte_len: Option<i64> = row.get(4)?;
            covers.insert(
                row.get(0)?,
                CoverInfo {
                    hash,
                    width: row.get(2)?,
                    height: row.get(3)?,
                    byte_len: byte_len.and_then(|n| u64::try_from(n).ok()),
                },
            );
        }
    }
    let mut albums: BTreeMap<i64, Gather> = BTreeMap::new();
    let mut stmt = c.prepare(
        "SELECT t.album_id, t.id, t.sub_index, t.start_frame, d.path, f.name, t.cover_id,
                d.cover_path, a.cover_id, d.cover_id
         FROM tracks t
         JOIN files f ON f.id = t.file_id
         JOIN dirs d ON d.id = f.dir_id
         JOIN albums a ON a.id = t.album_id
         WHERE f.missing_since IS NULL
         ORDER BY d.path, f.name, t.sub_index",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let album: i64 = row.get(0)?;
        let track = track_id_from_sql(row.get(1)?);
        let sub_index: i64 = row.get(2)?;
        let start: Option<i64> = row.get(3)?;
        let dir = path_from_blob(row.get(4)?);
        let name = path_from_blob(row.get(5)?);
        let embedded: Option<i64> = row.get(6)?;
        let folder_image: Option<Vec<u8>> = row.get(7)?;
        let cover: Option<i64> = row.get(8)?;
        let folder_cover: Option<i64> = row.get(9)?;
        let g = albums.entry(album).or_default();
        g.tracks += 1;
        g.cover = cover;
        if !g.folders.contains(&dir) {
            g.folders.push(dir.clone());
        }
        if sub_index == 0 && start.is_none() {
            g.files.push((track, dir.join(name)));
        }
        if let Some(id) = embedded {
            g.with_embedded += 1;
            *g.embedded.entry(id).or_default() += 1;
        }
        if g.folder_image.is_none() && folder_image.is_some() {
            g.folder_image = folder_image.map(path_from_blob);
            g.folder_cover = folder_cover;
        }
    }
    Ok(albums
        .into_iter()
        .map(|(album, g)| {
            let common = g
                .embedded
                .iter()
                .max_by_key(|(id, n)| (**n, -**id))
                .map(|(id, _)| *id);
            AlbumCoverFacts {
                album: AlbumId::from_raw(album),
                folders: g.folders,
                files: g.files,
                tracks: g.tracks,
                embedded: g.with_embedded,
                distinct_embedded: g.embedded.len(),
                folder_image: g.folder_image,
                cover: g.cover.and_then(|id| covers.get(&id).cloned()),
                embedded_cover: common.and_then(|id| covers.get(&id).cloned()),
                folder_cover: g.folder_cover.and_then(|id| covers.get(&id).cloned()),
            }
        })
        .collect())
}

impl Library {
    /// The covers of every album whose files are present, by album id.
    pub fn album_cover_facts(&self) -> Result<Vec<AlbumCoverFacts>, DbError> {
        self.database().read(read)
    }
}
