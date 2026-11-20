// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Labels: free words the user gives tracks, albums and artists, kept in the
//! library and never written to a file.
//!
//! - A label of a track belongs to its identity, as its statistics and rating do,
//!   so it stays through a rescan, a move or a rename.
//! - A label of an album belongs to the album's key (its MusicBrainz release ID, or
//!   its folded album artist, title and year), and one of an artist to the artist's
//!   folded name, so both stay when a rescan builds the album or the artist anew.
//! - A track has the labels given to it, to its album and to its artists (track
//!   artists and album artists): `label:jazz` finds them all.
//!
//! Labels are told apart by their folded form, so "Jazz" and "jazz" are one label;
//! the name keeps the spelling it was last given. A label nothing has any more is
//! removed.

use std::time::SystemTime;

use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension, params};

use crate::Library;
use crate::catalog::{AlbumId, sql_track_id, track_id_from_sql};
use crate::db::DbError;
use crate::db::unix_time::to_unix_seconds;
use crate::search::fold;

/// The longest label, in characters.
pub const MAX_LABEL_CHARS: usize = 100;

/// What a label is given to or taken from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelTarget {
    /// These tracks.
    Tracks(Vec<TrackId>),
    /// An album, and so every track of it.
    Album(AlbumId),
    /// The album of this track.
    AlbumOf(TrackId),
    /// An artist by name (in any case and with or without accents), and so every
    /// track the artist plays or is album artist of.
    Artist(String),
}

/// A label with what has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelSummary {
    /// The name.
    pub name: String,
    /// How many tracks have it, given to them or to their albums or artists.
    pub tracks: usize,
    /// How many tracks it was given to themselves.
    pub own_tracks: usize,
    /// How many albums it was given to.
    pub albums: usize,
    /// How many artists it was given to.
    pub artists: usize,
}

/// The labels of a track, an album or an artist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemLabels {
    /// The labels given to the item itself (for several tracks, to any of them).
    pub own: Vec<String>,
    /// The other labels a track has from its album or its artists.
    pub inherited: Vec<String>,
}

/// A label with everything that has it, for an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelItems {
    /// The name.
    pub name: String,
    /// The tracks it was given to, as they are in the library now.
    pub tracks: Vec<TrackId>,
    /// The albums it was given to that are in the library now.
    pub albums: Vec<AlbumId>,
    /// The artists it was given to, by name.
    pub artists: Vec<String>,
}

/// Why a label could not be given, taken, renamed or merged.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LabelError {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A label needs a name with more than white space.
    #[error("a label needs a name")]
    EmptyName,
    /// The name is longer than [`MAX_LABEL_CHARS`].
    #[error("a label has at most {MAX_LABEL_CHARS} characters")]
    TooLong,
    /// There is no such label.
    #[error("there is no label {0}")]
    UnknownLabel(String),
    /// Another label has that name already; merge the two instead.
    #[error("there is already a label {0}; merge the labels instead")]
    Exists(String),
    /// A label cannot be merged into itself.
    #[error("a label cannot be merged into itself")]
    SameLabel,
    /// The library has no such album.
    #[error("there is no album {0}")]
    UnknownAlbum(AlbumId),
    /// The track is not on an album, or not in the library.
    #[error("track {0} is on no album")]
    NoAlbum(TrackId),
    /// The library has no artist of that name.
    #[error("there is no artist {0}")]
    UnknownArtist(String),
}

/// A failure found inside a database job.
enum Refusal {
    UnknownLabel(String),
    Exists(String),
    SameLabel,
    UnknownAlbum(AlbumId),
    NoAlbum(TrackId),
    UnknownArtist(String),
}

impl From<Refusal> for LabelError {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::UnknownLabel(name) => Self::UnknownLabel(name),
            Refusal::Exists(name) => Self::Exists(name),
            Refusal::SameLabel => Self::SameLabel,
            Refusal::UnknownAlbum(album) => Self::UnknownAlbum(album),
            Refusal::NoAlbum(track) => Self::NoAlbum(track),
            Refusal::UnknownArtist(name) => Self::UnknownArtist(name),
        }
    }
}

/// `name` as a label: trimmed, with runs of white space as one space and without
/// control characters.
pub fn clean_label(name: &str) -> Result<String, LabelError> {
    let name = name
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        Err(LabelError::EmptyName)
    } else if name.chars().count() > MAX_LABEL_CHARS {
        Err(LabelError::TooLong)
    } else {
        Ok(name)
    }
}

/// The condition that the track `t` has one of the labels whose ids `labels` (a
/// subquery) selects: given to it, to its album or to one of its artists. Never
/// NULL.
pub(crate) fn has_label(labels: &str) -> String {
    format!(
        "(t.identity_id IN (SELECT identity_id FROM label_tracks WHERE label_id IN ({labels}))
          OR coalesce(t.album_id IN (SELECT al.id FROM albums al
                JOIN label_albums la ON la.album_key = al.album_key
                WHERE la.label_id IN ({labels})), 0)
          OR t.id IN (SELECT tp.track_id FROM track_people tp
                JOIN people p ON p.id = tp.person_id
                JOIN label_artists lr ON lr.artist_fold = p.fold
                WHERE tp.role = 0 AND lr.label_id IN ({labels}))
          OR coalesce(t.album_id IN (SELECT ap.album_id FROM album_people ap
                JOIN people p ON p.id = ap.person_id
                JOIN label_artists lr ON lr.artist_fold = p.fold
                WHERE lr.label_id IN ({labels})), 0))"
    )
}

/// The first label of the track `t` in name order, for sorting; NULL for none.
pub(crate) fn first_label() -> String {
    "(SELECT l.name FROM labels l WHERE l.id IN (
        SELECT label_id FROM label_tracks WHERE identity_id = t.identity_id
        UNION SELECT la.label_id FROM label_albums la
            JOIN albums al ON al.album_key = la.album_key WHERE al.id = t.album_id
        UNION SELECT lr.label_id FROM label_artists lr
            JOIN people p ON p.fold = lr.artist_fold
            JOIN track_people tp ON tp.person_id = p.id
            WHERE tp.track_id = t.id AND tp.role = 0
        UNION SELECT lr.label_id FROM label_artists lr
            JOIN people p ON p.fold = lr.artist_fold
            JOIN album_people ap ON ap.person_id = p.id WHERE ap.album_id = t.album_id)
     ORDER BY l.name COLLATE HK_LOCALE LIMIT 1) COLLATE HK_LOCALE"
        .to_owned()
}

/// The ids of the labels track `?1` has from its album and its artists.
const INHERITED: &str = "SELECT la.label_id FROM label_albums la
        JOIN albums al ON al.album_key = la.album_key
        WHERE al.id = (SELECT album_id FROM tracks WHERE id = ?1)
    UNION SELECT lr.label_id FROM label_artists lr
        JOIN people p ON p.fold = lr.artist_fold
        JOIN track_people tp ON tp.person_id = p.id
        WHERE tp.track_id = ?1 AND tp.role = 0
    UNION SELECT lr.label_id FROM label_artists lr
        JOIN people p ON p.fold = lr.artist_fold
        JOIN album_people ap ON ap.person_id = p.id
        WHERE ap.album_id = (SELECT album_id FROM tracks WHERE id = ?1)";

/// The ids of the labels given to track `?1` itself.
const OWN: &str = "SELECT label_id FROM label_tracks
        WHERE identity_id = (SELECT identity_id FROM tracks WHERE id = ?1)";

/// Every label of track `track`, own and inherited, in name order.
pub(crate) fn of_track(c: &Connection, track: i64) -> rusqlite::Result<Vec<String>> {
    c.prepare_cached(&format!(
        "SELECT name FROM labels WHERE id IN ({OWN} UNION {INHERITED})
         ORDER BY name COLLATE HK_LOCALE"
    ))?
    .query_map([track], |row| row.get(0))?
    .collect()
}

fn names(c: &Connection, sql: &str, param: &dyn rusqlite::ToSql) -> rusqlite::Result<Vec<String>> {
    c.prepare_cached(sql)?
        .query_map([param], |row| row.get(0))?
        .collect()
}

/// The id of the label `name`, made if there is none; renamed to `name` if only
/// its spelling differs.
fn label_id(c: &Connection, name: &str, now: i64) -> rusqlite::Result<i64> {
    c.prepare_cached(
        "INSERT INTO labels (name, fold, created_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (fold) DO UPDATE SET name = excluded.name
         RETURNING id",
    )?
    .query_row(params![name, fold(name), now], |row| row.get(0))
}

fn find_label(c: &Connection, name: &str) -> rusqlite::Result<Option<(i64, String)>> {
    c.prepare_cached("SELECT id, name FROM labels WHERE fold = ?1")?
        .query_row([fold(name)], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()
}

/// Removes the label `id` if nothing has it.
pub(crate) fn drop_if_unused(c: &Connection, id: i64) -> rusqlite::Result<()> {
    c.prepare_cached(
        "DELETE FROM labels WHERE id = ?1
           AND NOT EXISTS (SELECT 1 FROM label_tracks WHERE label_id = ?1)
           AND NOT EXISTS (SELECT 1 FROM label_albums WHERE label_id = ?1)
           AND NOT EXISTS (SELECT 1 FROM label_artists WHERE label_id = ?1)",
    )?
    .execute([id])?;
    Ok(())
}

/// The keys of `target` in the label tables: the identities of tracks, the key of
/// an album or the folded name of an artist.
enum Keys {
    Identities(Vec<i64>),
    Album(String),
    Artist(String),
}

fn keys(c: &Connection, target: &LabelTarget) -> rusqlite::Result<Result<Keys, Refusal>> {
    Ok(Ok(match target {
        LabelTarget::Tracks(tracks) => {
            let mut statement = c.prepare_cached("SELECT identity_id FROM tracks WHERE id = ?1")?;
            let mut identities = Vec::with_capacity(tracks.len());
            for track in tracks {
                if let Some(identity) = statement
                    .query_row([sql_track_id(*track)], |row| row.get::<_, i64>(0))
                    .optional()?
                    && !identities.contains(&identity)
                {
                    identities.push(identity);
                }
            }
            Keys::Identities(identities)
        }
        LabelTarget::Album(album) => {
            match c
                .prepare_cached("SELECT album_key FROM albums WHERE id = ?1")?
                .query_row([album.get()], |row| row.get(0))
                .optional()?
            {
                Some(key) => Keys::Album(key),
                None => return Ok(Err(Refusal::UnknownAlbum(*album))),
            }
        }
        LabelTarget::AlbumOf(track) => {
            match c
                .prepare_cached(
                    "SELECT a.album_key FROM tracks t JOIN albums a ON a.id = t.album_id
                     WHERE t.id = ?1",
                )?
                .query_row([sql_track_id(*track)], |row| row.get(0))
                .optional()?
            {
                Some(key) => Keys::Album(key),
                None => return Ok(Err(Refusal::NoAlbum(*track))),
            }
        }
        LabelTarget::Artist(name) => {
            let folded = fold(name.trim());
            let known: bool = c
                .prepare_cached("SELECT EXISTS (SELECT 1 FROM people WHERE fold = ?1)")?
                .query_row([&folded], |row| row.get(0))?;
            if !known {
                return Ok(Err(Refusal::UnknownArtist(name.trim().to_owned())));
            }
            Keys::Artist(folded)
        }
    }))
}

/// Gives (`add`) or takes the label `label` to or from `keys`; how many items
/// changed.
fn change(c: &Connection, label: i64, keys: &Keys, add: bool) -> rusqlite::Result<usize> {
    let (table, column) = match keys {
        Keys::Identities(_) => ("label_tracks", "identity_id"),
        Keys::Album(_) => ("label_albums", "album_key"),
        Keys::Artist(_) => ("label_artists", "artist_fold"),
    };
    let sql = if add {
        format!("INSERT OR IGNORE INTO {table} (label_id, {column}) VALUES (?1, ?2)")
    } else {
        format!("DELETE FROM {table} WHERE label_id = ?1 AND {column} = ?2")
    };
    let mut statement = c.prepare_cached(&sql)?;
    Ok(match keys {
        Keys::Identities(ids) => {
            let mut changed = 0;
            for id in ids {
                changed += statement.execute(params![label, id])?;
            }
            changed
        }
        Keys::Album(key) | Keys::Artist(key) => statement.execute(params![label, key])?,
    })
}

impl Library {
    /// Every label, in name order, with how many tracks, albums and artists have it.
    pub fn labels(&self) -> Result<Vec<LabelSummary>, DbError> {
        let tracks = has_label("SELECT l.id");
        self.database().read(move |c| {
            c.prepare(&format!(
                "SELECT l.name,
                    (SELECT count(*) FROM tracks t WHERE {tracks}),
                    (SELECT count(*) FROM label_tracks lt JOIN tracks t
                        ON t.identity_id = lt.identity_id WHERE lt.label_id = l.id),
                    (SELECT count(*) FROM label_albums WHERE label_id = l.id),
                    (SELECT count(*) FROM label_artists WHERE label_id = l.id)
                 FROM labels l ORDER BY l.name COLLATE HK_LOCALE, l.id"
            ))?
            .query_map([], |row| {
                let count = |i| {
                    row.get::<_, i64>(i)
                        .map(|n| usize::try_from(n).unwrap_or(0))
                };
                Ok(LabelSummary {
                    name: row.get(0)?,
                    tracks: count(1)?,
                    own_tracks: count(2)?,
                    albums: count(3)?,
                    artists: count(4)?,
                })
            })?
            .collect()
        })
    }

    /// Gives the label `name` to `target`, making the label if there is none (a
    /// label that differs only in case or accents is the same label, and takes
    /// this spelling). How many tracks, albums or artists did not have it before.
    pub fn add_label(&self, target: &LabelTarget, name: &str) -> Result<usize, LabelError> {
        let name = clean_label(name)?;
        let target = target.clone();
        let now = to_unix_seconds(SystemTime::now());
        let done = self.database().writer()?.write(move |c| {
            let keys = match keys(c, &target)? {
                Ok(keys) => keys,
                Err(refusal) => return Ok(Err(refusal)),
            };
            if matches!(&keys, Keys::Identities(ids) if ids.is_empty()) {
                return Ok(Ok(0));
            }
            let label = label_id(c, &name, now)?;
            change(c, label, &keys, true).map(Ok)
        })?;
        Ok(done?)
    }

    /// Takes the label `name` from `target`; a label nothing has any more is
    /// removed. How many tracks, albums or artists had it.
    pub fn remove_label(&self, target: &LabelTarget, name: &str) -> Result<usize, LabelError> {
        let name = clean_label(name)?;
        let target = target.clone();
        let done = self.database().writer()?.write(move |c| {
            let Some((label, _)) = find_label(c, &name)? else {
                return Ok(Err(Refusal::UnknownLabel(name)));
            };
            let keys = match keys(c, &target)? {
                Ok(keys) => keys,
                Err(refusal) => return Ok(Err(refusal)),
            };
            let changed = change(c, label, &keys, false)?;
            drop_if_unused(c, label)?;
            Ok(Ok(changed))
        })?;
        Ok(done?)
    }

    /// Renames the label `name` to `new_name`. A new name that is another label's
    /// fails with [`LabelError::Exists`]: [`merge_labels`](Self::merge_labels)
    /// joins two labels.
    pub fn rename_label(&self, name: &str, new_name: &str) -> Result<(), LabelError> {
        let name = clean_label(name)?;
        let new_name = clean_label(new_name)?;
        let done = self.database().writer()?.write(move |c| {
            let Some((label, _)) = find_label(c, &name)? else {
                return Ok(Err(Refusal::UnknownLabel(name)));
            };
            if let Some((other, other_name)) = find_label(c, &new_name)?
                && other != label
            {
                return Ok(Err(Refusal::Exists(other_name)));
            }
            c.prepare_cached("UPDATE labels SET name = ?2, fold = ?3 WHERE id = ?1")?
                .execute(params![label, new_name, fold(&new_name)])?;
            Ok(Ok(()))
        })?;
        Ok(done?)
    }

    /// Merges the label `from` into the label `into`: whatever has `from` has
    /// `into` instead, and `from` is removed.
    pub fn merge_labels(&self, from: &str, into: &str) -> Result<(), LabelError> {
        let from = clean_label(from)?;
        let into = clean_label(into)?;
        let done = self.database().writer()?.write(move |c| {
            let Some((source, _)) = find_label(c, &from)? else {
                return Ok(Err(Refusal::UnknownLabel(from)));
            };
            let Some((target, _)) = find_label(c, &into)? else {
                return Ok(Err(Refusal::UnknownLabel(into)));
            };
            if source == target {
                return Ok(Err(Refusal::SameLabel));
            }
            for (table, column) in [
                ("label_tracks", "identity_id"),
                ("label_albums", "album_key"),
                ("label_artists", "artist_fold"),
            ] {
                c.execute(
                    &format!(
                        "INSERT OR IGNORE INTO {table} (label_id, {column})
                         SELECT ?2, {column} FROM {table} WHERE label_id = ?1"
                    ),
                    params![source, target],
                )?;
            }
            c.prepare_cached("DELETE FROM labels WHERE id = ?1")?
                .execute([source])?;
            Ok(Ok(()))
        })?;
        Ok(done?)
    }

    /// Removes the label `name` from everything that has it.
    pub fn delete_label(&self, name: &str) -> Result<(), LabelError> {
        let name = clean_label(name)?;
        let done = self.database().writer()?.write(move |c| {
            let Some((label, _)) = find_label(c, &name)? else {
                return Ok(Err(Refusal::UnknownLabel(name)));
            };
            c.prepare_cached("DELETE FROM labels WHERE id = ?1")?
                .execute([label])?;
            Ok(Ok(()))
        })?;
        Ok(done?)
    }

    /// The labels of `target`, in name order: for tracks, those given to them and
    /// those they have from their albums and artists.
    pub fn labels_of(&self, target: &LabelTarget) -> Result<ItemLabels, DbError> {
        let target = target.clone();
        self.database().read(move |c| {
            let order = "ORDER BY name COLLATE HK_LOCALE";
            Ok(match &target {
                LabelTarget::Tracks(tracks) => {
                    let mut own: Vec<String> = Vec::new();
                    let mut inherited: Vec<String> = Vec::new();
                    for track in tracks {
                        let id = sql_track_id(*track);
                        for name in names(
                            c,
                            &format!("SELECT name FROM labels WHERE id IN ({OWN}) {order}"),
                            &id,
                        )? {
                            if !own.contains(&name) {
                                own.push(name);
                            }
                        }
                        for name in names(
                            c,
                            &format!("SELECT name FROM labels WHERE id IN ({INHERITED}) {order}"),
                            &id,
                        )? {
                            if !inherited.contains(&name) {
                                inherited.push(name);
                            }
                        }
                    }
                    inherited.retain(|name| !own.contains(name));
                    ItemLabels { own, inherited }
                }
                LabelTarget::Album(_) | LabelTarget::AlbumOf(_) => {
                    let album = match &target {
                        LabelTarget::Album(_) => "?1",
                        _ => "(SELECT album_id FROM tracks WHERE id = ?1)",
                    };
                    let id = match &target {
                        LabelTarget::Album(album) => album.get(),
                        LabelTarget::AlbumOf(track) => sql_track_id(*track),
                        _ => -1,
                    };
                    let own = names(
                        c,
                        &format!(
                            "SELECT name FROM labels WHERE id IN (SELECT la.label_id
                                FROM label_albums la JOIN albums al ON al.album_key = la.album_key
                                WHERE al.id = {album}) {order}"
                        ),
                        &id,
                    )?;
                    ItemLabels {
                        own,
                        inherited: Vec::new(),
                    }
                }
                LabelTarget::Artist(name) => {
                    let own = names(
                        c,
                        &format!(
                            "SELECT name FROM labels WHERE id IN (SELECT label_id
                                FROM label_artists WHERE artist_fold = ?1) {order}"
                        ),
                        &fold(name.trim()),
                    )?;
                    ItemLabels {
                        own,
                        inherited: Vec::new(),
                    }
                }
            })
        })
    }

    /// Every label of each of `tracks`, own and inherited, in name order, as the
    /// labels column of a track list shows them: one read for the lot. Tracks the
    /// library does not have are left out; a track without labels has an empty
    /// list.
    pub fn track_labels(&self, tracks: &[TrackId]) -> Result<Vec<(TrackId, Vec<String>)>, DbError> {
        let tracks = tracks.to_vec();
        self.database().read(move |c| {
            let mut known = c.prepare_cached("SELECT 1 FROM tracks WHERE id = ?1")?;
            let mut out = Vec::with_capacity(tracks.len());
            for track in tracks {
                let id = sql_track_id(track);
                if known.exists([id])? {
                    out.push((track, of_track(c, id)?));
                }
            }
            Ok(out)
        })
    }

    /// Every label with the tracks, albums and artists it was given to, for an
    /// export of the user's data. Tracks and albums that are not in the library now
    /// are left out.
    pub fn label_items(&self) -> Result<Vec<LabelItems>, DbError> {
        self.database().read(|c| {
            let labels: Vec<(i64, String)> = c
                .prepare("SELECT id, name FROM labels ORDER BY name COLLATE HK_LOCALE, id")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut out = Vec::with_capacity(labels.len());
            for (id, name) in labels {
                let tracks = c
                    .prepare_cached(
                        "SELECT t.id FROM label_tracks lt
                         JOIN tracks t ON t.identity_id = lt.identity_id
                         WHERE lt.label_id = ?1 ORDER BY t.id",
                    )?
                    .query_map([id], |row| row.get(0).map(track_id_from_sql))?
                    .collect::<rusqlite::Result<_>>()?;
                let albums = c
                    .prepare_cached(
                        "SELECT al.id FROM label_albums la
                         JOIN albums al ON al.album_key = la.album_key
                         WHERE la.label_id = ?1 ORDER BY al.id",
                    )?
                    .query_map([id], |row| row.get(0).map(AlbumId))?
                    .collect::<rusqlite::Result<_>>()?;
                let artists = c
                    .prepare_cached(
                        "SELECT coalesce((SELECT p.name FROM people p
                                          WHERE p.fold = lr.artist_fold LIMIT 1), lr.artist_fold)
                         FROM label_artists lr WHERE lr.label_id = ?1
                         ORDER BY 1 COLLATE HK_LOCALE",
                    )?
                    .query_map([id], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                out.push(LabelItems {
                    name,
                    tracks,
                    albums,
                    artists,
                });
            }
            Ok(out)
        })
    }
}

/// Moves the labels of the identity `from` to `to`, as when copies are merged.
pub(crate) fn move_identity(c: &Connection, from: i64, to: i64) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT OR IGNORE INTO label_tracks (label_id, identity_id)
         SELECT label_id, ?2 FROM label_tracks WHERE identity_id = ?1",
    )?
    .execute(params![from, to])?;
    c.prepare_cached("DELETE FROM label_tracks WHERE identity_id = ?1")?
        .execute([from])?;
    Ok(())
}

/// Gives the identity `to` the labels of `from`, as when a track stops being a copy.
pub(crate) fn copy_identity(c: &Connection, from: i64, to: i64) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT OR IGNORE INTO label_tracks (label_id, identity_id)
         SELECT label_id, ?2 FROM label_tracks WHERE identity_id = ?1",
    )?
    .execute(params![from, to])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_label("  late \t night ").unwrap(), "late night");
        assert!(matches!(clean_label(" \n"), Err(LabelError::EmptyName)));
        assert!(matches!(
            clean_label(&"x".repeat(MAX_LABEL_CHARS + 1)),
            Err(LabelError::TooLong)
        ));
    }
}
