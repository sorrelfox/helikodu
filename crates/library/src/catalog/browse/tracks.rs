// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lists of tracks.

use rusqlite::types::Value;
use rusqlite::{Connection, Row, params_from_iter};

use super::{Paged, Params, TrackFilter, TrackQuery, TrackSummary};
use crate::catalog::ids::{AlbumId, track_id_from_sql};
use crate::catalog::roots::visibility;
use crate::catalog::track_record::{ArtistRole, TrackKind, TrackSection};
use crate::db::unix_time;

/// The columns that [`summary`] reads, from the tracks `t`, files `f`, folders `d` and
/// albums `a`.
pub(super) const COLUMNS: &str = "t.id, t.title, t.artist, t.album_id, a.title, t.disc_no,
    t.track_no, t.duration_ms, t.year, t.kind, t.section, t.sub_index,
    f.missing_since IS NOT NULL";

/// The joins under [`COLUMNS`].
pub(super) const FROM: &str = "FROM tracks t
    JOIN files f ON f.id = t.file_id
    JOIN dirs d ON d.id = f.dir_id
    LEFT JOIN albums a ON a.id = t.album_id";

/// The order of the tracks of one album.
pub(super) const ALBUM_ORDER: &str = "coalesce(t.disc_no, 1), t.track_no IS NULL, t.track_no,
    d.path, f.name, t.sub_index, t.id";

/// A track from a row of [`COLUMNS`].
pub(super) fn summary(row: &Row<'_>) -> rusqlite::Result<TrackSummary> {
    Ok(TrackSummary {
        id: track_id_from_sql(row.get(0)?),
        title: row.get(1)?,
        artist: row.get(2)?,
        album: row.get::<_, Option<i64>>(3)?.map(AlbumId),
        album_title: row.get(4)?,
        disc_no: row.get(5)?,
        track_no: row.get(6)?,
        duration: row.get::<_, Option<i64>>(7)?.map(unix_time::from_millis),
        year: row.get(8)?,
        kind: TrackKind::from_sql(row.get(9)?),
        section: TrackSection::from_sql(row.get(10)?),
        sub_index: row.get(11)?,
        missing: row.get(12)?,
    })
}

/// The tracks of `query`.
pub(crate) fn tracks(c: &Connection, query: &TrackQuery) -> rusqlite::Result<Paged<TrackSummary>> {
    let visible = visibility(c, query.include_masked)?;
    let mut params: Params = Vec::new();
    let (filter, order) = match query.filter {
        TrackFilter::All => ("1".to_owned(), BY_ALBUM_ARTIST),
        TrackFilter::Album(album) => {
            params.push(Value::Integer(album.0));
            ("t.album_id = ?1".to_owned(), ALBUM_ORDER)
        }
        TrackFilter::Artist(person, None) => {
            params.push(Value::Integer(person.0));
            (
                "t.id IN (SELECT track_id FROM track_people WHERE person_id = ?1)".to_owned(),
                BY_YEAR,
            )
        }
        TrackFilter::Artist(person, Some(ArtistRole::AlbumArtist)) => {
            params.push(Value::Integer(person.0));
            (
                "t.album_id IN (SELECT album_id FROM album_people WHERE person_id = ?1)".to_owned(),
                BY_YEAR,
            )
        }
        TrackFilter::Artist(person, Some(role)) => {
            params.push(Value::Integer(person.0));
            params.push(Value::Integer(role.to_sql()));
            (
                "t.id IN (SELECT track_id FROM track_people
                          WHERE person_id = ?1 AND role = ?2)"
                    .to_owned(),
                BY_YEAR,
            )
        }
        TrackFilter::Genre(genre) => {
            params.push(Value::Integer(genre.0));
            (
                "t.id IN (SELECT track_id FROM track_genres WHERE genre_id = ?1)".to_owned(),
                BY_ALBUM_ARTIST,
            )
        }
    };
    let total: i64 = c
        .prepare_cached(&format!(
            "SELECT count(*) FROM tracks t WHERE {filter} AND {visible}"
        ))?
        .query_row(params_from_iter(&params), |row| row.get(0))?;
    let (limit, offset) = query.page.sql();
    let n = params.len();
    params.push(Value::Integer(limit));
    params.push(Value::Integer(offset));
    let items = c
        .prepare_cached(&format!(
            "SELECT {COLUMNS} {FROM} WHERE {filter} AND {visible}
             ORDER BY {order} LIMIT ?{} OFFSET ?{}",
            n + 1,
            n + 2
        ))?
        .query_map(params_from_iter(&params), summary)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Paged {
        items,
        total: usize::try_from(total).unwrap_or(0),
    })
}

/// Albums in the order of their year, then the tracks without an album by title.
const BY_YEAR: &str = "t.album_id IS NULL, coalesce(a.original_year, a.year) IS NULL,
    coalesce(a.original_year, a.year), a.title_sort, t.album_id,
    coalesce(t.disc_no, 1), t.track_no IS NULL, t.track_no, d.path, f.name, t.sub_index,
    t.title_sort, t.id";

/// Albums by album artist, year and title, then the tracks without an album by title.
const BY_ALBUM_ARTIST: &str = "t.album_id IS NULL, a.artist_sort IS NULL, a.artist_sort,
    coalesce(a.original_year, a.year), a.title_sort, t.album_id,
    coalesce(t.disc_no, 1), t.track_no IS NULL, t.track_no, d.path, f.name, t.sub_index,
    t.title_sort, t.id";
