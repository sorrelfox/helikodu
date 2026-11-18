// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the player's play modes need to know about tracks: the album, folder,
//! grouping and artist they belong to, for shuffling by blocks and for "Play only",
//! and the per-track playback flags, which belong to the track's identity like its
//! statistics.

use std::collections::HashMap;
use std::path::PathBuf;

use helikodu_model::library::TrackId;
use helikodu_model::player::TrackFlags;
use rusqlite::{Connection, OptionalExtension, params};

use crate::Library;
use crate::catalog::{AlbumId, IdentityId, PersonId, TrackKind, sql_track_id};
use crate::db::{DbError, path_from_blob};

/// The blocks a track belongs to, and its playback flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayKeys {
    /// Its album, if it has one.
    pub album: Option<AlbumId>,
    /// The folder of its file.
    pub folder: PathBuf,
    /// Its grouping: a `grouping` tag kept with the other tags, or else its work.
    pub grouping: Option<String>,
    /// Its first artist.
    pub artist: Option<PersonId>,
    /// The per-track playback flags. An audiobook is always left out of shuffle.
    pub flags: TrackFlags,
}

impl Library {
    /// The play keys of `tracks`; tracks that are not in the library are left out.
    pub fn play_keys(&self, tracks: &[TrackId]) -> Result<HashMap<TrackId, PlayKeys>, DbError> {
        let tracks = tracks.to_vec();
        self.database().read(move |c| {
            let mut keys = HashMap::with_capacity(tracks.len());
            for track in tracks {
                if keys.contains_key(&track) {
                    continue;
                }
                if let Some(found) = play_keys_of(c, track)? {
                    keys.insert(track, found);
                }
            }
            Ok(keys)
        })
    }

    /// The playback flags of `track`; the defaults when none were set.
    pub fn track_flags(&self, track: TrackId) -> Result<TrackFlags, DbError> {
        self.database()
            .read(move |c| {
                c.prepare_cached(
                    "SELECT s.flags FROM tracks t
                     LEFT JOIN stats s ON s.identity_id = t.identity_id
                     WHERE t.id = ?1",
                )?
                .query_row([sql_track_id(track)], |row| row.get::<_, Option<i64>>(0))
                .optional()
            })?
            .map(|bits| TrackFlags::from_bits(bits.unwrap_or(0)))
            .ok_or(DbError::UnknownTrack(track))
    }

    /// Stores the playback flags of `track`.
    pub fn set_track_flags(&self, track: TrackId, flags: TrackFlags) -> Result<(), DbError> {
        let found = self.database().writer()?.write(move |c| {
            let identity = c
                .prepare_cached("SELECT identity_id FROM tracks WHERE id = ?1")?
                .query_row([sql_track_id(track)], |row| row.get::<_, i64>(0))
                .optional()?;
            let Some(identity) = identity.map(IdentityId) else {
                return Ok(false);
            };
            c.prepare_cached("INSERT INTO stats (identity_id) VALUES (?1) ON CONFLICT DO NOTHING")?
                .execute([identity.0])?;
            let bits = (!flags.is_default()).then(|| flags.to_bits());
            c.prepare_cached("UPDATE stats SET flags = ?2 WHERE identity_id = ?1")?
                .execute(params![identity.0, bits])?;
            Ok(true)
        })?;
        if found {
            Ok(())
        } else {
            Err(DbError::UnknownTrack(track))
        }
    }
}

/// The playback flags of a track of `kind`: an audiobook is left out of shuffle.
fn flags_of_kind(mut flags: TrackFlags, kind: TrackKind) -> TrackFlags {
    if kind == TrackKind::Audiobook {
        flags.skip_in_shuffle = true;
    }
    flags
}

fn play_keys_of(c: &Connection, track: TrackId) -> rusqlite::Result<Option<PlayKeys>> {
    c.prepare_cached(
        "SELECT t.album_id, d.path,
                coalesce((SELECT g.value FROM track_tags g
                          WHERE g.track_id = t.id AND g.key = 'GROUPING' LIMIT 1), t.work),
                (SELECT p.person_id FROM track_people p
                 WHERE p.track_id = t.id AND p.role = 0 ORDER BY p.position LIMIT 1),
                s.flags, t.kind
         FROM tracks t
         JOIN files f ON f.id = t.file_id
         JOIN dirs d ON d.id = f.dir_id
         LEFT JOIN stats s ON s.identity_id = t.identity_id
         WHERE t.id = ?1",
    )?
    .query_row([sql_track_id(track)], |row| {
        Ok(PlayKeys {
            album: row.get::<_, Option<i64>>(0)?.map(AlbumId::from_raw),
            folder: path_from_blob(row.get(1)?),
            grouping: row
                .get::<_, Option<String>>(2)?
                .filter(|g| !g.trim().is_empty()),
            artist: row.get::<_, Option<i64>>(3)?.map(PersonId::from_raw),
            flags: flags_of_kind(
                TrackFlags::from_bits(row.get::<_, Option<i64>>(4)?.unwrap_or(0)),
                TrackKind::from_sql(row.get(5)?),
            ),
        })
    })
    .optional()
}
