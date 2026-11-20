// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Labels and the user's fields in an import of user data: what the plan finds,
//! what applying it adds, and what undoing it removes. A label is only ever
//! given, never taken; a field is only defined where the library has none of
//! that name.

use std::collections::{HashMap, HashSet};

use helikodu_model::expression::Problem;
use helikodu_model::user_fields::{DefinitionProblem, FieldDefinition, UserField, UserFields};
use rusqlite::{Connection, OptionalExtension, params};

use super::foreign::ForeignLibrary;
use super::matching::{LibraryIndex, Match};
use super::uri::PathMapping;
use super::user_data::{FIELD_COMPUTED, FIELD_CUSTOM};
use crate::labels::{clean_label, drop_if_unused};
use crate::search::fold;

/// `import_run_labels.target` of a track, an album and an artist.
const TARGET_TRACK: i64 = 0;
const TARGET_ALBUM: i64 = 1;
const TARGET_ARTIST: i64 = 2;

/// A field of the import whose name the library has for another definition: the
/// library's definition is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldConflict {
    /// The name.
    pub name: String,
    /// The library's definition, which is kept.
    pub ours: FieldDefinition,
    /// The definition in the import, left out.
    pub theirs: FieldDefinition,
}

/// What a label is given to, as the label tables key it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum LabelKey {
    /// A track's identity.
    Identity(i64),
    /// An album's key.
    Album(String),
    /// An artist's folded name.
    Artist(String),
}

impl LabelKey {
    fn table(&self) -> (&'static str, &'static str, i64) {
        match self {
            Self::Identity(_) => ("label_tracks", "identity_id", TARGET_TRACK),
            Self::Album(_) => ("label_albums", "album_key", TARGET_ALBUM),
            Self::Artist(_) => ("label_artists", "artist_fold", TARGET_ARTIST),
        }
    }

    fn value(&self) -> rusqlite::types::Value {
        match self {
            Self::Identity(id) => rusqlite::types::Value::Integer(*id),
            Self::Album(key) | Self::Artist(key) => rusqlite::types::Value::Text(key.clone()),
        }
    }
}

/// A label to give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LabelWork {
    pub(super) name: String,
    pub(super) key: LabelKey,
}

/// What the plan found for the labels and fields of an import.
#[derive(Debug, Default)]
pub(super) struct Planned {
    /// The labels to give.
    pub(super) links: Vec<LabelWork>,
    /// The labels that would be made.
    pub(super) new_labels: usize,
    /// The tracks, albums and artists of labels that match nothing here (or
    /// albums that match more than one).
    pub(super) unmatched: usize,
    /// The fields to define, as they will be stored.
    pub(super) fields: Vec<UserField>,
    /// Fields defined differently here.
    pub(super) conflicts: Vec<FieldConflict>,
    /// What was left out, in words.
    pub(super) notes: Vec<String>,
}

fn has_link(c: &Connection, label: i64, key: &LabelKey) -> rusqlite::Result<bool> {
    let (table, column, _) = key.table();
    c.prepare_cached(&format!(
        "SELECT EXISTS (SELECT 1 FROM {table} WHERE label_id = ?1 AND {column} = ?2)"
    ))?
    .query_row(params![label, key.value()], |row| row.get(0))
}

fn plan_labels(
    c: &Connection,
    index: &LibraryIndex,
    mappings: &[PathMapping],
    library: &ForeignLibrary,
    out: &mut Planned,
) -> rusqlite::Result<()> {
    if library.labels.is_empty() {
        return Ok(());
    }
    // Albums by folded album artist and title, and the folded artist names.
    let mut albums: HashMap<(String, String), Vec<String>> = HashMap::new();
    {
        let mut statement =
            c.prepare("SELECT album_key, artist, title FROM albums WHERE title IS NOT NULL")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (key, artist, title) = row?;
            albums
                .entry((fold(artist.as_deref().unwrap_or("")), fold(&title)))
                .or_default()
                .push(key);
        }
    }
    let people: HashSet<String> = c
        .prepare("SELECT DISTINCT fold FROM people")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut find = c.prepare_cached("SELECT id FROM labels WHERE fold = ?1")?;
    let mut seen: HashSet<(String, LabelKey)> = HashSet::new();
    let mut new_labels: HashSet<String> = HashSet::new();
    for label in &library.labels {
        let name = match clean_label(&label.name) {
            Ok(name) => name,
            Err(error) => {
                out.notes
                    .push(format!("the label {:?} was left out: {error}", label.name));
                continue;
            }
        };
        let folded = fold(&name);
        let id: Option<i64> = find.query_row([&folded], |row| row.get(0)).optional()?;
        let mut keys = Vec::new();
        for track in &label.tracks {
            match index.find(track, mappings) {
                Match::Track { identity, .. } => keys.push(LabelKey::Identity(identity)),
                _ => out.unmatched += 1,
            }
        }
        for album in &label.albums {
            let wanted = (
                fold(album.album_artist.as_deref().unwrap_or("")),
                fold(&album.album),
            );
            match albums.get(&wanted).map(Vec::as_slice) {
                Some([key]) => keys.push(LabelKey::Album(key.clone())),
                _ => out.unmatched += 1,
            }
        }
        for artist in &label.artists {
            let artist = fold(artist.trim());
            if people.contains(&artist) {
                keys.push(LabelKey::Artist(artist));
            } else {
                out.unmatched += 1;
            }
        }
        for key in keys {
            if !seen.insert((folded.clone(), key.clone())) {
                continue;
            }
            if let Some(id) = id
                && has_link(c, id, &key)?
            {
                continue;
            }
            if id.is_none() {
                new_labels.insert(folded.clone());
            }
            out.links.push(LabelWork {
                name: name.clone(),
                key,
            });
        }
    }
    out.new_labels = new_labels.len();
    Ok(())
}

/// Whether two definitions are the same.
fn same(ours: &FieldDefinition, theirs: &FieldDefinition) -> bool {
    match (ours, theirs) {
        (
            FieldDefinition::Computed { expression: a },
            FieldDefinition::Computed { expression: b },
        ) => a.trim() == b.trim(),
        (FieldDefinition::Tag { key: a }, FieldDefinition::Tag { key: b }) => {
            a.trim().eq_ignore_ascii_case(b.trim())
        }
        _ => false,
    }
}

fn plan_fields(
    c: &Connection,
    library: &ForeignLibrary,
    out: &mut Planned,
) -> rusqlite::Result<()> {
    if library.fields.is_empty() {
        return Ok(());
    }
    let stored = crate::user_fields::stored(c)?;
    let mut candidates: Vec<UserField> = Vec::new();
    for field in &library.fields {
        let name = field.name.trim().to_ascii_lowercase();
        if let Some(ours) = stored.iter().find(|f| f.name.eq_ignore_ascii_case(&name)) {
            if !same(&ours.definition, &field.definition) {
                out.conflicts.push(FieldConflict {
                    name,
                    ours: ours.definition.clone(),
                    theirs: field.definition.clone(),
                });
            }
            continue;
        }
        if candidates.iter().any(|f| f.name == name) {
            continue;
        }
        candidates.push(UserField {
            name,
            definition: field.definition.clone(),
        });
    }
    if candidates.is_empty() {
        return Ok(());
    }
    let mut all = stored.clone();
    all.extend(candidates.iter().cloned());
    let (set, errors) = UserFields::build_lenient(&all);
    for candidate in candidates {
        match set.get(&candidate.name) {
            Some(field) => out.fields.push(field.clone()),
            None => {
                let reason = errors
                    .iter()
                    .find(|e| e.name.trim().eq_ignore_ascii_case(&candidate.name))
                    .map_or_else(|| "it cannot be used".to_owned(), |e| e.problem.to_string());
                out.notes.push(format!(
                    "the field {} was left out: {reason}",
                    candidate.name
                ));
            }
        }
    }
    Ok(())
}

/// The labels and fields of `library` that importing it would add to the
/// library behind `c`.
pub(super) fn plan(
    c: &Connection,
    index: &LibraryIndex,
    mappings: &[PathMapping],
    library: &ForeignLibrary,
) -> rusqlite::Result<Planned> {
    let mut out = Planned::default();
    plan_labels(c, index, mappings, library, &mut out)?;
    plan_fields(c, library, &mut out)?;
    Ok(out)
}

fn kind_and_definition(field: &UserField) -> (i64, &str) {
    match &field.definition {
        FieldDefinition::Computed { expression } => (FIELD_COMPUTED, expression.as_str()),
        FieldDefinition::Tag { key } => (FIELD_CUSTOM, key.as_str()),
    }
}

/// Gives the labels of `links` and defines `fields` where still missing,
/// recording in `run` what was added.
pub(super) fn apply(
    c: &Connection,
    run: i64,
    links: &[LabelWork],
    fields: &[UserField],
    now: i64,
) -> rusqlite::Result<()> {
    for link in links {
        let folded = fold(&link.name);
        c.prepare_cached(
            "INSERT INTO labels (name, fold, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (fold) DO NOTHING",
        )?
        .execute(params![link.name, folded, now])?;
        let label: i64 = c
            .prepare_cached("SELECT id FROM labels WHERE fold = ?1")?
            .query_row([&folded], |row| row.get(0))?;
        let (table, column, target) = link.key.table();
        let added = c
            .prepare_cached(&format!(
                "INSERT OR IGNORE INTO {table} (label_id, {column}) VALUES (?1, ?2)"
            ))?
            .execute(params![label, link.key.value()])?;
        if added > 0 {
            let (identity, key) = match &link.key {
                LabelKey::Identity(id) => (Some(*id), None),
                LabelKey::Album(key) | LabelKey::Artist(key) => (None, Some(key.as_str())),
            };
            c.prepare_cached(
                "INSERT INTO import_run_labels (run_id, label_id, target, identity_id, item_key)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![run, label, target, identity, key])?;
        }
    }
    for field in fields {
        let exists: bool = c
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM user_fields WHERE name = ?1)")?
            .query_row([&field.name], |row| row.get(0))?;
        if exists {
            continue;
        }
        let (kind, definition) = kind_and_definition(field);
        c.prepare_cached(
            "INSERT INTO user_fields (name, kind, definition, position)
             VALUES (?1, ?2, ?3, (SELECT coalesce(max(position) + 1, 0) FROM user_fields))",
        )?
        .execute(params![field.name, kind, definition])?;
        c.prepare_cached(
            "INSERT INTO import_run_fields (run_id, name, kind, definition)
             VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![run, field.name, kind, definition])?;
    }
    Ok(())
}

/// Takes back the labels `run` gave and removes the fields it defined, leaving a
/// field the user changed since or that one of the user's fields uses.
pub(super) fn undo(c: &Connection, run: i64) -> rusqlite::Result<()> {
    let links: Vec<(i64, i64, Option<i64>, Option<String>)> = c
        .prepare(
            "SELECT label_id, target, identity_id, item_key FROM import_run_labels
             WHERE run_id = ?1",
        )?
        .query_map([run], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut touched = Vec::new();
    for (label, target, identity, key) in links {
        let key = match (target, identity, key) {
            (TARGET_TRACK, Some(id), _) => LabelKey::Identity(id),
            (TARGET_ALBUM, _, Some(key)) => LabelKey::Album(key),
            (TARGET_ARTIST, _, Some(key)) => LabelKey::Artist(key),
            _ => continue,
        };
        let (table, column, _) = key.table();
        c.prepare_cached(&format!(
            "DELETE FROM {table} WHERE label_id = ?1 AND {column} = ?2"
        ))?
        .execute(params![label, key.value()])?;
        if !touched.contains(&label) {
            touched.push(label);
        }
    }
    for label in touched {
        drop_if_unused(c, label)?;
    }

    // Fields that still have the definition the run gave them.
    let mut remove: Vec<String> = c
        .prepare(
            "SELECT f.name FROM import_run_fields r JOIN user_fields f
                 ON f.name = r.name AND f.kind = r.kind AND f.definition = r.definition
             WHERE r.run_id = ?1",
        )?
        .query_map([run], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if remove.is_empty() {
        return Ok(());
    }
    let current = crate::user_fields::stored(c)?;
    let failing: HashSet<String> = UserFields::build_lenient(&current)
        .1
        .into_iter()
        .map(|e| e.name)
        .collect();
    // Keep the fields that a field left in place uses.
    loop {
        let remaining: Vec<UserField> = current
            .iter()
            .filter(|f| !remove.contains(&f.name))
            .cloned()
            .collect();
        let errors: Vec<_> = UserFields::build_lenient(&remaining)
            .1
            .into_iter()
            .filter(|e| !failing.contains(&e.name))
            .collect();
        if errors.is_empty() {
            break;
        }
        let before = remove.len();
        for error in errors {
            if let DefinitionProblem::Expression(Problem::UnknownField(name)) = &error.problem {
                remove.retain(|r| !r.eq_ignore_ascii_case(name));
            }
        }
        if remove.len() == before {
            remove.clear();
        }
        if remove.is_empty() {
            break;
        }
    }
    for name in remove {
        c.prepare_cached("DELETE FROM user_fields WHERE name = ?1")?
            .execute([name])?;
    }
    Ok(())
}
