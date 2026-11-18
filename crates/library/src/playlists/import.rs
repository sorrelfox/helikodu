// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Matching the entries of a playlist file to tracks of the library.
//!
//! In order:
//! 1. **Path.** The entry's path (and, for a file in a legacy charset, the path made of
//!    its original bytes) is a file of the library.
//! 2. **End of the path.** The file name and the folders above it, compared without
//!    regard to ASCII case as Windows does, match exactly one file of the library on at
//!    least the name and its folder; or the name alone matches exactly one file. This
//!    finds the files of a playlist made on another system (`C:\Music\…`) or before
//!    the library moved.
//! 3. **Tags.** The title (folded), the artist when given (among the track's folded
//!    artists) and the length when known (within 3 s) match exactly one track.
//!
//! An entry that matches nothing, and every URL, becomes a dead item that keeps the
//! path or URL and the tags. The file name and title tables are read only when an
//! entry needs them, once per import.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use helikodu_meta::playlists::{EntryLocation, ParsedPlaylist, PlaylistEntry};
use rusqlite::{Connection, OptionalExtension};

use super::{ImportReport, NewItem, PlaylistId, REF_FOREIGN, REF_PATH, REF_URL};
use crate::catalog::write::file_at;
use crate::db::{display_text, path_to_blob};
use crate::search::fold;

/// The items for the entries of `parsed`, and the report of an import into a playlist
/// named `name` (whose id is not known yet).
pub(crate) fn match_entries(
    c: &Connection,
    parsed: &ParsedPlaylist,
    name: String,
) -> rusqlite::Result<(Vec<NewItem>, ImportReport)> {
    let mut matcher = Matcher::default();
    let mut report = ImportReport {
        playlist: PlaylistId(0),
        name,
        kind: parsed.kind,
        encoding: parsed.encoding,
        entries: parsed.playlist.entries.len(),
        by_path: 0,
        by_name: 0,
        by_tags: 0,
        dead: 0,
        unsupported: 0,
        warnings: parsed.warnings.clone(),
    };
    let mut items = Vec::with_capacity(parsed.playlist.entries.len());
    for entry in &parsed.playlist.entries {
        let (item, found) = matcher.item(c, entry)?;
        match found {
            Found::Path => report.by_path += 1,
            Found::Name => report.by_name += 1,
            Found::Tags => report.by_tags += 1,
            Found::Nothing => report.dead += 1,
        }
        if item.ref_kind == REF_URL {
            report.unsupported += 1;
        }
        items.push(item);
    }
    Ok((items, report))
}

/// How an entry was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    Path,
    Name,
    Tags,
    Nothing,
}

/// A file of the library, for matching by the end of its path.
#[derive(Debug)]
struct FileEntry {
    /// The folders of its path, lower-cased in ASCII, from the root.
    folders: Vec<Vec<u8>>,
    identity: i64,
}

/// A track of the library, for matching by its tags.
#[derive(Debug)]
struct TitleEntry {
    identity: i64,
    duration_ms: Option<i64>,
    artists_fold: Option<String>,
}

/// Finds the tracks a playlist's entries stand for.
#[derive(Debug, Default)]
struct Matcher {
    by_name: Option<HashMap<Vec<u8>, Vec<FileEntry>>>,
    by_title: Option<HashMap<String, Vec<TitleEntry>>>,
}

/// The longest difference in length between an entry and a track matched by tags.
const LENGTH_TOLERANCE_MS: i64 = 3000;

impl Matcher {
    /// The item for `entry`, and how its track was found.
    fn item(
        &mut self,
        c: &Connection,
        entry: &PlaylistEntry,
    ) -> rusqlite::Result<(NewItem, Found)> {
        let hints = &entry.hints;
        let mut item = NewItem {
            title: hints.title.clone(),
            artist: hints.artist.clone(),
            album: hints.album.clone(),
            duration_ms: hints
                .duration
                .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
            ..NewItem::default()
        };
        let mut found = Found::Nothing;
        match &entry.location {
            EntryLocation::Remote(url) => {
                item.ref_kind = REF_URL;
                item.reference = Some(url.as_bytes().to_vec());
                // Never fetched, never matched.
                return Ok((item, found));
            }
            EntryLocation::Local(path) => {
                item.ref_kind = REF_PATH;
                item.reference = Some(path_to_blob(path).to_vec());
                for path in entry.local_paths() {
                    if let Some(identity) = by_path(c, path)? {
                        item.identity = Some(identity);
                        found = Found::Path;
                        break;
                    }
                }
                if item.identity.is_none() {
                    item.identity = self.by_end(c, path.as_os_str().as_bytes())?;
                    if item.identity.is_none()
                        && let Some(undecoded) = &entry.undecoded_path
                    {
                        item.identity = self.by_end(c, undecoded.as_os_str().as_bytes())?;
                    }
                    if item.identity.is_some() {
                        found = Found::Name;
                    }
                }
            }
            EntryLocation::Foreign(path) => {
                item.ref_kind = REF_FOREIGN;
                item.reference = Some(path.clone());
                item.identity = self.by_end(c, path)?;
                if item.identity.is_some() {
                    found = Found::Name;
                }
            }
        }
        if item.identity.is_none() {
            item.identity = self.by_tags(c, &item)?;
            if item.identity.is_some() {
                found = Found::Tags;
            }
        }
        Ok((item, found))
    }

    /// The identity of the one file whose path ends like `path`.
    fn by_end(&mut self, c: &Connection, path: &[u8]) -> rusqlite::Result<Option<i64>> {
        let wanted: Vec<Vec<u8>> = path
            .split(|&b| b == b'/')
            .filter(|part| !part.is_empty())
            .map(<[u8]>::to_ascii_lowercase)
            .collect();
        let Some((name, folders)) = wanted.split_last() else {
            return Ok(None);
        };
        let table = match &mut self.by_name {
            Some(table) => table,
            None => self.by_name.insert(file_table(c)?),
        };
        let Some(candidates) = table.get(name) else {
            return Ok(None);
        };
        if let [only] = candidates.as_slice() {
            return Ok(Some(only.identity));
        }
        // The number of folders that agree, from the file upwards.
        let score = |candidate: &FileEntry| {
            candidate
                .folders
                .iter()
                .rev()
                .zip(folders.iter().rev())
                .take_while(|(a, b)| a == b)
                .count()
        };
        let best = candidates.iter().map(score).max().unwrap_or(0);
        let mut winners = candidates.iter().filter(|c| score(c) == best);
        match (winners.next(), winners.next()) {
            (Some(winner), None) if best >= 1 => Ok(Some(winner.identity)),
            _ => Ok(None),
        }
    }

    /// The identity of the one track whose tags match `item`'s.
    fn by_tags(&mut self, c: &Connection, item: &NewItem) -> rusqlite::Result<Option<i64>> {
        let Some(title) = item.title.as_deref() else {
            return Ok(None);
        };
        let table = match &mut self.by_title {
            Some(table) => table,
            None => self.by_title.insert(title_table(c)?),
        };
        let Some(candidates) = table.get(&fold(title)) else {
            return Ok(None);
        };
        let artist = item.artist.as_deref().map(fold);
        let mut matching = candidates.iter().filter(|candidate| {
            let artist_ok = match (&artist, &candidate.artists_fold) {
                (None, _) => true,
                (Some(wanted), Some(known)) => known.split('\u{1f}').any(|name| name == wanted),
                (Some(_), None) => false,
            };
            let length_ok = match (item.duration_ms, candidate.duration_ms) {
                (Some(a), Some(b)) => (a - b).abs() <= LENGTH_TOLERANCE_MS,
                _ => true,
            };
            artist_ok && length_ok
        });
        match (matching.next(), matching.next()) {
            (Some(only), None) => Ok(Some(only.identity)),
            _ => Ok(None),
        }
    }
}

/// The identity of the first track of the file at `path`.
fn by_path(c: &Connection, path: &Path) -> rusqlite::Result<Option<i64>> {
    let Some(file) = file_at(c, path)? else {
        return Ok(None);
    };
    c.prepare_cached(
        "SELECT identity_id FROM tracks WHERE file_id = ?1 ORDER BY sub_index, id LIMIT 1",
    )?
    .query_row([file], |row| row.get(0))
    .optional()
}

/// The files that have tracks, by their lower-cased name.
fn file_table(c: &Connection) -> rusqlite::Result<HashMap<Vec<u8>, Vec<FileEntry>>> {
    let mut table: HashMap<Vec<u8>, Vec<FileEntry>> = HashMap::new();
    let mut query = c.prepare(
        "SELECT d.path, f.name,
                (SELECT t.identity_id FROM tracks t WHERE t.file_id = f.id
                 ORDER BY t.sub_index, t.id LIMIT 1)
         FROM files f JOIN dirs d ON d.id = f.dir_id
         WHERE EXISTS (SELECT 1 FROM tracks t WHERE t.file_id = f.id)",
    )?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        let dir: Vec<u8> = row.get(0)?;
        let name: Vec<u8> = row.get(1)?;
        let folders = dir
            .split(|&b| b == b'/')
            .filter(|part| !part.is_empty())
            .map(<[u8]>::to_ascii_lowercase)
            .collect();
        table
            .entry(name.to_ascii_lowercase())
            .or_default()
            .push(FileEntry {
                folders,
                identity: row.get(2)?,
            });
    }
    Ok(table)
}

/// The tracks with a title, by their folded title.
fn title_table(c: &Connection) -> rusqlite::Result<HashMap<String, Vec<TitleEntry>>> {
    let mut table: HashMap<String, Vec<TitleEntry>> = HashMap::new();
    let mut query = c.prepare(
        "SELECT title_fold, identity_id, duration_ms, artists_fold FROM tracks
         WHERE title_fold IS NOT NULL AND title_fold != ''",
    )?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        table.entry(row.get(0)?).or_default().push(TitleEntry {
            identity: row.get(1)?,
            duration_ms: row.get(2)?,
            artists_fold: row.get(3)?,
        });
    }
    Ok(table)
}

/// The name of an imported playlist: the one asked for, the file's title, or the
/// file's name without its extension.
pub(crate) fn imported_name(asked: Option<&str>, title: Option<&str>, path: &Path) -> String {
    [asked, title]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            path.file_stem()
                .map(|stem| display_text(OsStr::new(stem)).trim().to_owned())
                .filter(|stem| !stem.is_empty())
        })
        .unwrap_or_else(|| "Imported playlist".to_owned())
}
