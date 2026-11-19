// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lollypop: `lollypop.db` and the `playlists.db` beside it, read from copies.
//!
//! The `tracks` table gives `uri`, `name` (the title), `tracknumber`, `duration`
//! (milliseconds; seconds in old versions), `popularity` (a play count),
//! `rate` (0 to 5 stars; 0 or less is not rated), `loved` (1 for loved) and
//! `ltime` (the last play, in seconds since the epoch); the album and the
//! artists come from `albums`, `track_artists` and `artists`. Lollypop keeps no
//! skip count. Static playlists list URIs; smart playlists hold SQL for
//! Lollypop's own database and are not imported.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use super::foreign::{ForeignItem, ForeignLibrary, ForeignPlaylist, ForeignPlaylistKind};
use super::sqlite_copy::{DatabaseCopy, columns, format_error};
use super::{ForeignTrack, ImportError};
use crate::db::unix_time::from_unix_seconds;
use crate::stats::Rating;

/// Lengths above this many are taken as milliseconds, not seconds (a recording
/// of more than 5½ hours in seconds is rarer than one of 20 s in milliseconds).
const MILLISECONDS_ABOVE: f64 = 20_000.0;

/// Reads `lollypop.db` at `path` and the `playlists.db` beside it.
pub(super) fn read(path: &Path) -> Result<ForeignLibrary, ImportError> {
    let copy = DatabaseCopy::open(path)?;
    let c = copy.connection();
    let tracks = columns(c, "tracks").map_err(|e| format_error(path, &e))?;
    if !tracks.iter().any(|c| c == "popularity") {
        return Err(ImportError::Format {
            path: path.to_path_buf(),
            reason: "not a Lollypop database: it has no tracks with a popularity".to_owned(),
        });
    }
    let (mut library, by_uri) = read_tracks(c).map_err(|e| format_error(path, &e))?;
    let playlists = path.with_file_name("playlists.db");
    if playlists.is_file() {
        let copy = DatabaseCopy::open(&playlists)?;
        read_playlists(copy.connection(), &by_uri, &mut library)
            .map_err(|e| format_error(&playlists, &e))?;
    }
    Ok(library)
}

fn text(value: Option<String>) -> Option<String> {
    value.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty())
}

type ByUri = HashMap<String, usize>;

fn read_tracks(c: &Connection) -> rusqlite::Result<(ForeignLibrary, ByUri)> {
    let mut library = ForeignLibrary::default();
    let mut by_uri = HashMap::new();
    let artists = !columns(c, "track_artists")?.is_empty() && !columns(c, "artists")?.is_empty();
    let artist = if artists {
        "(SELECT group_concat(name, ', ') FROM (SELECT a.name FROM track_artists ta
            JOIN artists a ON a.id = ta.artist_id WHERE ta.track_id = t.id ORDER BY ta.rowid))"
    } else {
        "NULL"
    };
    let album = if columns(c, "albums")?.is_empty() {
        "NULL"
    } else {
        "(SELECT name FROM albums WHERE id = t.album_id)"
    };
    let mut statement = c.prepare(&format!(
        "SELECT t.uri, t.name, {artist}, {album}, t.tracknumber, t.duration, t.popularity,
                t.rate, t.loved, t.ltime
         FROM tracks t ORDER BY t.id"
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let uri = text(row.get(0)?);
        let duration = row
            .get::<_, Option<f64>>(5)?
            .filter(|d| d.is_finite() && *d > 0.0)
            .map(|d| {
                if d > MILLISECONDS_ABOVE {
                    Duration::from_millis(d as u64)
                } else {
                    Duration::from_secs_f64(d)
                }
            });
        let track = ForeignTrack {
            location: uri.clone(),
            title: text(row.get(1)?),
            artist: text(row.get(2)?),
            album: text(row.get(3)?),
            track_number: row
                .get::<_, Option<i64>>(4)?
                .and_then(|n| u32::try_from(n).ok())
                .filter(|&n| n > 0),
            duration,
            play_count: row
                .get::<_, Option<i64>>(6)?
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0),
            rating: row
                .get::<_, Option<i64>>(7)?
                .filter(|&r| r > 0)
                .and_then(|r| u8::try_from(r.min(5)).ok())
                .and_then(Rating::from_stars),
            loved: row.get::<_, Option<i64>>(8)? == Some(1),
            last_played: row
                .get::<_, Option<i64>>(9)?
                .filter(|&s| s > 0)
                .map(from_unix_seconds),
            ..ForeignTrack::default()
        };
        if let Some(uri) = uri {
            by_uri.insert(uri, library.tracks.len());
        }
        library.tracks.push(track);
    }
    Ok((library, by_uri))
}

fn read_playlists(
    c: &Connection,
    by_uri: &ByUri,
    library: &mut ForeignLibrary,
) -> rusqlite::Result<()> {
    let lists = columns(c, "playlists")?;
    if lists.is_empty() || columns(c, "tracks")?.is_empty() {
        return Ok(());
    }
    let smart = if lists.iter().any(|c| c == "smart_enabled") {
        "coalesce(smart_enabled, 0)"
    } else {
        "0"
    };
    let mut playlists = c.prepare(&format!(
        "SELECT id, name, {smart} FROM playlists ORDER BY id"
    ))?;
    let mut entries = c.prepare("SELECT uri FROM tracks WHERE playlist_id = ?1 ORDER BY rowid")?;
    let mut rows = playlists.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let Some(name) = text(row.get(1)?) else {
            continue;
        };
        let is_smart: bool = row.get::<_, i64>(2)? != 0;
        let kind = if is_smart {
            ForeignPlaylistKind::Unmapped(
                "its rules are SQL for Lollypop's own database".to_owned(),
            )
        } else {
            let mut items = Vec::new();
            let mut uris = entries.query([id])?;
            while let Some(uri) = uris.next()? {
                let Some(uri) = text(uri.get(0)?) else {
                    continue;
                };
                items.push(match by_uri.get(&uri) {
                    Some(&i) => ForeignItem::Track(i),
                    None => ForeignItem::Other(Box::new(ForeignTrack {
                        location: Some(uri),
                        ..ForeignTrack::default()
                    })),
                });
            }
            ForeignPlaylistKind::Static {
                items,
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
