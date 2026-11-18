// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the Library statistics view counts beyond [`Library::totals`]: the tracks
//! added lately, albums without a cover, the formats and their quality, listening
//! since a date, and the most played artists and albums.

use std::time::{Duration, SystemTime};

use helikodu_meta::covers::CoverHash;
use rusqlite::Connection;

use crate::Library;
use crate::catalog::{AlbumId, LOSSLESS, PersonId};
use crate::db::DbError;
use crate::db::unix_time::{self, to_unix_seconds};

/// The counts of the statistics view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryStatistics {
    /// Tracks added since the date asked for (the start of the month).
    pub added_since: usize,
    /// Albums whose tracks have no cover.
    pub albums_without_cover: usize,
    /// The tracks of each format (the codec in lower case, `None` when unknown), most
    /// tracks first.
    pub formats: Vec<(Option<String>, usize)>,
    /// Tracks in a lossless format at up to 48 kHz and 16 bits.
    pub lossless_cd: usize,
    /// Tracks in a lossless format above 48 kHz or 16 bits.
    pub lossless_hi_res: usize,
    /// Tracks in a lossy format (or of an unknown one).
    pub lossy: usize,
    /// Plays counted since the date of the listening figures.
    pub plays: u64,
    /// The time listened since then.
    pub listened: Duration,
    /// Skips of the tracks last skipped since then.
    pub skips: u64,
    /// Tracks that were never played.
    pub never_played: usize,
    /// The most played track artists, most plays first.
    pub top_artists: Vec<PlayedArtist>,
    /// The most played albums, most plays first.
    pub top_albums: Vec<PlayedAlbum>,
}

/// An artist with the plays of their tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayedArtist {
    /// The artist.
    pub person: PersonId,
    /// Their name.
    pub name: String,
    /// Plays of their tracks together.
    pub plays: u64,
}

/// An album with the plays of its tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayedAlbum {
    /// The album.
    pub album: AlbumId,
    /// Its title.
    pub title: Option<String>,
    /// Its album artist as credited.
    pub artist: Option<String>,
    /// Its cover.
    pub cover: Option<CoverHash>,
    /// Plays of its tracks together.
    pub plays: u64,
}

/// What [`Library::statistics`] counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatisticsRequest {
    /// Tracks added since then count as added lately.
    pub added_since: SystemTime,
    /// The listening figures count from then.
    pub listening_since: SystemTime,
    /// How many artists and albums the lists of the most played hold.
    pub top: usize,
}

impl Library {
    /// The counts of the statistics view.
    pub fn statistics(&self, request: &StatisticsRequest) -> Result<LibraryStatistics, DbError> {
        self.database().read(|c| statistics(c, request))
    }
}

fn to_count(n: i64) -> usize {
    usize::try_from(n).unwrap_or(0)
}

fn statistics(c: &Connection, request: &StatisticsRequest) -> rusqlite::Result<LibraryStatistics> {
    let shown = crate::copies::settings::hidden_copies_condition(c)?.unwrap_or("1");
    let added = to_unix_seconds(request.added_since);
    let since = to_unix_seconds(request.listening_since);
    let top = i64::try_from(request.top).unwrap_or(i64::MAX);
    let count = |sql: &str, param: Option<i64>| -> rusqlite::Result<i64> {
        let mut statement = c.prepare(sql)?;
        match param {
            Some(p) => statement.query_row([p], |row| row.get(0)),
            None => statement.query_row([], |row| row.get(0)),
        }
    };
    let added_since = count(
        &format!("SELECT count(*) FROM tracks t WHERE {shown} AND t.added_at >= ?1"),
        Some(added),
    )?;
    let albums_without_cover = count(
        &format!(
            "SELECT count(*) FROM albums a WHERE a.cover_id IS NULL
             AND EXISTS (SELECT 1 FROM tracks t WHERE t.album_id = a.id AND {shown})"
        ),
        None,
    )?;
    let formats = c
        .prepare(&format!(
            "SELECT lower(t.codec), count(*) FROM tracks t WHERE {shown}
             GROUP BY lower(t.codec) ORDER BY 2 DESC, 1"
        ))?
        .query_map([], |row| Ok((row.get(0)?, to_count(row.get(1)?))))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (lossless_cd, lossless_hi_res, lossy): (i64, i64, i64) = c
        .prepare(&format!(
            "SELECT
                 coalesce(sum(CASE WHEN t.codec IS NOT NULL AND {LOSSLESS}
                     AND coalesce(t.sample_rate, 0) <= 48000
                     AND coalesce(t.bit_depth, 0) <= 16 THEN 1 ELSE 0 END), 0),
                 coalesce(sum(CASE WHEN t.codec IS NOT NULL AND {LOSSLESS}
                     AND (coalesce(t.sample_rate, 0) > 48000
                          OR coalesce(t.bit_depth, 0) > 16) THEN 1 ELSE 0 END), 0),
                 coalesce(sum(CASE WHEN t.codec IS NOT NULL AND {LOSSLESS}
                     THEN 0 ELSE 1 END), 0)
             FROM tracks t WHERE {shown}"
        ))?
        .query_row([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let (plays, listened_ms): (i64, i64) = c
        .prepare(
            "SELECT count(*) FILTER (WHERE counted = 1), coalesce(sum(played_ms), 0)
             FROM history WHERE started_at >= ?1",
        )?
        .query_row([since], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let skips = count(
        "SELECT coalesce(sum(skip_count), 0) FROM stats WHERE last_skipped >= ?1",
        Some(since),
    )?;
    let never_played = count(
        &format!(
            "SELECT count(*) FROM tracks t LEFT JOIN stats s ON s.identity_id = t.identity_id
             WHERE {shown} AND coalesce(s.play_count, 0) = 0"
        ),
        None,
    )?;
    let top_artists = c
        .prepare(&format!(
            "SELECT p.id, p.name, sum(s.play_count) AS plays
             FROM track_people tp
             JOIN tracks t ON t.id = tp.track_id
             JOIN stats s ON s.identity_id = t.identity_id
             JOIN people p ON p.id = tp.person_id
             WHERE tp.role = 0 AND s.play_count > 0 AND {shown}
             GROUP BY p.id ORDER BY plays DESC, p.sort_name, p.id LIMIT ?1"
        ))?
        .query_map([top], |row| {
            Ok(PlayedArtist {
                person: PersonId(row.get(0)?),
                name: row.get(1)?,
                plays: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let top_albums = c
        .prepare(&format!(
            "SELECT a.id, a.title, a.artist,
                    (SELECT hash FROM covers WHERE id = a.cover_id),
                    sum(s.play_count) AS plays
             FROM tracks t
             JOIN albums a ON a.id = t.album_id
             JOIN stats s ON s.identity_id = t.identity_id
             WHERE s.play_count > 0 AND {shown}
             GROUP BY a.id ORDER BY plays DESC, a.title_sort, a.id LIMIT ?1"
        ))?
        .query_map([top], |row| {
            Ok(PlayedAlbum {
                album: AlbumId(row.get(0)?),
                title: row.get(1)?,
                artist: row.get(2)?,
                cover: row
                    .get::<_, Option<Vec<u8>>>(3)?
                    .and_then(|hash| CoverHash::from_bytes(&hash)),
                plays: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(LibraryStatistics {
        added_since: to_count(added_since),
        albums_without_cover: to_count(albums_without_cover),
        formats,
        lossless_cd: to_count(lossless_cd),
        lossless_hi_res: to_count(lossless_hi_res),
        lossy: to_count(lossy),
        plays: u64::try_from(plays).unwrap_or(0),
        listened: unix_time::from_millis(listened_ms),
        skips: u64::try_from(skips).unwrap_or(0),
        never_played: to_count(never_played),
        top_artists,
        top_albums,
    })
}
