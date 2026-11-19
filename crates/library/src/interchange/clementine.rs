// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Strawberry and Clementine: their SQLite database, read from a copy.
//!
//! The `songs` table gives the location (`url` in Strawberry, `filename` in
//! Clementine and old Strawberry versions), `title`, `artist`, `album`, `track`,
//! `length` (nanoseconds), `playcount`, `skipcount`, `lastplayed` (seconds since
//! the epoch; -1 for never) and `rating` (0.0 to 1.0; -1 for not rated). The
//! playlists come from `playlists` and `playlist_items` (in the order the items
//! were added); an item names a library song (`collection_id` or `library_id`) or
//! a location. Smart (dynamic) playlists keep their rules in a binary form and
//! are not imported.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use super::ImportError;
use super::foreign::{
    ForeignItem, ForeignLibrary, ForeignPlaylist, ForeignPlaylistKind, ForeignTrack, Player,
};
use super::sqlite_copy::{DatabaseCopy, columns, format_error};
use crate::db::unix_time::from_unix_seconds;
use crate::stats::Rating;

/// Reads the database at `path` of `player` (Strawberry or Clementine).
pub(super) fn read(path: &Path, player: Player) -> Result<ForeignLibrary, ImportError> {
    let copy = DatabaseCopy::open(path)?;
    let c = copy.connection();
    let songs = columns(c, "songs").map_err(|e| format_error(path, &e))?;
    let Some(location) = ["url", "filename"]
        .into_iter()
        .find(|n| songs.iter().any(|c| c == n))
    else {
        return Err(ImportError::Format {
            path: path.to_path_buf(),
            reason: format!("not a {} database: it has no songs table", player.name()),
        });
    };
    read_from(c, location).map_err(|e| format_error(path, &e))
}

/// A rating of 0.0 to 1.0; a negative value is not rated.
fn fraction(value: Option<f64>) -> Option<Rating> {
    let value = value.filter(|v| v.is_finite() && *v > 0.0)?;
    Rating::from_half_stars((value.min(1.0) * 10.0).round() as u8)
}

fn time(seconds: Option<i64>) -> Option<std::time::SystemTime> {
    seconds.filter(|&s| s > 0).map(from_unix_seconds)
}

fn text(value: Option<String>) -> Option<String> {
    value.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty())
}

fn nanoseconds(value: Option<i64>) -> Option<Duration> {
    value
        .and_then(|n| u64::try_from(n).ok())
        .filter(|&n| n > 0)
        .map(Duration::from_nanos)
}

/// The location column as text; Clementine stores it as a BLOB in some versions.
fn location(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<String>> {
    use rusqlite::types::ValueRef;
    Ok(match row.get_ref(index)? {
        ValueRef::Text(t) | ValueRef::Blob(t) => {
            text(Some(String::from_utf8_lossy(t).into_owned()))
        }
        _ => None,
    })
}

fn read_from(c: &Connection, location_column: &str) -> rusqlite::Result<ForeignLibrary> {
    let mut library = ForeignLibrary::default();
    let mut by_id: HashMap<i64, usize> = HashMap::new();
    let sql = format!(
        "SELECT ROWID, {location_column}, title, artist, album, track, length, playcount,
                skipcount, lastplayed, rating
         FROM songs ORDER BY ROWID"
    );
    let mut statement = c.prepare(&sql)?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        by_id.insert(row.get(0)?, library.tracks.len());
        library.tracks.push(ForeignTrack {
            location: location(row, 1)?,
            title: text(row.get(2)?),
            artist: text(row.get(3)?),
            album: text(row.get(4)?),
            track_number: row
                .get::<_, Option<i64>>(5)?
                .and_then(|n| u32::try_from(n).ok())
                .filter(|&n| n > 0),
            duration: nanoseconds(row.get(6)?),
            play_count: row
                .get::<_, Option<i64>>(7)?
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0),
            skip_count: row
                .get::<_, Option<i64>>(8)?
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0),
            last_played: time(row.get(9)?),
            rating: fraction(row.get(10)?),
            ..ForeignTrack::default()
        });
    }
    read_playlists(c, &by_id, &mut library)?;
    Ok(library)
}

fn read_playlists(
    c: &Connection,
    by_id: &HashMap<i64, usize>,
    library: &mut ForeignLibrary,
) -> rusqlite::Result<()> {
    let lists = columns(c, "playlists")?;
    let items = columns(c, "playlist_items")?;
    if lists.is_empty() || items.is_empty() {
        return Ok(());
    }
    let has = |cols: &[String], name: &str| cols.iter().any(|c| c == name);
    let dynamic = if has(&lists, "dynamic_playlist_type") {
        "coalesce(dynamic_playlist_type, '') != '' AND dynamic_playlist_type != '0'"
    } else {
        "0"
    };
    let special = if has(&lists, "special_type") {
        "coalesce(special_type, '') != ''"
    } else {
        "0"
    };
    let id_column = ["collection_id", "library_id"]
        .into_iter()
        .find(|n| has(&items, n))
        .unwrap_or("NULL");
    let location_column = ["url", "filename"]
        .into_iter()
        .find(|n| has(&items, n))
        .unwrap_or("NULL");
    let optional = |name: &'static str| if has(&items, name) { name } else { "NULL" };
    let mut playlists = c.prepare(&format!(
        "SELECT ROWID, name, {dynamic}, {special} FROM playlists ORDER BY ROWID"
    ))?;
    let mut entries = c.prepare(&format!(
        "SELECT {id_column}, {location_column}, {}, {}, {}, {} FROM playlist_items
         WHERE playlist = ?1 ORDER BY ROWID",
        optional("title"),
        optional("artist"),
        optional("album"),
        optional("length"),
    ))?;
    let mut rows = playlists.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let Some(name) = text(row.get(1)?) else {
            continue;
        };
        let is_dynamic: bool = row.get(2)?;
        let is_special: bool = row.get(3)?;
        if is_special {
            continue;
        }
        let kind = if is_dynamic {
            ForeignPlaylistKind::Unmapped(
                "its rules are stored in a form that cannot be read".to_owned(),
            )
        } else {
            let mut list = Vec::new();
            let mut item_rows = entries.query([id])?;
            while let Some(item) = item_rows.next()? {
                let song: Option<i64> = item.get(0)?;
                match song.filter(|&s| s > 0).and_then(|s| by_id.get(&s)) {
                    Some(&index) => list.push(ForeignItem::Track(index)),
                    None => list.push(ForeignItem::Other(Box::new(ForeignTrack {
                        location: location(item, 1)?,
                        title: text(item.get(2)?),
                        artist: text(item.get(3)?),
                        album: text(item.get(4)?),
                        duration: nanoseconds(item.get(5)?),
                        ..ForeignTrack::default()
                    }))),
                }
            }
            ForeignPlaylistKind::Static {
                items: list,
                from_rules: false,
            }
        };
        library.playlists.push(ForeignPlaylist {
            name,
            folder: Vec::new(),
            kind,
        });
    }
    Ok(())
}
