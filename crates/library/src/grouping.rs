// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A list of tracks in groups: the groups in the order of their values, each
//! with its tracks in the order the list had, so that a list sorted by its
//! columns becomes a list ordered by the group first and then by those columns.
//!
//! The tracks without a value form one group with an empty value, which comes
//! first. Values that differ only in case form one group, named as its first
//! track has it. Groups follow [`compare_keys`]: without regard to case, and
//! numbers by their value ("1990s" before "2000s", "Disc 2" before "Disc 10").
//!
//! A field the library keeps in its tables (album artist, album, artist, genre,
//! year, format, label, a composer, a tag) is read for the whole list in one
//! query. A computed field and an expression go through the expression language
//! ([`Library::evaluate_expression`]), track by track, as the query language
//! does for a computed field: a query cannot sort by a computed field, but a
//! list can be grouped by one, because the group order is worked out here and
//! not in SQL.

use std::cmp::Ordering;
use std::collections::HashMap;

use helikodu_model::expression::{Template, compare_keys};
use helikodu_model::library::TrackId;
use helikodu_model::query::Field;
use rusqlite::{Connection, params};

use crate::Library;
use crate::catalog::sql_track_id;
use crate::db::DbError;
use crate::labels::first_label;

/// What a list of tracks is grouped by.
#[derive(Debug, Clone, PartialEq)]
pub enum GroupBy {
    /// A field: a built-in one, a label, or one the user defined.
    Field(Field),
    /// The text an expression of the expression language gives.
    Expression(Template),
}

/// One group of a list of tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackGroup {
    /// The value its tracks share, trimmed; empty for the tracks without one.
    pub value: String,
    /// Its tracks, in the order of the list.
    pub tracks: Vec<TrackId>,
}

/// The joins of a value query onto the ids in the JSON array `?1`.
const FROM: &str = "FROM json_each(?1) k
    JOIN tracks t ON t.id = k.value
    LEFT JOIN albums a ON a.id = t.album_id";

/// The first person of `role` of track `t`, by the name shown.
fn person(role: i64) -> String {
    format!(
        "(SELECT p.name FROM track_people tp JOIN people p ON p.id = tp.person_id
            WHERE tp.track_id = t.id AND tp.role = {role} ORDER BY tp.position LIMIT 1)"
    )
}

/// The SQL for the value of `field` of track `t`, and the tag key it binds as
/// `?2`; `None` for a field that is worked out by the expression language.
fn value_sql(field: &Field) -> Option<(String, Option<String>)> {
    let sql = match field {
        Field::AlbumArtist => "coalesce(a.artist, t.album_artist)".to_owned(),
        Field::Album => "coalesce(a.title, t.album_title)".to_owned(),
        Field::Artist => "t.artist".to_owned(),
        Field::Genre => "(SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
                WHERE tg.track_id = t.id ORDER BY tg.position LIMIT 1)"
            .to_owned(),
        Field::Year => "CAST(t.year AS TEXT)".to_owned(),
        Field::OriginalYear => "CAST(t.original_year AS TEXT)".to_owned(),
        Field::Codec => "upper(t.codec)".to_owned(),
        Field::Label => first_label(),
        Field::Composer => person(2),
        Field::Work => "t.work".to_owned(),
        Field::ReleaseType => "t.release_type".to_owned(),
        Field::Tag(key) => {
            return Some((
                "(SELECT tt.value FROM track_tags tt WHERE tt.track_id = t.id
                    AND tt.key = ?2 ORDER BY tt.rowid LIMIT 1)"
                    .to_owned(),
                Some(key.clone()),
            ));
        }
        _ => return None,
    };
    Some((sql, None))
}

/// The value of `sql` for each of `tracks`, by id.
fn sql_values(
    c: &Connection,
    sql: &str,
    tag: Option<&str>,
    tracks: &[TrackId],
) -> rusqlite::Result<HashMap<i64, String>> {
    let list: Vec<String> = tracks
        .iter()
        .map(|t| sql_track_id(*t).to_string())
        .collect();
    let json = format!("[{}]", list.join(","));
    let mut statement = c.prepare_cached(&format!("SELECT t.id, {sql} {FROM}"))?;
    let row = |row: &rusqlite::Row<'_>| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
    };
    match tag {
        Some(tag) => statement.query_map(params![json, tag], row)?.collect(),
        None => statement.query_map(params![json], row)?.collect(),
    }
}

/// The expression that gives the value of `field`.
fn template_of(field: &Field) -> Option<Template> {
    if let Field::Computed(name) = field {
        return helikodu_model::user_fields::current()
            .expression(name)
            .cloned();
    }
    Template::parse(&format!("{{raw({})}}", field.name())).ok()
}

/// `values` (each track with its value, in the list's order) in groups, as the
/// module documentation describes.
pub(crate) fn group_values(values: impl IntoIterator<Item = (TrackId, String)>) -> Vec<TrackGroup> {
    let mut groups: Vec<TrackGroup> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (track, value) in values {
        let value = value.trim();
        let key = value.to_lowercase();
        match index.get(&key) {
            Some(&at) => groups[at].tracks.push(track),
            None => {
                index.insert(key, groups.len());
                groups.push(TrackGroup {
                    value: value.to_owned(),
                    tracks: vec![track],
                });
            }
        }
    }
    // Stable: values that compare equal keep the order they came in.
    groups.sort_by(|a, b| match (a.value.is_empty(), b.value.is_empty()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => compare_keys(&a.value, &b.value),
    });
    groups
}

impl Library {
    /// `tracks` in groups by `by`: the groups in the order of their values, the
    /// tracks without a value first, and the tracks of each group in the order
    /// of `tracks`. Tracks the library does not have are left out.
    pub fn group_tracks(
        &self,
        by: &GroupBy,
        tracks: &[TrackId],
    ) -> Result<Vec<TrackGroup>, DbError> {
        let template = match by {
            GroupBy::Field(field) => {
                if let Some((sql, tag)) = value_sql(field) {
                    let values = self
                        .database()
                        .read(|c| sql_values(c, &sql, tag.as_deref(), tracks))?;
                    return Ok(group_values(tracks.iter().filter_map(|track| {
                        values
                            .get(&sql_track_id(*track))
                            .map(|value| (*track, value.clone()))
                    })));
                }
                match template_of(field) {
                    Some(template) => template,
                    // A computed field no longer defined: no track has a value.
                    None => {
                        return Ok(group_values(tracks.iter().map(|t| (*t, String::new()))));
                    }
                }
            }
            GroupBy::Expression(template) => template.clone(),
        };
        Ok(group_values(self.evaluate_expression(&template, tracks)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> TrackId {
        TrackId::new(n)
    }

    #[test]
    fn groups_follow_their_values_with_no_value_first() {
        let groups = group_values([
            (id(1), "2010s".to_owned()),
            (id(2), String::new()),
            (id(3), "1990s".to_owned()),
            (id(4), "2010S ".to_owned()),
            (id(5), "  ".to_owned()),
            (id(6), "Disc 10".to_owned()),
            (id(7), "Disc 2".to_owned()),
        ]);
        let shown: Vec<(&str, Vec<TrackId>)> = groups
            .iter()
            .map(|g| (g.value.as_str(), g.tracks.clone()))
            .collect();
        assert_eq!(
            shown,
            [
                ("", vec![id(2), id(5)]),
                ("1990s", vec![id(3)]),
                // Case aside the same value: one group, named as it came first.
                ("2010s", vec![id(1), id(4)]),
                ("Disc 2", vec![id(7)]),
                ("Disc 10", vec![id(6)]),
            ]
        );
    }

    #[test]
    fn the_tracks_of_a_group_keep_the_list_order() {
        let groups = group_values([
            (id(9), "b".to_owned()),
            (id(3), "a".to_owned()),
            (id(7), "b".to_owned()),
            (id(1), "a".to_owned()),
        ]);
        assert_eq!(groups[0].tracks, [id(3), id(1)]);
        assert_eq!(groups[1].tracks, [id(9), id(7)]);
    }

    #[test]
    fn fields_kept_in_tables_are_read_by_one_query() {
        for field in [
            Field::AlbumArtist,
            Field::Album,
            Field::Genre,
            Field::Year,
            Field::Codec,
            Field::Label,
            Field::Tag("MOOD".into()),
        ] {
            assert!(value_sql(&field).is_some(), "{field:?}");
        }
        assert!(value_sql(&Field::Computed("decade".into())).is_none());
    }
}
