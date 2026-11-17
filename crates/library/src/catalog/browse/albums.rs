// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lists of albums, and one album with its discs and tracks.

use helikodu_meta::covers::CoverHash;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use super::tracks::{ALBUM_ORDER, COLUMNS, FROM, summary};
use super::{
    AlbumDetail, AlbumFilter, AlbumQuery, AlbumSort, AlbumSummary, ArtistRef, Disc, GenreRef,
    Paged, Params,
};
use crate::catalog::ids::{AlbumId, GenreId, PersonId};
use crate::catalog::roots::visibility;
use crate::catalog::track_record::ArtistRole;
use crate::db::unix_time;

/// The album columns that [`album_row`] reads, from the albums `a`.
const ALBUM_COLUMNS: &str = "a.id, a.title, a.artist, a.is_compilation, a.year,
    a.original_year, a.release_type, a.disc_total, a.added_at,
    (SELECT hash FROM covers WHERE id = a.cover_id)";

/// An album from a row of [`ALBUM_COLUMNS`], without its counts.
fn album_row(row: &Row<'_>) -> rusqlite::Result<AlbumSummary> {
    let release_type: Option<String> = row.get(6)?;
    Ok(AlbumSummary {
        id: AlbumId(row.get(0)?),
        title: row.get(1)?,
        artist: row.get(2)?,
        is_compilation: row.get(3)?,
        year: row.get(4)?,
        original_year: row.get(5)?,
        release_types: release_type
            .map(|types| {
                types
                    .split("; ")
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        track_count: 0,
        disc_count: 0,
        disc_total: row.get(7)?,
        duration: None,
        added_at: row.get(8)?,
        cover: row
            .get::<_, Option<Vec<u8>>>(9)?
            .and_then(|hash| CoverHash::from_bytes(&hash)),
    })
}

/// Fills in the counts of `album` from its tracks that satisfy `visible`.
fn add_counts(c: &Connection, album: &mut AlbumSummary, visible: &str) -> rusqlite::Result<()> {
    let (tracks, discs, duration): (i64, i64, Option<i64>) = c
        .prepare_cached(&format!(
            "SELECT count(*), count(DISTINCT coalesce(t.disc_no, 1)), sum(t.duration_ms)
             FROM tracks t WHERE t.album_id = ?1 AND {visible}"
        ))?
        .query_row([album.id.0], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    album.track_count = usize::try_from(tracks).unwrap_or(0);
    album.disc_count = u32::try_from(discs).unwrap_or(0);
    album.duration = duration.map(unix_time::from_millis);
    Ok(())
}

/// The condition and parameters of `filter` on the albums `a`.
fn filter_sql(filter: AlbumFilter) -> (&'static str, Params) {
    let person = |p: PersonId| Value::Integer(p.0);
    match filter {
        AlbumFilter::All => ("1", Vec::new()),
        AlbumFilter::ByArtist(p) | AlbumFilter::Credited(p, ArtistRole::AlbumArtist) => (
            "a.id IN (SELECT album_id FROM album_people WHERE person_id = ?1)",
            vec![person(p)],
        ),
        AlbumFilter::AppearsOn(p) => (
            "a.id IN (SELECT t.album_id FROM track_people tp JOIN tracks t ON t.id = tp.track_id
                      WHERE tp.person_id = ?1 AND tp.role = ?2)
             AND a.id NOT IN (SELECT album_id FROM album_people WHERE person_id = ?1)",
            vec![person(p), Value::Integer(ArtistRole::Artist.to_sql())],
        ),
        AlbumFilter::Credited(p, role) => (
            "a.id IN (SELECT t.album_id FROM track_people tp JOIN tracks t ON t.id = tp.track_id
                      WHERE tp.person_id = ?1 AND tp.role = ?2)",
            vec![person(p), Value::Integer(role.to_sql())],
        ),
        AlbumFilter::Compilations => ("a.is_compilation = 1", Vec::new()),
        AlbumFilter::Genre(GenreId(g)) => (
            "a.id IN (SELECT t.album_id FROM track_genres tg JOIN tracks t ON t.id = tg.track_id
                      WHERE tg.genre_id = ?1)",
            vec![Value::Integer(g)],
        ),
    }
}

fn order_sql(sort: AlbumSort) -> &'static str {
    match sort {
        AlbumSort::Title => "a.title_sort, a.id",
        AlbumSort::Artist => {
            "a.artist_sort IS NULL, a.artist_sort, coalesce(a.original_year, a.year),
             a.title_sort, a.id"
        }
        AlbumSort::Year => {
            "coalesce(a.original_year, a.year) IS NULL, coalesce(a.original_year, a.year),
             a.title_sort, a.id"
        }
        AlbumSort::Added => "a.added_at DESC, a.id DESC",
    }
}

/// The albums of `query`.
pub(crate) fn albums(c: &Connection, query: &AlbumQuery) -> rusqlite::Result<Paged<AlbumSummary>> {
    let visible = visibility(c, query.include_masked)?;
    let shown = if visible == "1" {
        "1".to_owned()
    } else {
        format!("EXISTS (SELECT 1 FROM tracks t WHERE t.album_id = a.id AND {visible})")
    };
    let (filter, mut params) = filter_sql(query.filter);
    let total: i64 = c
        .prepare_cached(&format!(
            "SELECT count(*) FROM albums a WHERE {filter} AND {shown}"
        ))?
        .query_row(params_from_iter(&params), |row| row.get(0))?;
    let (limit, offset) = query.page.sql();
    let n = params.len();
    params.push(Value::Integer(limit));
    params.push(Value::Integer(offset));
    let mut items: Vec<AlbumSummary> = c
        .prepare_cached(&format!(
            "SELECT {ALBUM_COLUMNS} FROM albums a WHERE {filter} AND {shown}
             ORDER BY {} LIMIT ?{} OFFSET ?{}",
            order_sql(query.sort),
            n + 1,
            n + 2
        ))?
        .query_map(params_from_iter(&params), album_row)?
        .collect::<rusqlite::Result<_>>()?;
    for album in &mut items {
        add_counts(c, album, &visible)?;
    }
    Ok(Paged {
        items,
        total: usize::try_from(total).unwrap_or(0),
    })
}

/// The album `id` with its discs and tracks; `None` if there is no such album, or
/// none of its tracks is shown.
pub(crate) fn album(
    c: &Connection,
    id: AlbumId,
    include_masked: bool,
) -> rusqlite::Result<Option<AlbumDetail>> {
    let visible = visibility(c, include_masked)?;
    let found = c
        .prepare_cached(&format!(
            "SELECT {ALBUM_COLUMNS}, a.title_sort, a.artist_sort, a.mbid, a.release_group_mbid
             FROM albums a WHERE a.id = ?1"
        ))?
        .query_row([id.0], |row| {
            Ok((
                album_row(row)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
            ))
        })
        .optional()?;
    let Some((mut summary_row, title_sort, artist_sort, mbid, release_group_mbid)) = found else {
        return Ok(None);
    };
    let tracks: Vec<(super::TrackSummary, Option<String>)> = c
        .prepare_cached(&format!(
            "SELECT {COLUMNS}, t.disc_subtitle {FROM}
             WHERE t.album_id = ?1 AND {visible} ORDER BY {ALBUM_ORDER}"
        ))?
        .query_map([id.0], |row| Ok((summary(row)?, row.get(13)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if tracks.is_empty() {
        return Ok(None);
    }
    add_counts(c, &mut summary_row, &visible)?;

    let mut discs: Vec<Disc> = Vec::new();
    for (track, subtitle) in tracks {
        let number = track.disc_no.unwrap_or(1);
        match discs.last_mut() {
            Some(disc) if disc.number == number => {
                if disc.subtitle.is_none() {
                    disc.subtitle = subtitle;
                }
                disc.tracks.push(track);
            }
            _ => discs.push(Disc {
                number,
                subtitle,
                tracks: vec![track],
            }),
        }
    }
    let artists = c
        .prepare_cached(
            "SELECT p.id, p.name FROM album_people ap JOIN people p ON p.id = ap.person_id
             WHERE ap.album_id = ?1 ORDER BY ap.position",
        )?
        .query_map([id.0], |row| {
            Ok(ArtistRef {
                id: PersonId(row.get(0)?),
                name: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let genres = c
        .prepare_cached(&format!(
            "SELECT g.id, g.name FROM track_genres tg
             JOIN tracks t ON t.id = tg.track_id JOIN genres g ON g.id = tg.genre_id
             WHERE t.album_id = ?1 AND {visible}
             GROUP BY g.id ORDER BY count(*) DESC, min(tg.position), g.name"
        ))?
        .query_map(params![id.0], |row| {
            Ok(GenreRef {
                id: GenreId(row.get(0)?),
                name: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(AlbumDetail {
        summary: summary_row,
        title_sort,
        artist_sort,
        artists,
        genres,
        mbid,
        release_group_mbid,
        discs,
    }))
}
