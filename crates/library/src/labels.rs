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
//!
//! Every change can be undone: [`Library::change_labels`] makes one and returns a
//! [`LabelUndo`], the labels it touched as they were before, and
//! [`Library::undo_labels`] puts them back. Several changes made one after the other
//! are undone together by joining their undos with [`LabelUndo::absorb`]. The undo
//! is kept by the caller, not in the library: it is meant for the moment after the
//! change, as an Undo button offers it, and is not part of the history of tag edits
//! and file runs.

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

/// A label, by its row in the library. It keeps its id through a rename, and
/// through an undo of a change that removed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LabelId(i64);

impl LabelId {
    /// The row id.
    pub fn get(self) -> i64 {
        self.0
    }
}

/// A change of labels, as [`Library::change_labels`] makes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelChange {
    /// Gives the label `name` to `target`, making it if there is none.
    Give {
        /// What gets it.
        target: LabelTarget,
        /// The label.
        name: String,
    },
    /// Takes the label `name` from `target`.
    Take {
        /// What loses it.
        target: LabelTarget,
        /// The label.
        name: String,
    },
    /// Renames the label `name` to `new_name`, which no other label may have.
    Rename {
        /// The label.
        name: String,
        /// Its new name.
        new_name: String,
    },
    /// Merges the label `from` into `into`.
    Merge {
        /// The label that goes.
        from: String,
        /// The label that stays.
        into: String,
    },
    /// Removes the label `name` from everything that has it.
    Delete {
        /// The label.
        name: String,
    },
}

/// A label as it was stored, for an undo.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredLabel {
    name: String,
    fold: String,
    created_at: i64,
    identities: Vec<i64>,
    albums: Vec<String>,
    artists: Vec<String>,
    /// The rows of `import_run_labels`: run, target, identity and item key.
    imports: Vec<(i64, i64, Option<i64>, Option<String>)>,
}

/// What undoes label changes: each label they touched, as it was before the
/// first of them, or `None` for a label they made.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabelUndo {
    labels: Vec<(i64, Option<StoredLabel>)>,
}

impl LabelUndo {
    /// Whether there is nothing to undo.
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    /// Joins the undo of `later`, a change made after the ones of `self`, so that
    /// undoing the whole puts the labels back as they were before the first.
    pub fn absorb(&mut self, later: LabelUndo) {
        for (id, state) in later.labels {
            if !self.labels.iter().any(|(known, _)| *known == id) {
                self.labels.push((id, state));
            }
        }
    }

    /// Notes the label `id` as it is now, unless it was noted before.
    fn touch(&mut self, c: &Connection, id: i64) -> rusqlite::Result<()> {
        if self.labels.iter().any(|(known, _)| *known == id) {
            return Ok(());
        }
        let state = stored(c, id)?;
        self.labels.push((id, state));
        Ok(())
    }

    /// Notes the label `id`, which the change made.
    fn made(&mut self, id: i64) {
        if !self.labels.iter().any(|(known, _)| *known == id) {
            self.labels.push((id, None));
        }
    }
}

/// The label `id` with everything that has it, as stored.
fn stored(c: &Connection, id: i64) -> rusqlite::Result<Option<StoredLabel>> {
    let Some((name, fold, created_at)) = c
        .prepare_cached("SELECT name, fold, created_at FROM labels WHERE id = ?1")?
        .query_row([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?
    else {
        return Ok(None);
    };
    let column = |sql: &str| -> rusqlite::Result<Vec<String>> {
        c.prepare_cached(sql)?
            .query_map([id], |row| row.get(0))?
            .collect()
    };
    let identities = c
        .prepare_cached("SELECT identity_id FROM label_tracks WHERE label_id = ?1")?
        .query_map([id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let imports = c
        .prepare_cached(
            "SELECT run_id, target, identity_id, item_key FROM import_run_labels
             WHERE label_id = ?1",
        )?
        .query_map([id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(StoredLabel {
        name,
        fold,
        created_at,
        identities,
        albums: column("SELECT album_key FROM label_albums WHERE label_id = ?1")?,
        artists: column("SELECT artist_fold FROM label_artists WHERE label_id = ?1")?,
        imports,
    }))
}

/// Puts back the labels of `undo`.
fn restore(c: &Connection, undo: &LabelUndo) -> rusqlite::Result<()> {
    // The labels the changes made go first, so that a label they renamed can take
    // its old name again.
    for (id, state) in &undo.labels {
        if state.is_none() {
            c.prepare_cached("DELETE FROM labels WHERE id = ?1")?
                .execute([id])?;
        }
    }
    for (id, state) in &undo.labels {
        let Some(state) = state else { continue };
        let exists: bool = c
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM labels WHERE id = ?1)")?
            .query_row([id], |row| row.get(0))?;
        // A label made since with the old name takes the rows instead.
        let holder: Option<i64> = c
            .prepare_cached("SELECT id FROM labels WHERE fold = ?1 AND id != ?2")?
            .query_row(params![state.fold, id], |row| row.get(0))
            .optional()?;
        let label = match holder {
            Some(other) => {
                c.prepare_cached("DELETE FROM labels WHERE id = ?1")?
                    .execute([id])?;
                other
            }
            None if exists => {
                c.prepare_cached("UPDATE labels SET name = ?2, fold = ?3 WHERE id = ?1")?
                    .execute(params![id, state.name, state.fold])?;
                for table in ["label_tracks", "label_albums", "label_artists"] {
                    c.execute(&format!("DELETE FROM {table} WHERE label_id = ?1"), [id])?;
                }
                *id
            }
            None => {
                c.prepare_cached(
                    "INSERT INTO labels (id, name, fold, created_at) VALUES (?1, ?2, ?3, ?4)",
                )?
                .execute(params![id, state.name, state.fold, state.created_at])?;
                *id
            }
        };
        let mut track = c.prepare_cached(
            "INSERT OR IGNORE INTO label_tracks (label_id, identity_id)
             SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM identities WHERE id = ?2)",
        )?;
        for identity in &state.identities {
            track.execute(params![label, identity])?;
        }
        let mut album = c.prepare_cached(
            "INSERT OR IGNORE INTO label_albums (label_id, album_key) VALUES (?1, ?2)",
        )?;
        for key in &state.albums {
            album.execute(params![label, key])?;
        }
        let mut artist = c.prepare_cached(
            "INSERT OR IGNORE INTO label_artists (label_id, artist_fold) VALUES (?1, ?2)",
        )?;
        for fold in &state.artists {
            artist.execute(params![label, fold])?;
        }
        // A label that was removed lost its part in the imports; it gets it back.
        if !exists && holder.is_none() {
            let mut import = c.prepare_cached(
                "INSERT INTO import_run_labels (run_id, label_id, target, identity_id, item_key)
                 SELECT ?1, ?2, ?3, ?4, ?5
                 WHERE EXISTS (SELECT 1 FROM import_runs WHERE id = ?1)
                   AND (?4 IS NULL OR EXISTS (SELECT 1 FROM identities WHERE id = ?4))",
            )?;
            for (run, target, identity, key) in &state.imports {
                import.execute(params![run, label, target, identity, key])?;
            }
        }
        drop_if_unused(c, label)?;
    }
    Ok(())
}

/// How a label stands for the items of a [`LabelTarget`], for choosing labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelMark {
    /// The label.
    pub id: LabelId,
    /// Its name.
    pub name: String,
    /// How many of the items have it given to them.
    pub own: usize,
    /// How many of the tracks without it of their own have it from their album.
    pub from_album: usize,
    /// How many of the tracks without it of their own or from their album have it
    /// from one of their artists.
    pub from_artist: usize,
}

/// The labels of the items of a [`LabelTarget`]: how many items there are, and
/// every label that at least one of them has, in name order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabelMarks {
    /// The tracks, or 1 for an album or an artist the library has.
    pub items: usize,
    /// The labels.
    pub labels: Vec<LabelMark>,
}

/// A label with what has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelSummary {
    /// The label.
    pub id: LabelId,
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

/// The ids of the labels track `?1` has from its album.
const FROM_ALBUM: &str = "SELECT la.label_id FROM label_albums la
        JOIN albums al ON al.album_key = la.album_key
        WHERE al.id = (SELECT album_id FROM tracks WHERE id = ?1)";

/// The ids of the labels track `?1` has from its artists.
const FROM_ARTISTS: &str = "SELECT lr.label_id FROM label_artists lr
        JOIN people p ON p.fold = lr.artist_fold
        JOIN track_people tp ON tp.person_id = p.id
        WHERE tp.track_id = ?1 AND tp.role = 0
    UNION SELECT lr.label_id FROM label_artists lr
        JOIN people p ON p.fold = lr.artist_fold
        JOIN album_people ap ON ap.person_id = p.id
        WHERE ap.album_id = (SELECT album_id FROM tracks WHERE id = ?1)";

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
fn change_rows(c: &Connection, label: i64, keys: &Keys, add: bool) -> rusqlite::Result<usize> {
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

/// Makes `change`, whose names are clean, in one transaction.
fn apply(
    c: &Connection,
    change: &LabelChange,
    now: i64,
) -> rusqlite::Result<Result<(usize, LabelUndo), Refusal>> {
    let mut undo = LabelUndo::default();
    match change {
        LabelChange::Give { target, name } => {
            let keys = match keys(c, target)? {
                Ok(keys) => keys,
                Err(refusal) => return Ok(Err(refusal)),
            };
            if matches!(&keys, Keys::Identities(ids) if ids.is_empty()) {
                return Ok(Ok((0, undo)));
            }
            if let Some((known, _)) = find_label(c, name)? {
                undo.touch(c, known)?;
            }
            let label = label_id(c, name, now)?;
            undo.made(label);
            Ok(Ok((change_rows(c, label, &keys, true)?, undo)))
        }
        LabelChange::Take { target, name } => {
            let Some((label, _)) = find_label(c, name)? else {
                return Ok(Err(Refusal::UnknownLabel(name.clone())));
            };
            let keys = match keys(c, target)? {
                Ok(keys) => keys,
                Err(refusal) => return Ok(Err(refusal)),
            };
            undo.touch(c, label)?;
            let changed = change_rows(c, label, &keys, false)?;
            drop_if_unused(c, label)?;
            Ok(Ok((changed, undo)))
        }
        LabelChange::Rename { name, new_name } => {
            let Some((label, _)) = find_label(c, name)? else {
                return Ok(Err(Refusal::UnknownLabel(name.clone())));
            };
            if let Some((other, other_name)) = find_label(c, new_name)?
                && other != label
            {
                return Ok(Err(Refusal::Exists(other_name)));
            }
            undo.touch(c, label)?;
            c.prepare_cached("UPDATE labels SET name = ?2, fold = ?3 WHERE id = ?1")?
                .execute(params![label, new_name, fold(new_name)])?;
            Ok(Ok((0, undo)))
        }
        LabelChange::Merge { from, into } => {
            let Some((source, _)) = find_label(c, from)? else {
                return Ok(Err(Refusal::UnknownLabel(from.clone())));
            };
            let Some((target, _)) = find_label(c, into)? else {
                return Ok(Err(Refusal::UnknownLabel(into.clone())));
            };
            if source == target {
                return Ok(Err(Refusal::SameLabel));
            }
            undo.touch(c, source)?;
            undo.touch(c, target)?;
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
            Ok(Ok((0, undo)))
        }
        LabelChange::Delete { name } => {
            let Some((label, _)) = find_label(c, name)? else {
                return Ok(Err(Refusal::UnknownLabel(name.clone())));
            };
            undo.touch(c, label)?;
            c.prepare_cached("DELETE FROM labels WHERE id = ?1")?
                .execute([label])?;
            Ok(Ok((0, undo)))
        }
    }
}

impl Library {
    /// Every label, in name order, with how many tracks, albums and artists have it.
    pub fn labels(&self) -> Result<Vec<LabelSummary>, DbError> {
        let tracks = has_label("SELECT l.id");
        self.database().read(move |c| {
            c.prepare(&format!(
                "SELECT l.id, l.name,
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
                    id: LabelId(row.get(0)?),
                    name: row.get(1)?,
                    tracks: count(2)?,
                    own_tracks: count(3)?,
                    albums: count(4)?,
                    artists: count(5)?,
                })
            })?
            .collect()
        })
    }

    /// Gives the label `name` to `target`, making the label if there is none (a
    /// label that differs only in case or accents is the same label, and takes
    /// this spelling). How many tracks, albums or artists did not have it before.
    pub fn add_label(&self, target: &LabelTarget, name: &str) -> Result<usize, LabelError> {
        let change = LabelChange::Give {
            target: target.clone(),
            name: name.to_owned(),
        };
        Ok(self.change_labels(&change)?.0)
    }

    /// Takes the label `name` from `target`; a label nothing has any more is
    /// removed. How many tracks, albums or artists had it.
    pub fn remove_label(&self, target: &LabelTarget, name: &str) -> Result<usize, LabelError> {
        let change = LabelChange::Take {
            target: target.clone(),
            name: name.to_owned(),
        };
        Ok(self.change_labels(&change)?.0)
    }

    /// Renames the label `name` to `new_name`. A new name that is another label's
    /// fails with [`LabelError::Exists`]: [`merge_labels`](Self::merge_labels)
    /// joins two labels.
    pub fn rename_label(&self, name: &str, new_name: &str) -> Result<(), LabelError> {
        let change = LabelChange::Rename {
            name: name.to_owned(),
            new_name: new_name.to_owned(),
        };
        self.change_labels(&change).map(|_| ())
    }

    /// Merges the label `from` into the label `into`: whatever has `from` has
    /// `into` instead, and `from` is removed.
    pub fn merge_labels(&self, from: &str, into: &str) -> Result<(), LabelError> {
        let change = LabelChange::Merge {
            from: from.to_owned(),
            into: into.to_owned(),
        };
        self.change_labels(&change).map(|_| ())
    }

    /// Removes the label `name` from everything that has it.
    pub fn delete_label(&self, name: &str) -> Result<(), LabelError> {
        let change = LabelChange::Delete {
            name: name.to_owned(),
        };
        self.change_labels(&change).map(|_| ())
    }

    /// Makes `change`, and returns how many tracks, albums or artists got or lost
    /// the label (for [`LabelChange::Give`] and [`LabelChange::Take`]; 0 for the
    /// others) with what undoes it (see [`undo_labels`](Self::undo_labels)).
    pub fn change_labels(&self, change: &LabelChange) -> Result<(usize, LabelUndo), LabelError> {
        let now = to_unix_seconds(SystemTime::now());
        let change = match change {
            LabelChange::Give { target, name } => LabelChange::Give {
                target: target.clone(),
                name: clean_label(name)?,
            },
            LabelChange::Take { target, name } => LabelChange::Take {
                target: target.clone(),
                name: clean_label(name)?,
            },
            LabelChange::Rename { name, new_name } => LabelChange::Rename {
                name: clean_label(name)?,
                new_name: clean_label(new_name)?,
            },
            LabelChange::Merge { from, into } => LabelChange::Merge {
                from: clean_label(from)?,
                into: clean_label(into)?,
            },
            LabelChange::Delete { name } => LabelChange::Delete {
                name: clean_label(name)?,
            },
        };
        let done = self
            .database()
            .writer()?
            .write(move |c| apply(c, &change, now))?;
        Ok(done?)
    }

    /// Puts back the labels `undo` noted, as they were before the changes it
    /// undoes. What was given or taken since to the same labels is undone too.
    pub fn undo_labels(&self, undo: &LabelUndo) -> Result<(), DbError> {
        let undo = undo.clone();
        self.database().writer()?.write(move |c| restore(c, &undo))
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
            labels
                .into_iter()
                .map(|(id, name)| items_of(c, id, name))
                .collect()
        })
    }

    /// The label `name` with the tracks, albums and artists it was given to, as
    /// [`label_items`](Self::label_items) lists them; `None` when there is no such
    /// label.
    pub fn label_items_of(&self, name: &str) -> Result<Option<LabelItems>, DbError> {
        let name = name.to_owned();
        self.database().read(move |c| {
            find_label(c, &name)?
                .map(|(id, name)| items_of(c, id, name))
                .transpose()
        })
    }

    /// The labels the items of `target` have, with how many have each, for
    /// choosing labels for them. An album or artist the library does not have has
    /// no items.
    pub fn label_marks(&self, target: &LabelTarget) -> Result<LabelMarks, DbError> {
        let target = target.clone();
        self.database().read(move |c| {
            // By label: own, from the album, from an artist.
            let mut counts: Vec<(i64, [usize; 3])> = Vec::new();
            let mut count =
                |id: i64, which: usize| match counts.iter_mut().find(|(known, _)| *known == id) {
                    Some((_, n)) => n[which] += 1,
                    None => {
                        let mut n = [0; 3];
                        n[which] = 1;
                        counts.push((id, n));
                    }
                };
            let ids = |sql: &str, param: &dyn rusqlite::ToSql| -> rusqlite::Result<Vec<i64>> {
                c.prepare_cached(sql)?
                    .query_map([param], |row| row.get(0))?
                    .collect()
            };
            let items = match &target {
                LabelTarget::Tracks(tracks) => {
                    let mut items = 0;
                    let mut seen = Vec::with_capacity(tracks.len());
                    for track in tracks {
                        let id = sql_track_id(*track);
                        let known: bool = c
                            .prepare_cached("SELECT EXISTS (SELECT 1 FROM tracks WHERE id = ?1)")?
                            .query_row([id], |row| row.get(0))?;
                        if !known || seen.contains(&id) {
                            continue;
                        }
                        seen.push(id);
                        items += 1;
                        let own = ids(OWN, &id)?;
                        let album = ids(FROM_ALBUM, &id)?;
                        for label in &own {
                            count(*label, 0);
                        }
                        for label in album.iter().filter(|l| !own.contains(l)) {
                            count(*label, 1);
                        }
                        for label in ids(FROM_ARTISTS, &id)?
                            .iter()
                            .filter(|l| !own.contains(l) && !album.contains(l))
                        {
                            count(*label, 2);
                        }
                    }
                    items
                }
                LabelTarget::Album(_) | LabelTarget::AlbumOf(_) => {
                    let key: Option<String> = match &target {
                        LabelTarget::Album(album) => c
                            .prepare_cached("SELECT album_key FROM albums WHERE id = ?1")?
                            .query_row([album.get()], |row| row.get(0))
                            .optional()?,
                        LabelTarget::AlbumOf(track) => c
                            .prepare_cached(
                                "SELECT a.album_key FROM tracks t
                                 JOIN albums a ON a.id = t.album_id WHERE t.id = ?1",
                            )?
                            .query_row([sql_track_id(*track)], |row| row.get(0))
                            .optional()?,
                        _ => None,
                    };
                    match key {
                        Some(key) => {
                            for label in ids(
                                "SELECT label_id FROM label_albums WHERE album_key = ?1",
                                &key,
                            )? {
                                count(label, 0);
                            }
                            1
                        }
                        None => 0,
                    }
                }
                LabelTarget::Artist(name) => {
                    let folded = fold(name.trim());
                    let known: bool = c
                        .prepare_cached("SELECT EXISTS (SELECT 1 FROM people WHERE fold = ?1)")?
                        .query_row([&folded], |row| row.get(0))?;
                    if known {
                        for label in ids(
                            "SELECT label_id FROM label_artists WHERE artist_fold = ?1",
                            &folded,
                        )? {
                            count(label, 0);
                        }
                        1
                    } else {
                        0
                    }
                }
            };
            let mut labels = Vec::with_capacity(counts.len());
            if !counts.is_empty() {
                let mut all = c.prepare_cached(
                    "SELECT id, name FROM labels ORDER BY name COLLATE HK_LOCALE, id",
                )?;
                let mut rows = all.query([])?;
                while let Some(row) = rows.next()? {
                    let id: i64 = row.get(0)?;
                    if let Some((_, [own, from_album, from_artist])) =
                        counts.iter().find(|(known, _)| *known == id)
                    {
                        labels.push(LabelMark {
                            id: LabelId(id),
                            name: row.get(1)?,
                            own: *own,
                            from_album: *from_album,
                            from_artist: *from_artist,
                        });
                    }
                }
            }
            Ok(LabelMarks { items, labels })
        })
    }
}

/// The label `id` called `name` with what it was given to.
fn items_of(c: &Connection, id: i64, name: String) -> rusqlite::Result<LabelItems> {
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
    Ok(LabelItems {
        name,
        tracks,
        albums,
        artists,
    })
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
