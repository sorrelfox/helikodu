// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The equalizer of a track, which replaces the equalizer of the settings while the
//! track plays. It belongs to the track's identity, like its statistics.

use std::time::{SystemTime, UNIX_EPOCH};

use helikodu_model::library::TrackId;
use helikodu_model::settings::EqualizerSettings;
use rusqlite::{OptionalExtension, params};

use crate::Library;
use crate::catalog::{sql_track_id, track_id_from_sql};
use crate::db::DbError;

impl Library {
    /// The equalizer of `track`, if one was set. A stored value this version cannot
    /// read counts as none.
    pub fn track_equalizer(&self, track: TrackId) -> Result<Option<EqualizerSettings>, DbError> {
        let text = self.database().read(move |c| {
            c.prepare_cached(
                "SELECT e.settings FROM tracks t
                 JOIN track_equalizers e ON e.identity_id = t.identity_id
                 WHERE t.id = ?1",
            )?
            .query_row([sql_track_id(track)], |row| row.get::<_, String>(0))
            .optional()
        })?;
        Ok(text.and_then(|text| serde_json::from_str(&text).ok()))
    }

    /// Sets the equalizer of `track`, or removes it with `None`.
    pub fn set_track_equalizer(
        &self,
        track: TrackId,
        equalizer: Option<&EqualizerSettings>,
    ) -> Result<(), DbError> {
        let text = equalizer.map(|eq| serde_json::to_string(eq).unwrap_or_default());
        let found = self.database().writer()?.write(move |c| {
            let identity = c
                .prepare_cached("SELECT identity_id FROM tracks WHERE id = ?1")?
                .query_row([sql_track_id(track)], |row| row.get::<_, i64>(0))
                .optional()?;
            let Some(identity) = identity else {
                return Ok(false);
            };
            match &text {
                Some(text) => {
                    c.prepare_cached(
                        "INSERT INTO track_equalizers (identity_id, settings, changed_at)
                         VALUES (?1, ?2, ?3)
                         ON CONFLICT (identity_id) DO UPDATE
                         SET settings = excluded.settings, changed_at = excluded.changed_at",
                    )?
                    .execute(params![identity, text, now_seconds()])?;
                }
                None => {
                    c.prepare_cached("DELETE FROM track_equalizers WHERE identity_id = ?1")?
                        .execute([identity])?;
                }
            }
            Ok(true)
        })?;
        if found {
            Ok(())
        } else {
            Err(DbError::UnknownTrack(track))
        }
    }

    /// The tracks that have an equalizer of their own, by id; one track per
    /// identity.
    pub fn tracks_with_equalizer(&self) -> Result<Vec<TrackId>, DbError> {
        self.database().read(|c| {
            let mut statement = c.prepare_cached(
                "SELECT min(t.id) FROM track_equalizers e
                 JOIN tracks t ON t.identity_id = e.identity_id
                 GROUP BY e.identity_id ORDER BY 1",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
            let mut tracks = Vec::new();
            for id in rows {
                tracks.push(track_id_from_sql(id?));
            }
            Ok(tracks)
        })
    }
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
