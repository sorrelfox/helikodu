// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lists of tracks.

use rusqlite::types::Value;
use rusqlite::{Connection, Row, params_from_iter};

use super::{Paged, Params, TrackFilter, TrackOrder, TrackQuery, TrackSortKey, TrackSummary};
use helikodu_model::library::TrackId;

use crate::catalog::ids::{AlbumId, track_id_from_sql};
use crate::catalog::roots::visibility;
use crate::catalog::track_record::{ArtistRole, TrackKind, TrackSection};
use crate::db::unix_time;

/// The columns that [`summary`] reads, from the tracks `t`, files `f`, folders `d` and
/// albums `a`.
pub(crate) const COLUMNS: &str = "t.id, t.title, t.artist, t.album_id, a.title, t.disc_no,
    t.track_no, t.duration_ms, t.year, t.kind, t.section, t.sub_index,
    f.missing_since IS NOT NULL, t.copies";

/// The joins under [`COLUMNS`].
pub(super) const FROM: &str = "FROM tracks t
    JOIN files f ON f.id = t.file_id
    JOIN dirs d ON d.id = f.dir_id
    LEFT JOIN albums a ON a.id = t.album_id";

/// The order of the tracks of one album.
pub(super) const ALBUM_ORDER: &str = "coalesce(t.disc_no, 1), t.track_no IS NULL, t.track_no,
    d.path, f.name, t.sub_index, t.id";

/// A track from a row of [`COLUMNS`].
pub(crate) fn summary(row: &Row<'_>) -> rusqlite::Result<TrackSummary> {
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
        copies: u32::try_from(row.get::<_, i64>(13)?).unwrap_or(0),
    })
}

/// The tracks of `query`.
pub(crate) fn tracks(c: &Connection, query: &TrackQuery) -> rusqlite::Result<Paged<TrackSummary>> {
    let visible = visibility(c, query.include_masked)?;
    let mut params: Params = Vec::new();
    let (filter, order) = filter_and_order(query.filter, &mut params);
    let (join, order) = sorted(&query.sort, order);
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
            "SELECT {COLUMNS} {FROM} {join} WHERE {filter} AND {visible}
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

/// Every track of `query` in its order, as its id and album only, in one
/// sorted query; the page of `query` is not used.
pub(crate) fn track_order(
    c: &Connection,
    query: &TrackQuery,
) -> rusqlite::Result<Vec<(TrackId, Option<AlbumId>)>> {
    let visible = visibility(c, query.include_masked)?;
    let mut params: Params = Vec::new();
    let (filter, order) = filter_and_order(query.filter, &mut params);
    // The order of whole lists by album and folder compares the ranks of the
    // albums and folders rather than their names and paths, the same order
    // with far smaller rows to sort.
    let (ranks, order) = match order {
        BY_ALBUM_ARTIST => (RANKS_BY_ALBUM_ARTIST, RANKED_ORDER),
        BY_YEAR => (RANKS_BY_YEAR, RANKED_ORDER),
        other => ("", other),
    };
    let (join, order) = sorted(&query.sort, order);
    let rank_join = if ranks.is_empty() {
        ""
    } else {
        "LEFT JOIN album_rank ar ON ar.id = t.album_id JOIN dir_rank dr ON dr.id = f.dir_id"
    };
    c.prepare_cached(&format!(
        "{ranks} SELECT t.id, t.album_id {FROM} {rank_join} {join}
         WHERE {filter} AND {visible} ORDER BY {order}"
    ))?
    .query_map(params_from_iter(&params), |row| {
        Ok((
            track_id_from_sql(row.get(0)?),
            row.get::<_, Option<i64>>(1)?.map(AlbumId),
        ))
    })?
    .collect()
}

/// The condition of `filter` (its values pushed to `params`) and its own order.
fn filter_and_order(filter: TrackFilter, params: &mut Params) -> (String, &'static str) {
    match filter {
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
        TrackFilter::Years(span) => (super::years::track_condition(span, params), BY_ALBUM_ARTIST),
    }
}

/// Albums in the order of their year, then the tracks without an album by title.
const BY_YEAR: &str = "t.album_id IS NULL, coalesce(a.original_year, a.year) IS NULL,
    coalesce(a.original_year, a.year), a.title_sort, t.album_id,
    coalesce(t.disc_no, 1), t.track_no IS NULL, t.track_no, d.path, f.name, t.sub_index,
    t.title_sort, t.id";

/// Albums by album artist, year and title, then the tracks without an album by title.
pub(crate) const BY_ALBUM_ARTIST: &str = "t.album_id IS NULL, a.artist_sort IS NULL, a.artist_sort,
    coalesce(a.original_year, a.year), a.title_sort, t.album_id,
    coalesce(t.disc_no, 1), t.track_no IS NULL, t.track_no, d.path, f.name, t.sub_index,
    t.title_sort, t.id";

/// [`BY_ALBUM_ARTIST`] and [`BY_YEAR`] over the ranks of [`RANKS_BY_ALBUM_ARTIST`]
/// or [`RANKS_BY_YEAR`]: `ar.r` (no rank without an album) and `dr.r`.
const RANKED_ORDER: &str = "t.album_id IS NULL, ar.r, coalesce(t.disc_no, 1), t.track_no IS NULL,
    t.track_no, dr.r, f.name, t.sub_index, t.title_sort, t.id";

/// The albums ranked as [`BY_ALBUM_ARTIST`] orders them, and the folders by path.
const RANKS_BY_ALBUM_ARTIST: &str = "WITH album_rank(id, r) AS (
        SELECT id, row_number() OVER (ORDER BY artist_sort IS NULL, artist_sort,
            coalesce(original_year, year), title_sort, id) FROM albums),
    dir_rank(id, r) AS (SELECT id, row_number() OVER (ORDER BY path) FROM dirs)";

/// The albums ranked as [`BY_YEAR`] orders them, and the folders by path.
const RANKS_BY_YEAR: &str = "WITH album_rank(id, r) AS (
        SELECT id, row_number() OVER (ORDER BY coalesce(original_year, year) IS NULL,
            coalesce(original_year, year), title_sort, id) FROM albums),
    dir_rank(id, r) AS (SELECT id, row_number() OVER (ORDER BY path) FROM dirs)";

/// The order of `sort` in front of the filter's own `order`, and the join it needs.
fn sorted(sort: &[TrackOrder], order: &str) -> (&'static str, String) {
    if sort.is_empty() {
        return ("", order.to_owned());
    }
    let mut terms = Vec::new();
    let mut stats = false;
    for key in sort {
        let d = if key.descending { " DESC" } else { "" };
        terms.push(match key.key {
            TrackSortKey::Number => {
                format!("coalesce(t.disc_no, 1){d}, t.track_no IS NULL, t.track_no{d}")
            }
            TrackSortKey::Title => format!("t.title_sort IS NULL, t.title_sort{d}"),
            TrackSortKey::Artist => {
                format!("t.artist IS NULL, t.artist COLLATE HK_LOCALE{d}")
            }
            TrackSortKey::Album => format!("a.title_sort IS NULL, a.title_sort{d}"),
            TrackSortKey::Year => format!("t.year IS NULL, t.year{d}"),
            TrackSortKey::Length => format!("t.duration_ms IS NULL, t.duration_ms{d}"),
            TrackSortKey::Format => format!(
                "t.codec IS NULL, lower(t.codec){d}, t.sample_rate{d}, t.bit_depth{d}, \
                 t.bitrate{d}"
            ),
            TrackSortKey::Rating => {
                stats = true;
                format!("s.rating IS NULL, s.rating{d}")
            }
            TrackSortKey::Plays => {
                stats = true;
                format!("coalesce(s.play_count, 0){d}")
            }
        });
    }
    terms.push(order.to_owned());
    let join = if stats {
        "LEFT JOIN stats s ON s.identity_id = t.identity_id"
    } else {
        ""
    };
    (join, terms.join(", "))
}
