// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Facets: the values of one field among the tracks a query selects, each with its
//! number of tracks, narrowed by the values chosen in other fields.
//!
//! The facet browser of the track table fills each of its panes with
//! [`Library::facet_values`] and the table with [`Library::facet_track_ids`]. Every
//! pane is one grouped query on the database; nothing is filtered in memory.

use helikodu_model::library::TrackId;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, params_from_iter};

use super::compiler::{Compiled, FROM};
use super::{QueryError, QueryRequest, run};
use crate::catalog::track_id_from_sql;
use crate::library::Library;

/// A field the facet browser can list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FacetField {
    /// The genres of the tracks.
    Genre,
    /// The album artists of their albums.
    AlbumArtist,
    /// Their albums.
    Album,
    /// The track artists.
    Artist,
    /// The composers.
    Composer,
    /// The release year.
    Year,
    /// The audio format (the codec).
    Format,
    /// The labels of the tracks: their own and those of their albums and artists.
    Label,
}

impl FacetField {
    /// Every field, in the order the browser offers them.
    pub const ALL: [Self; 8] = [
        Self::Genre,
        Self::AlbumArtist,
        Self::Album,
        Self::Artist,
        Self::Composer,
        Self::Year,
        Self::Format,
        Self::Label,
    ];
}

/// One value of a facet: a row id (genre, person, album, label), a year, or a
/// codec.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FacetKey {
    /// The id of a genre, person, album or label, or a year.
    Id(i64),
    /// A codec, in lower case.
    Text(String),
}

/// A value of a facet as a pane lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetValue {
    /// What identifies it when it is chosen.
    pub key: FacetKey,
    /// What the pane shows.
    pub name: String,
    /// How many of the tracks have it.
    pub tracks: usize,
}

/// The values of one facet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FacetList {
    /// The values, in the pane's order.
    pub values: Vec<FacetValue>,
    /// How many tracks the pane covers in all (its "All …" row).
    pub tracks: usize,
}

/// The values chosen in one pane; a track passes when it has any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetChoice {
    /// The pane's field.
    pub field: FacetField,
    /// The values chosen; empty passes every track.
    pub keys: Vec<FacetKey>,
}

impl Library {
    /// The values of `field` among the tracks `request` selects that pass every
    /// choice of `chosen`, each with its number of tracks.
    pub fn facet_values(
        &self,
        request: &QueryRequest,
        chosen: &[FacetChoice],
        field: FacetField,
    ) -> Result<FacetList, QueryError> {
        let compiled = self.compile(request)?;
        Ok(self.database().read(|c| {
            let (condition, params) = narrowed(c, &compiled, request.include_masked, chosen)?;
            values(c, &condition, &params, field)
        })?)
    }

    /// Every track `request` selects that passes every choice of `chosen`, in the
    /// request's order.
    pub fn facet_track_ids(
        &self,
        request: &QueryRequest,
        chosen: &[FacetChoice],
    ) -> Result<Vec<TrackId>, QueryError> {
        let compiled = self.compile(request)?;
        Ok(self.database().read(|c| {
            let (condition, params) = narrowed(c, &compiled, request.include_masked, chosen)?;
            c.prepare(&format!(
                "SELECT t.id {FROM} WHERE {condition} ORDER BY {}",
                compiled.order
            ))?
            .query_map(params_from_iter(&params), |row| {
                row.get(0).map(track_id_from_sql)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })?)
    }
}

/// The condition of `compiled` (with its limit) and of every choice, with its
/// parameters.
fn narrowed(
    c: &Connection,
    compiled: &Compiled,
    include_masked: bool,
    chosen: &[FacetChoice],
) -> rusqlite::Result<(String, Vec<SqlValue>)> {
    let base = run::condition(c, compiled, include_masked)?;
    let mut params = compiled.params.clone();
    let mut parts = vec![format!("({base})")];
    if let Some(kept) = run::kept(c, compiled, &base)? {
        let list: Vec<String> = kept.iter().map(i64::to_string).collect();
        params.push(SqlValue::Text(format!("[{}]", list.join(","))));
        parts.push(format!(
            "t.id IN (SELECT value FROM json_each(?{}))",
            params.len()
        ));
    }
    for choice in chosen.iter().filter(|c| !c.keys.is_empty()) {
        parts.push(choice_condition(choice, &mut params));
    }
    Ok((parts.join(" AND "), params))
}

/// The condition that a track `t` has one of the values of `choice`.
fn choice_condition(choice: &FacetChoice, params: &mut Vec<SqlValue>) -> String {
    let mut list = Vec::with_capacity(choice.keys.len());
    for key in &choice.keys {
        params.push(match key {
            FacetKey::Id(id) => SqlValue::Integer(*id),
            FacetKey::Text(text) => SqlValue::Text(text.clone()),
        });
        list.push(format!("?{}", params.len()));
    }
    let list = list.join(", ");
    match choice.field {
        FacetField::Genre => {
            format!("t.id IN (SELECT track_id FROM track_genres WHERE genre_id IN ({list}))")
        }
        FacetField::AlbumArtist => {
            format!("t.album_id IN (SELECT album_id FROM album_people WHERE person_id IN ({list}))")
        }
        FacetField::Album => format!("t.album_id IN ({list})"),
        FacetField::Artist => people_condition(0, &list),
        FacetField::Composer => people_condition(2, &list),
        FacetField::Year => format!("t.year IN ({list})"),
        FacetField::Format => format!("lower(t.codec) IN ({list})"),
        FacetField::Label => crate::labels::has_label(&list),
    }
}

fn people_condition(role: i64, list: &str) -> String {
    format!(
        "t.id IN (SELECT track_id FROM track_people WHERE role = {role} AND person_id IN ({list}))"
    )
}

/// The values of `field` among the tracks of `condition`, and their number.
fn values(
    c: &Connection,
    condition: &str,
    params: &[SqlValue],
    field: FacetField,
) -> rusqlite::Result<FacetList> {
    let matching = format!(
        "WITH m AS (SELECT t.id AS id, t.album_id AS album_id, t.year AS year,
                           t.codec AS codec, t.identity_id AS identity_id
                    {FROM} WHERE {condition})"
    );
    let tracks: i64 = c
        .prepare(&format!("{matching} SELECT count(*) FROM m"))?
        .query_row(params_from_iter(params), |row| row.get(0))?;
    let grouped = match field {
        FacetField::Genre => {
            "SELECT g.id, g.name, count(DISTINCT m.id) FROM m
             JOIN track_genres tg ON tg.track_id = m.id
             JOIN genres g ON g.id = tg.genre_id
             GROUP BY g.id ORDER BY g.name COLLATE HK_LOCALE, g.id"
        }
        FacetField::AlbumArtist => {
            "SELECT p.id, p.name, count(DISTINCT m.id) FROM m
             JOIN album_people ap ON ap.album_id = m.album_id
             JOIN people p ON p.id = ap.person_id
             GROUP BY p.id ORDER BY coalesce(p.sort_name, p.name) COLLATE HK_LOCALE, p.id"
        }
        FacetField::Album => {
            "SELECT a.id, coalesce(a.title, ''), count(*) FROM m
             JOIN albums a ON a.id = m.album_id
             GROUP BY a.id ORDER BY a.title_sort, a.id"
        }
        FacetField::Artist => {
            "SELECT p.id, p.name, count(DISTINCT m.id) FROM m
             JOIN track_people tp ON tp.track_id = m.id AND tp.role = 0
             JOIN people p ON p.id = tp.person_id
             GROUP BY p.id ORDER BY coalesce(p.sort_name, p.name) COLLATE HK_LOCALE, p.id"
        }
        FacetField::Composer => {
            "SELECT p.id, p.name, count(DISTINCT m.id) FROM m
             JOIN track_people tp ON tp.track_id = m.id AND tp.role = 2
             JOIN people p ON p.id = tp.person_id
             GROUP BY p.id ORDER BY coalesce(p.sort_name, p.name) COLLATE HK_LOCALE, p.id"
        }
        FacetField::Year => {
            "SELECT m.year, CAST(m.year AS TEXT), count(*) FROM m
             WHERE m.year IS NOT NULL GROUP BY m.year ORDER BY m.year DESC"
        }
        FacetField::Format => {
            "SELECT lower(m.codec), lower(m.codec), count(*) FROM m
             WHERE m.codec IS NOT NULL GROUP BY lower(m.codec) ORDER BY count(*) DESC, 1"
        }
        // A track counts under each of its labels: those given to it, to its
        // album and to its artists.
        FacetField::Label => {
            "SELECT l.id, l.name, count(DISTINCT x.track) FROM (
                SELECT m.id AS track, lt.label_id AS label FROM m
                    JOIN label_tracks lt ON lt.identity_id = m.identity_id
                UNION SELECT m.id, la.label_id FROM m
                    JOIN albums al ON al.id = m.album_id
                    JOIN label_albums la ON la.album_key = al.album_key
                UNION SELECT m.id, lr.label_id FROM m
                    JOIN track_people tp ON tp.track_id = m.id AND tp.role = 0
                    JOIN people p ON p.id = tp.person_id
                    JOIN label_artists lr ON lr.artist_fold = p.fold
                UNION SELECT m.id, lr.label_id FROM m
                    JOIN album_people ap ON ap.album_id = m.album_id
                    JOIN people p ON p.id = ap.person_id
                    JOIN label_artists lr ON lr.artist_fold = p.fold
             ) x JOIN labels l ON l.id = x.label
             GROUP BY l.id ORDER BY l.name COLLATE HK_LOCALE, l.id"
        }
    };
    let text_key = field == FacetField::Format;
    let values = c
        .prepare(&format!("{matching} {grouped}"))?
        .query_map(params_from_iter(params), |row| {
            let key = if text_key {
                FacetKey::Text(row.get(0)?)
            } else {
                FacetKey::Id(row.get(0)?)
            };
            Ok(FacetValue {
                key,
                name: row.get(1)?,
                tracks: usize::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(FacetList {
        values,
        tracks: usize::try_from(tracks).unwrap_or(0),
    })
}
