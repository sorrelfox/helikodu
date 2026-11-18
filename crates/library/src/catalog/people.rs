// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing a track's people and genres.
//!
//! A person is one row per folded name (`hk_fold`), so `ABBA` and `Abba`, or `Beyoncé`
//! and `Beyonce`, are one person, shown under the spelling seen first. A MusicBrainz
//! artist ID tells apart two people with the same name. Genres are one row per folded
//! name too.
//!
//! The rows of a track are changed only where they differ, so that a person or genre
//! that stays is never removed and created again. Triggers remove a person or genre
//! when its last row goes.

use rusqlite::{Connection, OptionalExtension, params};

use super::track_record::{ArtistRole, CatalogTags, TrackTags};
use crate::search::fold;

/// One `track_people` row to be.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    role: i64,
    position: i64,
    person: i64,
    instrument: Option<String>,
}

/// The person called `name`, created if needed, with the sort name and MusicBrainz ID
/// the tags give for them.
pub(crate) fn person(
    c: &Connection,
    name: &str,
    sort_name: Option<&str>,
    mbid: Option<&str>,
) -> rusqlite::Result<i64> {
    let folded = fold(name);
    let mbid = mbid.map(str::trim).filter(|m| !m.is_empty());
    let mut found: Option<(i64, Option<String>, Option<String>)> = None;
    if let Some(mbid) = mbid {
        found = c
            .prepare_cached("SELECT id, mbid, sort_name FROM people WHERE mbid = ?1 LIMIT 1")?
            .query_row([mbid], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
    }
    if found.is_none() {
        // Without an ID, any person of that name; with one, a person without an ID.
        found = c
            .prepare_cached(
                "SELECT id, mbid, sort_name FROM people
                 WHERE fold = ?1 AND (?2 IS NULL OR mbid IS NULL OR mbid = ?2)
                 ORDER BY mbid IS NULL, id LIMIT 1",
            )?
            .query_row(params![folded, mbid], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
    }
    match found {
        Some((id, known_mbid, known_sort)) => {
            let sort_name = sort_name.filter(|s| known_sort.as_deref() != Some(*s));
            let mbid = mbid.filter(|_| known_mbid.is_none());
            if sort_name.is_some() || mbid.is_some() {
                c.prepare_cached(
                    "UPDATE people SET sort_name = coalesce(?2, sort_name),
                                       mbid = coalesce(mbid, ?3)
                     WHERE id = ?1",
                )?
                .execute(params![id, sort_name, mbid])?;
            }
            Ok(id)
        }
        None => c
            .prepare_cached(
                "INSERT INTO people (name, sort_name, fold, mbid) VALUES (?1, ?2, ?3, ?4)
                 RETURNING id",
            )?
            .query_row(
                params![name, sort_name.unwrap_or(name), folded, mbid],
                |row| row.get(0),
            ),
    }
}

/// A name credited on the track, before it is looked up.
struct Credited<'a> {
    name: &'a str,
    sort: Option<&'a str>,
    instrument: Option<&'a str>,
    mbid: Option<&'a str>,
}

/// The sort name of the only name in `names`, if the tags give one.
fn only<'a>(names: &[String], sort: Option<&'a String>) -> Option<&'a str> {
    (names.len() == 1)
        .then_some(sort.map(String::as_str))
        .flatten()
}

/// Writes the track's people: artists, album artists, composers and the other
/// credits.
pub(crate) fn write_people<'a>(
    c: &Connection,
    track: i64,
    tags: &'a TrackTags,
    catalog: &'a CatalogTags,
) -> rusqlite::Result<()> {
    let mut rows: Vec<Row> = Vec::new();
    let mbid = |list: &'a [String], names: &[String], i: usize| -> Option<&'a str> {
        (list.len() == names.len())
            .then(|| list.get(i).map(String::as_str))
            .flatten()
    };
    let mut credited: Vec<(ArtistRole, Credited<'a>)> = Vec::new();
    for (role, names, sort, mbids) in [
        (
            ArtistRole::Artist,
            &tags.artists,
            only(&tags.artists, catalog.artist_sort.as_ref()),
            catalog.artist_mbids.as_slice(),
        ),
        (
            ArtistRole::AlbumArtist,
            &tags.album_artists,
            only(&tags.album_artists, catalog.album_artist_sort.as_ref()),
            catalog.album_artist_mbids.as_slice(),
        ),
        (
            ArtistRole::Composer,
            &tags.composers,
            only(&tags.composers, catalog.composer_sort.as_ref()),
            &[][..],
        ),
    ] {
        for (i, name) in names.iter().enumerate() {
            credited.push((
                role,
                Credited {
                    name,
                    sort,
                    instrument: None,
                    mbid: mbid(mbids, names, i),
                },
            ));
        }
    }
    for role in ArtistRole::ALL {
        for credit in catalog.credits.iter().filter(|credit| credit.role == role) {
            if matches!(
                role,
                ArtistRole::Artist | ArtistRole::AlbumArtist | ArtistRole::Composer
            ) {
                continue;
            }
            credited.push((
                role,
                Credited {
                    name: &credit.name,
                    sort: None,
                    instrument: credit.instrument.as_deref(),
                    mbid: None,
                },
            ));
        }
    }
    for (role, credit) in credited {
        let role = role.to_sql();
        let person = person(c, credit.name, credit.sort, credit.mbid)?;
        if rows.iter().any(|r| r.role == role && r.person == person) {
            continue; // The same person twice in one role.
        }
        let position = rows.iter().filter(|r| r.role == role).count();
        rows.push(Row {
            role,
            position: i64::try_from(position).unwrap_or(i64::MAX),
            person,
            instrument: credit.instrument.map(str::to_owned),
        });
    }
    replace_rows(c, track, &rows)
}

/// The folded names of the track's artists and album artists, separated by U+001F,
/// which search scans (`tracks.artists_fold`).
pub(crate) fn artists_fold(tags: &TrackTags) -> Option<String> {
    let artists: Vec<String> = tags
        .artists
        .iter()
        .chain(&tags.album_artists)
        .map(|name| fold(name))
        .collect();
    (!artists.is_empty()).then(|| artists.join("\u{1f}"))
}

/// Makes the track's `track_people` rows `rows`, changing only what differs.
fn replace_rows(c: &Connection, track: i64, rows: &[Row]) -> rusqlite::Result<()> {
    let existing: Vec<Row> = c
        .prepare_cached(
            "SELECT role, position, person_id, instrument FROM track_people WHERE track_id = ?1",
        )?
        .query_map([track], |row| {
            Ok(Row {
                role: row.get(0)?,
                position: row.get(1)?,
                person: row.get(2)?,
                instrument: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut upsert = c.prepare_cached(
        "INSERT INTO track_people (track_id, person_id, role, position, instrument)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (track_id, role, position) DO UPDATE SET
             person_id = excluded.person_id, instrument = excluded.instrument",
    )?;
    for row in rows {
        if !existing.contains(row) {
            upsert.execute(params![
                track,
                row.person,
                row.role,
                row.position,
                row.instrument
            ])?;
        }
    }
    let mut delete = c.prepare_cached(
        "DELETE FROM track_people WHERE track_id = ?1 AND role = ?2 AND position = ?3",
    )?;
    for old in &existing {
        if !rows
            .iter()
            .any(|r| r.role == old.role && r.position == old.position)
        {
            delete.execute(params![track, old.role, old.position])?;
        }
    }
    Ok(())
}

/// Writes the track's genres, changing only what differs.
pub(crate) fn write_genres(c: &Connection, track: i64, genres: &[String]) -> rusqlite::Result<()> {
    let mut wanted: Vec<i64> = Vec::with_capacity(genres.len());
    for name in genres {
        let id = genre(c, name)?;
        if !wanted.contains(&id) {
            wanted.push(id);
        }
    }
    let existing: Vec<(i64, i64)> = c
        .prepare_cached("SELECT genre_id, position FROM track_genres WHERE track_id = ?1")?
        .query_map([track], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut upsert = c.prepare_cached(
        "INSERT INTO track_genres (track_id, genre_id, position) VALUES (?1, ?2, ?3)
         ON CONFLICT (track_id, genre_id) DO UPDATE SET position = excluded.position",
    )?;
    for (position, genre) in (0_i64..).zip(&wanted) {
        if !existing.contains(&(*genre, position)) {
            upsert.execute(params![track, genre, position])?;
        }
    }
    let mut delete =
        c.prepare_cached("DELETE FROM track_genres WHERE track_id = ?1 AND genre_id = ?2")?;
    for (genre, _) in &existing {
        if !wanted.contains(genre) {
            delete.execute(params![track, genre])?;
        }
    }
    Ok(())
}

/// Writes the track's other tags (`track_tags`), changing only what differs. The
/// order of the values of a key is kept: a changed order rewrites the track's rows.
pub(crate) fn write_other_tags(
    c: &Connection,
    track: i64,
    tags: &[(String, String)],
) -> rusqlite::Result<()> {
    let existing: Vec<(String, String)> = c
        .prepare_cached("SELECT key, value FROM track_tags WHERE track_id = ?1 ORDER BY rowid")?
        .query_map([track], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if existing == tags {
        return Ok(());
    }
    c.prepare_cached("DELETE FROM track_tags WHERE track_id = ?1")?
        .execute([track])?;
    let mut insert =
        c.prepare_cached("INSERT INTO track_tags (track_id, key, value) VALUES (?1, ?2, ?3)")?;
    for (key, value) in tags {
        insert.execute(params![track, key, value])?;
    }
    Ok(())
}

/// The genre called `name` (compared folded), created if needed.
fn genre(c: &Connection, name: &str) -> rusqlite::Result<i64> {
    let folded = fold(name);
    let found = c
        .prepare_cached("SELECT id FROM genres WHERE fold = ?1 ORDER BY id LIMIT 1")?
        .query_row([&folded], |row| row.get(0))
        .optional()?;
    match found {
        Some(id) => Ok(id),
        None => c
            .prepare_cached(
                "INSERT INTO genres (name, fold) VALUES (?1, ?2)
                 ON CONFLICT (name) DO UPDATE SET fold = excluded.fold
                 RETURNING id",
            )?
            .query_row(params![name, folded], |row| row.get(0)),
    }
}
