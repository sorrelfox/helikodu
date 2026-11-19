// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Expressions of the expression language ([`helikodu_model::expression`]) for
//! library tracks: their values, and tracks sorted and grouped by them.
//!
//! The values are those naming patterns use ([`Library::naming_values`]), plus
//! the fields a file name has no use for but a display row, a sort key or a
//! group may want: the path, the kind, the length and resume point in seconds,
//! the size in bytes, plays, skips, the rating in stars, the ReplayGain gains in
//! dB, and the dates as `YYYY-MM-DD HH:MM:SS` in local time. Only the fields an
//! expression reads are loaded.
//!
//! Sorting and grouping work on a list of tracks (the result of a query, say)
//! rather than inside the query's SQL: an expression can call functions that
//! SQL does not have, and the fields of several values live in tables of their
//! own, so evaluating it in Rust once per track is both simpler and faster than
//! teaching SQLite the language.

use helikodu_model::expression::{FieldMap, Template, compare_keys};
use helikodu_model::library::TrackId;
use helikodu_model::query::{Field, MediaKind};
use rusqlite::{Connection, OptionalExtension};

use crate::Library;
use crate::catalog::sql_track_id;
use crate::db::DbError;
use crate::organize::naming_values_of;

/// The fields that naming patterns do not have.
const EXTRA: &[Field] = &[
    Field::Path,
    Field::Kind,
    Field::Duration,
    Field::Resume,
    Field::Size,
    Field::Plays,
    Field::Skips,
    Field::Rating,
    Field::TrackGain,
    Field::AlbumGain,
    Field::Added,
    Field::Modified,
    Field::LastPlayed,
    Field::FirstPlayed,
    Field::LastSkipped,
];

/// A date column as local time.
fn local(column: &str) -> String {
    format!("strftime('%Y-%m-%d %H:%M:%S', {column}, 'unixepoch', 'localtime')")
}

/// Adds the fields of [`EXTRA`] among `fields` to `map`.
fn add_extra(
    c: &Connection,
    track: TrackId,
    fields: &[Field],
    map: &mut FieldMap,
) -> rusqlite::Result<()> {
    if !fields.iter().any(|f| EXTRA.contains(f)) {
        return Ok(());
    }
    let sql = format!(
        "SELECT d.path_text || '/' || f.name_text, t.kind, t.duration_ms, s.resume_ms,
                f.size, coalesce(s.play_count, 0), coalesce(s.skip_count, 0), s.rating,
                t.rg_track_gain, t.rg_album_gain, {}, {}, {}, {}, {}
         FROM tracks t
         JOIN files f ON f.id = t.file_id
         JOIN dirs d ON d.id = f.dir_id
         LEFT JOIN stats s ON s.identity_id = t.identity_id
         WHERE t.id = ?1",
        local("t.added_at"),
        local("t.modified_at"),
        local("s.last_played"),
        local("s.first_played"),
        local("s.last_skipped"),
    );
    let Some(row) = c
        .prepare_cached(&sql)?
        .query_row([sql_track_id(track)], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<f64>>(8)?,
                row.get::<_, Option<f64>>(9)?,
                [
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                ],
            ))
        })
        .optional()?
    else {
        return Ok(());
    };
    let (path, kind, duration, resume, size, plays, skips, rating, track_gain, album_gain, dates) =
        row;
    let seconds = |ms: i64| format!("{}", ms as f64 / 1000.0);
    let kind = match kind.unwrap_or(0) {
        1 => MediaKind::Audiobook,
        2 => MediaKind::Podcast,
        3 => MediaKind::Video,
        _ => MediaKind::Music,
    };
    let [added, modified, last_played, first_played, last_skipped] = dates;
    let values: [(Field, Option<String>); 15] = [
        (Field::Path, path),
        (Field::Kind, Some(kind.name().to_owned())),
        (Field::Duration, duration.map(seconds)),
        (Field::Resume, resume.map(seconds)),
        (Field::Size, size.map(|n| n.to_string())),
        (Field::Plays, plays.map(|n| n.to_string())),
        (Field::Skips, skips.map(|n| n.to_string())),
        (Field::Rating, rating.map(|r| format!("{}", r as f64 / 2.0))),
        (Field::TrackGain, track_gain.map(|g| format!("{g:.2}"))),
        (Field::AlbumGain, album_gain.map(|g| format!("{g:.2}"))),
        (Field::Added, added),
        (Field::Modified, modified),
        (Field::LastPlayed, last_played),
        (Field::FirstPlayed, first_played),
        (Field::LastSkipped, last_skipped),
    ];
    for (field, value) in values {
        if let Some(value) = value
            && fields.contains(&field)
        {
            map.set(field, value);
        }
    }
    Ok(())
}

/// The values of `fields` (every field and tag when `None`) of each of `tracks`
/// on the connection `c`; tracks the library does not have are left out.
pub(crate) fn values_on(
    c: &Connection,
    tracks: &[TrackId],
    fields: Option<&[Field]>,
) -> rusqlite::Result<Vec<(TrackId, FieldMap)>> {
    let mut values = naming_values_of(c, tracks, fields)?;
    let extra = EXTRA.to_vec();
    let wanted = fields.unwrap_or(&extra);
    for (track, map) in &mut values {
        add_extra(c, *track, wanted, map)?;
    }
    Ok(values)
}

impl Library {
    /// The values of `fields` (every field and tag when `None`) of each of
    /// `tracks`, for expressions; tracks the library does not have are left out.
    pub fn expression_values(
        &self,
        tracks: &[TrackId],
        fields: Option<&[Field]>,
    ) -> Result<Vec<(TrackId, FieldMap)>, DbError> {
        let tracks = tracks.to_vec();
        let fields = fields.map(<[Field]>::to_vec);
        self.database()
            .read(move |c| values_on(c, &tracks, fields.as_deref()))
    }

    /// The text `expression` gives for each of `tracks`, in their order; tracks
    /// the library does not have are left out.
    pub fn evaluate_expression(
        &self,
        expression: &Template,
        tracks: &[TrackId],
    ) -> Result<Vec<(TrackId, String)>, DbError> {
        let fields = expression.fields();
        Ok(self
            .expression_values(tracks, Some(&fields))?
            .into_iter()
            .map(|(track, values)| (track, expression.evaluate(&values)))
            .collect())
    }

    /// `tracks` sorted by the text `expression` gives (the order of
    /// [`compare_keys`]: without regard to case, numbers by their value), the
    /// greatest first when `descending`; tracks with the same text keep their
    /// order. Tracks the library does not have are left out.
    pub fn sort_by_expression(
        &self,
        expression: &Template,
        tracks: &[TrackId],
        descending: bool,
    ) -> Result<Vec<TrackId>, DbError> {
        let mut keyed = self.evaluate_expression(expression, tracks)?;
        keyed.sort_by(|(_, a), (_, b)| {
            let order = compare_keys(a, b);
            if descending { order.reverse() } else { order }
        });
        Ok(keyed.into_iter().map(|(track, _)| track).collect())
    }

    /// `tracks` grouped by the text `expression` gives: the groups in the order
    /// their first track has in `tracks`, and the tracks of each in their order.
    /// Tracks the library does not have are left out.
    pub fn group_by_expression(
        &self,
        expression: &Template,
        tracks: &[TrackId],
    ) -> Result<Vec<(String, Vec<TrackId>)>, DbError> {
        let mut groups: Vec<(String, Vec<TrackId>)> = Vec::new();
        let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (track, key) in self.evaluate_expression(expression, tracks)? {
            match index.get(&key) {
                Some(&at) => groups[at].1.push(track),
                None => {
                    index.insert(key.clone(), groups.len());
                    groups.push((key, vec![track]));
                }
            }
        }
        Ok(groups)
    }
}
