// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The preview of the organizer: where the pattern puts each file, and what would
//! happen to it.
//!
//! The values of the fields come from the library (the tags as it read them) and
//! from the file's first track; the converter and folder sync take theirs from
//! here too ([`Library::naming_values`](crate::Library::naming_values)), so that
//! a pattern names a track alike in every tool. The pattern pads the numbers.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use helikodu_model::library::TrackId;
use helikodu_model::query::Field;
use rusqlite::{Connection, OptionalExtension, params};

use super::{
    Change, Destination, FieldMap, NamingPattern, OrganizeError, OrganizeOptions, PlannedMove,
};
use crate::catalog::write::file_at;
use crate::catalog::{sql_track_id, track_id_from_sql};
use crate::copies::FileId;
use crate::db::{Database, path_from_blob};

/// The library folders that are not masked: their rows and paths.
pub(super) fn roots(c: &Connection) -> rusqlite::Result<Vec<(i64, PathBuf)>> {
    c.prepare_cached("SELECT id, path FROM roots WHERE masked = 0")?
        .query_map([], |row| Ok((row.get(0)?, path_from_blob(row.get(1)?))))?
        .collect()
}

/// The library folder that holds `path`: the one with the longest path.
pub(super) fn root_of(roots: &[(i64, PathBuf)], path: &Path) -> Option<(i64, PathBuf)> {
    roots
        .iter()
        .filter(|(_, root)| path.starts_with(root) && path != root)
        .max_by_key(|(_, root)| root.as_os_str().len())
        .cloned()
}

/// What the preview knows of one file.
struct FileFacts {
    file: i64,
    track: i64,
    path: PathBuf,
    root: PathBuf,
    missing: bool,
    cue: bool,
    album: Option<i64>,
    values: HashMap<&'static str, String>,
    track_no: Option<i64>,
    track_total: Option<i64>,
    disc_no: Option<i64>,
    disc_total: Option<i64>,
}

const COLUMNS: &str = "f.id, t.id, d.path, f.name, r.path, f.missing_since IS NOT NULL,
    EXISTS (SELECT 1 FROM tracks c WHERE c.file_id = f.id AND c.section = 1),
    t.album_id, t.title, t.artist, t.album_title,
    coalesce(nullif(t.album_artist, ''), nullif(t.main_artist, ''), t.artist),
    t.year, t.original_year, t.track_no, t.track_total, t.disc_no, t.disc_total,
    t.disc_subtitle, t.release_type, t.codec, t.container, t.sample_rate, t.bit_depth,
    t.channels, t.bitrate, t.work, t.movement, t.movement_no";

/// One track, by its id, in a library folder masked or not.
const FROM_TRACK: &str = "FROM tracks t
    JOIN files f ON f.id = t.file_id
    JOIN dirs d ON d.id = f.dir_id
    JOIN roots r ON r.id = d.root_id
    WHERE t.id = ?1";

const FROM: &str = "FROM files f
    JOIN dirs d ON d.id = f.dir_id
    JOIN roots r ON r.id = d.root_id
    JOIN tracks t ON t.id = (SELECT t2.id FROM tracks t2 WHERE t2.file_id = f.id
                             ORDER BY t2.sub_index LIMIT 1)
    WHERE r.masked = 0";

fn facts(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileFacts> {
    let mut values = HashMap::new();
    let mut text = |key: &'static str, index: usize| -> rusqlite::Result<()> {
        if let Some(value) = row.get::<_, Option<String>>(index)? {
            values.insert(key, value);
        }
        Ok(())
    };
    text("title", 8)?;
    text("artist", 9)?;
    text("album", 10)?;
    text("albumartist", 11)?;
    text("discsubtitle", 18)?;
    text("releasetype", 19)?;
    text("codec", 20)?;
    text("container", 21)?;
    text("work", 26)?;
    text("movement", 27)?;
    let mut number = |key: &'static str, index: usize| -> rusqlite::Result<()> {
        if let Some(value) = row.get::<_, Option<i64>>(index)? {
            values.insert(key, value.to_string());
        }
        Ok(())
    };
    number("year", 12)?;
    number("originalyear", 13)?;
    number("samplerate", 22)?;
    number("bitdepth", 23)?;
    number("channels", 24)?;
    number("movementno", 28)?;
    if let Some(bitrate) = row.get::<_, Option<i64>>(25)? {
        values.insert("bitrate", (bitrate / 1000).to_string());
    }
    let dir = path_from_blob(row.get(2)?);
    let name = path_from_blob(row.get(3)?);
    Ok(FileFacts {
        file: row.get(0)?,
        track: row.get(1)?,
        path: dir.join(name),
        root: path_from_blob(row.get(4)?),
        missing: row.get(5)?,
        cue: row.get(6)?,
        album: row.get(7)?,
        values,
        track_no: row.get(14)?,
        track_total: row.get(15)?,
        disc_no: row.get(16)?,
        disc_total: row.get(17)?,
    })
}

/// The files of `tracks`, or every file.
fn load(c: &Connection, tracks: Option<&[TrackId]>) -> rusqlite::Result<Vec<FileFacts>> {
    let mut out = Vec::new();
    match tracks {
        None => {
            let mut statement = c.prepare(&format!("SELECT {COLUMNS} {FROM}"))?;
            for row in statement.query_map([], facts)? {
                out.push(row?);
            }
        }
        Some(tracks) => {
            let mut files: Vec<i64> = Vec::new();
            let mut seen = HashSet::new();
            let mut statement = c.prepare_cached("SELECT file_id FROM tracks WHERE id = ?1")?;
            for id in tracks.iter().copied().map(sql_track_id) {
                if let Some(file) = statement
                    .query_row([id], |row| row.get::<_, i64>(0))
                    .optional()?
                    && seen.insert(file)
                {
                    files.push(file);
                }
            }
            let mut statement =
                c.prepare_cached(&format!("SELECT {COLUMNS} {FROM} AND f.id = ?1"))?;
            for file in files {
                if let Some(found) = statement.query_row([file], facts).optional()? {
                    out.push(found);
                }
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// The names in `role` of `track`, in order.
fn people(c: &Connection, track: i64, role: i64) -> rusqlite::Result<Vec<String>> {
    c.prepare_cached(
        "SELECT p.name FROM track_people tp JOIN people p ON p.id = tp.person_id
             WHERE tp.track_id = ?1 AND tp.role = ?2 ORDER BY tp.position",
    )?
    .query_map(params![track, role], |row| row.get(0))?
    .collect()
}

/// The genres of `track`, in order.
fn genres(c: &Connection, track: i64) -> rusqlite::Result<Vec<String>> {
    c.prepare_cached(
        "SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
             WHERE tg.track_id = ?1 ORDER BY tg.position",
    )?
    .query_map([track], |row| row.get(0))?
    .collect()
}

/// The values of the tag `key` of `track`, in order.
fn other_tag(c: &Connection, track: i64, key: &str) -> rusqlite::Result<Vec<String>> {
    c.prepare_cached(
        "SELECT value FROM track_tags WHERE track_id = ?1 AND upper(key) = ?2
             ORDER BY rowid",
    )?
    .query_map(params![track, key], |row| row.get(0))?
    .collect()
}

/// The highest disc number and number of discs of each album.
fn album_discs(c: &Connection) -> rusqlite::Result<HashMap<i64, i64>> {
    c.prepare_cached(
        "SELECT album_id, max(max(coalesce(disc_no, 1)), coalesce(max(disc_total), 1))
         FROM tracks WHERE album_id IS NOT NULL GROUP BY album_id",
    )?
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
    .collect()
}

/// The values of the fields of `facts` (only `fields`, or every field and tag
/// when `None`) for a pattern.
fn values_of(
    c: &Connection,
    facts: &FileFacts,
    discs: &HashMap<i64, i64>,
    fields: Option<&[Field]>,
) -> rusqlite::Result<FieldMap> {
    let to_u32 = |n: i64| u32::try_from(n).ok();
    let mut map = FieldMap {
        track_count: facts.track_total.and_then(to_u32),
        disc_count: facts
            .album
            .and_then(|a| discs.get(&a).copied())
            .into_iter()
            .chain(facts.disc_total)
            .max()
            .and_then(to_u32),
        ..FieldMap::default()
    };
    let put = |map: &mut FieldMap, field: &Field| -> rusqlite::Result<()> {
        match list_of(c, facts, field)? {
            Some(list) => map.set_list(field.clone(), list),
            None => {
                if let Some(value) = value_of(facts, field) {
                    map.set(field.clone(), value);
                }
            }
        }
        Ok(())
    };
    match fields {
        Some(fields) => {
            for field in fields {
                put(&mut map, field)?;
            }
        }
        None => {
            for field in Field::named() {
                put(&mut map, &field)?;
            }
            let tags: Vec<(String, String)> = c
                .prepare_cached(
                    "SELECT upper(key), value FROM track_tags WHERE track_id = ?1 ORDER BY rowid",
                )?
                .query_map([facts.track], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut joined: Vec<(String, Vec<String>)> = Vec::new();
            for (key, value) in tags {
                match joined.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, values)) => values.push(value),
                    None => joined.push((key, vec![value])),
                }
            }
            for (key, values) in joined {
                map.set_list(Field::Tag(key), values);
            }
        }
    }
    Ok(map)
}

/// The values of `field` for `facts` when it is a field of several values.
fn list_of(
    c: &Connection,
    facts: &FileFacts,
    field: &Field,
) -> rusqlite::Result<Option<Vec<String>>> {
    Ok(Some(match field {
        Field::Composer => people(c, facts.track, 2)?,
        Field::Performer => people(c, facts.track, 3)?,
        Field::Conductor => people(c, facts.track, 4)?,
        Field::Lyricist => people(c, facts.track, 5)?,
        Field::Genre => genres(c, facts.track)?,
        Field::Tag(key) => other_tag(c, facts.track, key)?,
        _ => return Ok(None),
    }))
}

/// The value of `field` for `facts`, numbers as plain numbers.
fn value_of(facts: &FileFacts, field: &Field) -> Option<String> {
    match field {
        Field::Track => facts.track_no.map(|n| n.to_string()),
        Field::Disc => facts.disc_no.map(|n| n.to_string()),
        Field::Title => facts
            .values
            .get("title")
            .filter(|t| !t.trim().is_empty())
            .cloned()
            .or_else(|| {
                facts
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            }),
        Field::Filename => facts
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned()),
        Field::Folder => facts
            .path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned()),
        other => facts.values.get(other.name().as_ref()).cloned(),
    }
}

/// The values of every field and tag of each of `tracks` that the library has,
/// for a naming pattern: what the organizer names its files by.
pub(super) fn naming_values(
    c: &Connection,
    tracks: &[TrackId],
) -> rusqlite::Result<Vec<(TrackId, FieldMap)>> {
    naming_values_of(c, tracks, None)
}

/// [`naming_values`] of only `fields` (every field and tag when `None`).
pub(crate) fn naming_values_of(
    c: &Connection,
    tracks: &[TrackId],
    fields: Option<&[Field]>,
) -> rusqlite::Result<Vec<(TrackId, FieldMap)>> {
    let discs = album_discs(c)?;
    let mut statement = c.prepare_cached(&format!("SELECT {COLUMNS} {FROM_TRACK}"))?;
    let mut out = Vec::with_capacity(tracks.len());
    for &track in tracks {
        if let Some(facts) = statement
            .query_row([sql_track_id(track)], facts)
            .optional()?
        {
            out.push((track, values_of(c, &facts, &discs, fields)?));
        }
    }
    Ok(out)
}

/// The extension of `path` with its dot, or nothing.
fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default()
}

pub(super) fn plan(
    db: &Database,
    tracks: Option<&[TrackId]>,
    pattern: &NamingPattern,
    options: &OrganizeOptions,
) -> Result<Vec<PlannedMove>, OrganizeError> {
    let fields = pattern.fields();
    let into = options.into.clone();
    let (files, targets) = db
        .read(|c| {
            let files = load(c, tracks)?;
            let roots = roots(c)?;
            let discs = album_discs(c)?;
            let base_folder = match &into {
                Destination::OwnLibraryFolder => None,
                Destination::Folder(folder) => {
                    if root_of(&roots, folder).is_none() && !roots.iter().any(|(_, r)| r == folder)
                    {
                        return Ok(Err(folder.clone()));
                    }
                    Some(folder.clone())
                }
            };
            let mut targets = Vec::with_capacity(files.len());
            for facts in &files {
                let values = values_of(c, facts, &discs, Some(&fields))?;
                let ext = extension(&facts.path);
                let names = pattern.render(&values, options.characters, &options.unknown, &ext);
                let mut to = base_folder.clone().unwrap_or_else(|| facts.root.clone());
                let count = names.len();
                for (i, name) in names.into_iter().enumerate() {
                    if i + 1 == count {
                        to.push(format!("{name}{ext}"));
                    } else {
                        to.push(name);
                    }
                }
                targets.push(to);
            }
            Ok(Ok((files, targets)))
        })?
        .map_err(OrganizeError::OutsideLibrary)?;
    // What happens to each.
    let mut claimed: HashSet<PathBuf> = files
        .iter()
        .zip(&targets)
        .filter(|(f, to)| f.path == **to)
        .map(|(_, to)| to.clone())
        .collect();
    let mut planned = Vec::with_capacity(files.len());
    for (facts, to) in files.iter().zip(targets) {
        let change = if facts.missing {
            Change::Missing
        } else if facts.cue {
            Change::CueSheet
        } else if facts.path == to {
            Change::InPlace
        } else if claimed.contains(&to) || taken(db, facts, &to)? {
            Change::NameTaken
        } else {
            claimed.insert(to.clone());
            let same_folder = facts.path.parent() == to.parent();
            let same_name = facts.path.file_name() == to.file_name();
            match (same_folder, same_name) {
                (true, _) => Change::Rename,
                (false, true) => Change::Move,
                (false, false) => Change::MoveAndRename,
            }
        };
        planned.push(PlannedMove {
            file: FileId(facts.file),
            track: track_id_from_sql(facts.track),
            from: facts.path.clone(),
            to,
            change,
        });
    }
    Ok(planned)
}

/// Whether `to` is taken: by a file on the disk that is not the file itself (a
/// name in other capitals on a file system that ignores them), or by another
/// library file.
fn taken(db: &Database, facts: &FileFacts, to: &Path) -> Result<bool, OrganizeError> {
    if let Ok(there) = fs::symlink_metadata(to) {
        let same = fs::symlink_metadata(&facts.path)
            .is_ok_and(|here| here.dev() == there.dev() && here.ino() == there.ino());
        if !same {
            return Ok(true);
        }
    }
    let path = to.to_path_buf();
    let other = db.read(move |c| file_at(c, &path))?;
    Ok(other.is_some_and(|id| id != facts.file))
}

/// The paths of the present files of `tracks` (every file when `None`), sorted.
pub(super) fn present_files(
    c: &Connection,
    tracks: Option<&[TrackId]>,
) -> rusqlite::Result<Vec<PathBuf>> {
    let row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<PathBuf> {
        Ok(path_from_blob(row.get(0)?).join(path_from_blob(row.get(1)?)))
    };
    let mut out: Vec<PathBuf> = match tracks {
        None => c
            .prepare(
                "SELECT d.path, f.name FROM files f JOIN dirs d ON d.id = f.dir_id
                 JOIN roots r ON r.id = d.root_id
                 WHERE r.masked = 0 AND f.missing_since IS NULL
                   AND EXISTS (SELECT 1 FROM tracks t WHERE t.file_id = f.id)",
            )?
            .query_map([], row)?
            .collect::<rusqlite::Result<_>>()?,
        Some(tracks) => {
            let mut statement = c.prepare_cached(
                "SELECT d.path, f.name FROM tracks t JOIN files f ON f.id = t.file_id
                 JOIN dirs d ON d.id = f.dir_id
                 WHERE t.id = ?1 AND f.missing_since IS NULL",
            )?;
            let mut out = Vec::new();
            for id in tracks.iter().copied().map(sql_track_id) {
                if let Some(path) = statement.query_row([id], row).optional()? {
                    out.push(path);
                }
            }
            out
        }
    };
    out.sort();
    out.dedup();
    Ok(out)
}
