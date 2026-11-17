// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Details of single files and tracks, and totals of the whole library.
//!
//! - [`read_file_details`] reads a file's tags in this process, without the library:
//!   for a file that is not in it, and for the loudness tags of the files the player
//!   plays.
//! - [`Library::track_details`] is what the library holds about one track.
//! - [`Library::roots`], [`Library::totals`] and [`Library::most_played`] describe the
//!   library as a whole.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use helikodu_meta::tags::{self as meta_tags, ReadOptions, Role};
use helikodu_model::library::TrackId;
use helikodu_model::player::Loudness;
use rusqlite::{Connection, OptionalExtension};

use crate::Library;
use crate::catalog::{
    AudioProperties, IdentityId, TrackKind, TrackTags, sql_track_id, track_id_from_sql,
};
use crate::db::{DbError, path_from_blob, unix_time};
use crate::scan::store::STATE_UNREADABLE;
use crate::stats::Rating;

/// What a file's own tags say, read from the file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileDetails {
    /// The tags, mapped as the scanner maps them for the library.
    pub tags: TrackTags,
    /// The audio properties.
    pub audio: AudioProperties,
    /// The ReplayGain or Opus R128 values, relative to −18 LUFS.
    pub loudness: Loudness,
    /// The rating in the tags.
    pub rating: Option<Rating>,
}

/// Why a file's tags could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ReadFileError(String);

/// Reads the tags of the file at `path` in this process, with the default options
/// (legacy charsets repaired and guessed).
///
/// Scans read files in isolated worker processes instead; this is for single files the
/// user named or is about to play.
pub fn read_file_details(path: &Path) -> Result<FileDetails, ReadFileError> {
    let file = meta_tags::read_tags(path, &ReadOptions::default())
        .map_err(|error| ReadFileError(error.to_string()))?;
    let gain = &file.tags.replay_gain;
    Ok(FileDetails {
        tags: track_tags(&file.tags),
        audio: audio_properties(&file.audio),
        loudness: Loudness {
            track_gain_db: gain.track_gain_rg2_db(),
            track_peak: gain.track_peak,
            album_gain_db: gain.album_gain_rg2_db(),
            album_peak: gain.album_peak,
        },
        rating: file
            .tags
            .rating
            .and_then(|rating| Rating::from_half_stars(rating.half_stars())),
    })
}

/// The same mapping as the scanner's: a credited artist without a list of artists
/// stands for the list.
fn track_tags(tags: &meta_tags::TrackTags) -> TrackTags {
    let list_or = |list: &Vec<String>, single: &Option<String>| {
        if list.is_empty() {
            single.iter().cloned().collect()
        } else {
            list.clone()
        }
    };
    TrackTags {
        title: tags.title.clone(),
        title_sort: tags.title_sort.clone(),
        artists: list_or(&tags.artists, &tags.artist),
        album_artists: list_or(&tags.album_artists, &tags.album_artist),
        composers: tags
            .credits_with(Role::Composer)
            .map(|credit| credit.name.clone())
            .collect(),
        album: tags.album.clone(),
        genres: tags.genres.clone(),
        track_no: tags.track_number,
        disc_no: tags.disc_number,
        date: tags.date.clone(),
        year: tags.year(),
    }
}

fn audio_properties(audio: &meta_tags::AudioProperties) -> AudioProperties {
    AudioProperties {
        duration: audio.duration,
        sample_rate: audio.sample_rate,
        channels: audio.channels,
        bit_depth: audio.bit_depth.and_then(|bits| u16::try_from(bits).ok()),
        bitrate: audio.bitrate_kbps.map(|kbps| kbps.saturating_mul(1000)),
        codec: audio.codec.clone(),
        container: audio.container.clone(),
    }
}

/// What the library holds about one track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackDetails {
    /// The track.
    pub track: TrackId,
    /// Its identity, which its statistics belong to.
    pub identity: IdentityId,
    /// Its file, as the bytes the kernel returned.
    pub path: PathBuf,
    /// The CUE track or subsong number, or 0 for a whole file.
    pub sub_index: u32,
    /// What kind of recording it is.
    pub kind: TrackKind,
    /// The tags as stored.
    pub tags: TrackTags,
    /// The audio properties as stored.
    pub audio: AudioProperties,
    /// When the track entered the library.
    pub added_at: SystemTime,
    /// When its file disappeared, if it did.
    pub missing_since: Option<SystemTime>,
}

/// Totals of the whole library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryTotals {
    /// Library folders.
    pub roots: usize,
    /// Media files with at least one track (CUE sheets are not counted).
    pub files: usize,
    /// Tracks.
    pub tracks: usize,
    /// Albums with at least one track.
    pub albums: usize,
    /// Track artists and album artists.
    pub artists: usize,
    /// Genres with at least one track.
    pub genres: usize,
    /// The length of all tracks together.
    pub duration: Duration,
    /// The size of the media files together, in bytes.
    pub size: u64,
    /// Files that disappeared and have not come back.
    pub missing_files: usize,
    /// Files that could not be read.
    pub unreadable_files: usize,
    /// Plays of all tracks together.
    pub plays: u64,
    /// Tracks played at least once.
    pub played_tracks: usize,
    /// Tracks with a rating.
    pub rated_tracks: usize,
}

/// A track with its play count, as [`Library::most_played`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayedTrack {
    /// The track.
    pub track: TrackId,
    /// Its title.
    pub title: Option<String>,
    /// Its artists, separated by commas.
    pub artist: Option<String>,
    /// Its album title.
    pub album: Option<String>,
    /// How often it was played.
    pub play_count: u64,
    /// When it was last played.
    pub last_played: Option<SystemTime>,
}

impl Library {
    /// What the library holds about `track`, or `None` if there is no such track.
    pub fn track_details(&self, track: TrackId) -> Result<Option<TrackDetails>, DbError> {
        self.database().read(|c| track_details(c, track))
    }

    /// The library folders, in path order.
    pub fn roots(&self) -> Result<Vec<PathBuf>, DbError> {
        self.database().read(|c| {
            let mut statement = c.prepare_cached("SELECT path FROM roots ORDER BY path")?;
            let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
            rows.map(|row| row.map(path_from_blob)).collect()
        })
    }

    /// Totals of the whole library.
    pub fn totals(&self) -> Result<LibraryTotals, DbError> {
        self.database().read(totals)
    }

    /// The `limit` tracks played most often, most played first; one track for each
    /// identity.
    pub fn most_played(&self, limit: usize) -> Result<Vec<PlayedTrack>, DbError> {
        self.database().read(|c| most_played(c, limit))
    }
}

fn track_details(c: &Connection, track: TrackId) -> rusqlite::Result<Option<TrackDetails>> {
    let found = c
        .prepare_cached(
            "SELECT t.identity_id, d.path, f.name, t.sub_index, t.kind, t.title, t.title_sort,
                    a.title, t.track_no, t.disc_no, t.date, t.year, t.duration_ms,
                    t.sample_rate, t.channels, t.bit_depth, t.bitrate, t.codec, t.container,
                    t.added_at, f.missing_since
             FROM tracks t
             JOIN files f ON f.id = t.file_id
             JOIN dirs d ON d.id = f.dir_id
             LEFT JOIN albums a ON a.id = t.album_id
             WHERE t.id = ?1",
        )?
        .query_row([sql_track_id(track)], |row| {
            let dir = path_from_blob(row.get(1)?);
            let name = path_from_blob(row.get(2)?);
            Ok(TrackDetails {
                track,
                identity: IdentityId(row.get(0)?),
                path: dir.join(name),
                sub_index: row.get(3)?,
                kind: TrackKind::from_sql(row.get(4)?),
                tags: TrackTags {
                    title: row.get(5)?,
                    title_sort: row.get(6)?,
                    album: row.get(7)?,
                    track_no: row.get(8)?,
                    disc_no: row.get(9)?,
                    date: row.get(10)?,
                    year: row.get(11)?,
                    ..TrackTags::default()
                },
                audio: AudioProperties {
                    duration: row.get::<_, Option<i64>>(12)?.map(unix_time::from_millis),
                    sample_rate: row.get(13)?,
                    channels: row.get(14)?,
                    bit_depth: row.get(15)?,
                    bitrate: row.get(16)?,
                    codec: row.get(17)?,
                    container: row.get(18)?,
                },
                added_at: unix_time::from_unix_seconds(row.get(19)?),
                missing_since: row
                    .get::<_, Option<i64>>(20)?
                    .map(unix_time::from_unix_seconds),
            })
        })
        .optional()?;
    let Some(mut details) = found else {
        return Ok(None);
    };
    let id = sql_track_id(track);
    let mut people = c.prepare_cached(
        "SELECT tp.role, p.name FROM track_people tp JOIN people p ON p.id = tp.person_id
         WHERE tp.track_id = ?1 ORDER BY tp.role, tp.position",
    )?;
    let rows = people.query_map([id], |row| Ok((row.get::<_, i64>(0)?, row.get(1)?)))?;
    for row in rows {
        let (role, name) = row?;
        let list = match role {
            0 => &mut details.tags.artists,
            1 => &mut details.tags.album_artists,
            2 => &mut details.tags.composers,
            _ => continue,
        };
        list.push(name);
    }
    let mut genres = c.prepare_cached(
        "SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
         WHERE tg.track_id = ?1 ORDER BY tg.position",
    )?;
    details.tags.genres = genres
        .query_map([id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(details))
}

fn totals(c: &Connection) -> rusqlite::Result<LibraryTotals> {
    let count = |sql: &str| -> rusqlite::Result<usize> {
        let n: i64 = c.prepare_cached(sql)?.query_row([], |row| row.get(0))?;
        Ok(usize::try_from(n).unwrap_or(0))
    };
    let (tracks, duration_ms): (i64, i64) = c
        .prepare_cached("SELECT count(*), coalesce(sum(duration_ms), 0) FROM tracks")?
        .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let (files, size): (i64, i64) = c
        .prepare_cached(
            "SELECT count(*), coalesce(sum(size), 0) FROM files f
             WHERE EXISTS (SELECT 1 FROM tracks t WHERE t.file_id = f.id)",
        )?
        .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let (plays, played, rated): (i64, i64, i64) = c
        .prepare_cached(
            "SELECT coalesce(sum(s.play_count), 0), count(*) FILTER (WHERE s.play_count > 0),
                    count(*) FILTER (WHERE s.rating IS NOT NULL)
             FROM stats s
             WHERE EXISTS (SELECT 1 FROM tracks t WHERE t.identity_id = s.identity_id)",
        )?
        .query_row([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    Ok(LibraryTotals {
        roots: count("SELECT count(*) FROM roots")?,
        files: usize::try_from(files).unwrap_or(0),
        tracks: usize::try_from(tracks).unwrap_or(0),
        albums: count(
            "SELECT count(*) FROM albums a
             WHERE EXISTS (SELECT 1 FROM tracks t WHERE t.album_id = a.id)",
        )?,
        artists: count("SELECT count(DISTINCT person_id) FROM track_people WHERE role IN (0, 1)")?,
        genres: count("SELECT count(DISTINCT genre_id) FROM track_genres")?,
        duration: unix_time::from_millis(duration_ms),
        size: u64::try_from(size).unwrap_or(0),
        missing_files: count("SELECT count(*) FROM files WHERE missing_since IS NOT NULL")?,
        unreadable_files: c
            .prepare_cached("SELECT count(*) FROM files WHERE scan_state = ?1")?
            .query_row([STATE_UNREADABLE], |row| row.get::<_, i64>(0))
            .map(|n| usize::try_from(n).unwrap_or(0))?,
        plays: u64::try_from(plays).unwrap_or(0),
        played_tracks: usize::try_from(played).unwrap_or(0),
        rated_tracks: usize::try_from(rated).unwrap_or(0),
    })
}

fn most_played(c: &Connection, limit: usize) -> rusqlite::Result<Vec<PlayedTrack>> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut statement = c.prepare_cached(
        "WITH top AS (
             SELECT s.identity_id AS identity, s.play_count AS plays,
                    s.last_played AS last_played,
                    (SELECT min(t.id) FROM tracks t WHERE t.identity_id = s.identity_id)
                        AS track
             FROM stats s
             WHERE s.play_count > 0
               AND EXISTS (SELECT 1 FROM tracks t WHERE t.identity_id = s.identity_id)
             ORDER BY s.play_count DESC, s.last_played DESC, s.identity_id
             LIMIT ?1)
         SELECT top.track, t.title,
             (SELECT group_concat(name, ', ') FROM
                 (SELECT p.name AS name
                  FROM track_people tp JOIN people p ON p.id = tp.person_id
                  WHERE tp.track_id = t.id AND tp.role = 0 ORDER BY tp.position)),
             a.title, top.plays, top.last_played
         FROM top
         JOIN tracks t ON t.id = top.track
         LEFT JOIN albums a ON a.id = t.album_id
         ORDER BY top.plays DESC, top.last_played DESC, top.identity",
    )?;
    let rows = statement.query_map([limit], |row| {
        Ok(PlayedTrack {
            track: track_id_from_sql(row.get(0)?),
            title: row.get(1)?,
            artist: row.get(2)?,
            album: row.get(3)?,
            play_count: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
            last_played: row
                .get::<_, Option<i64>>(5)?
                .map(unix_time::from_unix_seconds),
        })
    })?;
    rows.collect()
}
