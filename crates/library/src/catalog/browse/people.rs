// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lists of people in a role, and of genres.

use rusqlite::{Connection, params};

use super::{ArtistQuery, ArtistSummary, GenreQuery, GenreSummary, Paged};
use crate::catalog::ids::{GenreId, PersonId};
use crate::catalog::roots::visibility;
use crate::catalog::track_record::ArtistRole;

/// The people in the role of `query`, by sort name.
pub(crate) fn artists(
    c: &Connection,
    query: &ArtistQuery,
) -> rusqlite::Result<Paged<ArtistSummary>> {
    let visible = visibility(c, query.include_masked)?;
    let role = query.role.to_sql();
    // The tracks `t` on which the person `?1` has the role `?2`.
    let tracks = if query.role == ArtistRole::AlbumArtist {
        "FROM album_people ap JOIN tracks t ON t.album_id = ap.album_id
         WHERE ap.person_id = ?1 AND ?2 = ?2"
    } else {
        "FROM track_people tp JOIN tracks t ON t.id = tp.track_id
         WHERE tp.person_id = ?1 AND tp.role = ?2"
    };
    let has = if visible == "1" {
        if query.role == ArtistRole::AlbumArtist {
            // `?1`, the role, is not needed; it is named so that every variant takes it.
            "EXISTS (SELECT 1 FROM album_people ap WHERE ap.person_id = p.id) AND ?1 = ?1"
                .to_owned()
        } else {
            "EXISTS (SELECT 1 FROM track_people tp WHERE tp.person_id = p.id AND tp.role = ?1)"
                .to_owned()
        }
    } else {
        format!(
            "EXISTS (SELECT 1 {} AND {visible})",
            tracks.replace("?1", "p.id").replace("?2", "?1")
        )
    };
    let total: i64 = c
        .prepare_cached(&format!("SELECT count(*) FROM people p WHERE {has}"))?
        .query_row([role], |row| row.get(0))?;
    let (limit, offset) = query.page.sql();
    let mut items: Vec<ArtistSummary> = c
        .prepare_cached(&format!(
            "SELECT p.id, p.name, p.sort_name FROM people p WHERE {has}
             ORDER BY p.sort_name, p.id LIMIT ?2 OFFSET ?3"
        ))?
        .query_map(params![role, limit, offset], |row| {
            Ok(ArtistSummary {
                id: PersonId(row.get(0)?),
                name: row.get(1)?,
                sort_name: row.get(2)?,
                album_count: 0,
                track_count: 0,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut counts = c.prepare_cached(&format!(
        "SELECT count(DISTINCT t.id), count(DISTINCT t.album_id) {tracks} AND {visible}"
    ))?;
    for artist in &mut items {
        let (track_count, album_count): (i64, i64) = counts
            .query_row(params![artist.id.0, role], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        artist.track_count = usize::try_from(track_count).unwrap_or(0);
        artist.album_count = usize::try_from(album_count).unwrap_or(0);
    }
    Ok(Paged {
        items,
        total: usize::try_from(total).unwrap_or(0),
    })
}

/// The genres, by name.
pub(crate) fn genres(c: &Connection, query: &GenreQuery) -> rusqlite::Result<Paged<GenreSummary>> {
    let visible = visibility(c, query.include_masked)?;
    let has = if visible == "1" {
        "EXISTS (SELECT 1 FROM track_genres tg WHERE tg.genre_id = g.id)".to_owned()
    } else {
        format!(
            "EXISTS (SELECT 1 FROM track_genres tg JOIN tracks t ON t.id = tg.track_id
                     WHERE tg.genre_id = g.id AND {visible})"
        )
    };
    let total: i64 = c
        .prepare_cached(&format!("SELECT count(*) FROM genres g WHERE {has}"))?
        .query_row([], |row| row.get(0))?;
    let (limit, offset) = query.page.sql();
    let mut items: Vec<GenreSummary> = c
        .prepare_cached(&format!(
            "SELECT g.id, g.name FROM genres g WHERE {has}
             ORDER BY g.name COLLATE HK_LOCALE, g.id LIMIT ?1 OFFSET ?2"
        ))?
        .query_map(params![limit, offset], |row| {
            Ok(GenreSummary {
                id: GenreId(row.get(0)?),
                name: row.get(1)?,
                album_count: 0,
                track_count: 0,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut counts = c.prepare_cached(&format!(
        "SELECT count(*), count(DISTINCT t.album_id)
         FROM track_genres tg JOIN tracks t ON t.id = tg.track_id
         WHERE tg.genre_id = ?1 AND {visible}"
    ))?;
    for genre in &mut items {
        let (track_count, album_count): (i64, i64) =
            counts.query_row([genre.id.0], |row| Ok((row.get(0)?, row.get(1)?)))?;
        genre.track_count = usize::try_from(track_count).unwrap_or(0);
        genre.album_count = usize::try_from(album_count).unwrap_or(0);
    }
    Ok(Paged {
        items,
        total: usize::try_from(total).unwrap_or(0),
    })
}
