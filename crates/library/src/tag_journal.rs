// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The undo journal of tag edits.
//!
//! [`Library::edit_tags`] writes tags like [`Library::write_tags`], and first
//! reads, from each file, the fields the edit changes (the front cover and the
//! lyrics too). After the write it reads them again, and records both as one
//! step. [`Library::undo_tags`] puts back what the newest step changed;
//! [`Library::redo_tags`] does the newest undone step again. A new edit drops the
//! steps that wait to be done again.
//!
//! A file is only changed back when it still holds what the step left in it:
//! a file whose fields were changed since (by another program, or by a later
//! step that was not undone) is left alone and reported as
//! [`StepProblem::Changed`]. The values are compared as stored, without the
//! charset repair, so that repaired text can be put back as it was.
//!
//! The newest [`KEPT_STEPS`] steps are kept, none older than [`KEPT_DAYS`] days.
//! Pictures are stored once, by their hash.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use helikodu_meta::covers::{CoverHash, CoverLimits, read_embedded_covers};
use helikodu_meta::tags::ReadOptions;
use helikodu_meta::tags::writing::{
    BatchItem, BatchProgress, CoverChange, FieldKey, TagChanges, WriteError, WriteMode,
    load_for_editing, write_tags,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::Library;
use crate::db::unix_time::{from_unix_seconds, to_unix_seconds};
use crate::db::{DbError, path_from_blob, path_to_blob};
use crate::scan::{CancelToken, ScanError, ScanOptions, ScanSummary};
use crate::tag_writes::{TagWriteReport, TagWriteSettings};

/// The steps kept in the journal.
pub const KEPT_STEPS: i64 = 100;
/// The age in days after which a step leaves the journal.
pub const KEPT_DAYS: u64 = 90;

/// The journal's name of the front cover.
const COVER: &str = "@cover";
/// The journal's name of the lyrics.
const LYRICS: &str = "@lyrics";
/// What separates the values of a field.
const SEPARATOR: char = '\u{1f}';

/// A step of the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TagStepId(pub i64);

/// A step that can be undone or done again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagStep {
    /// The step.
    pub id: TagStepId,
    /// When the edit was saved.
    pub at: SystemTime,
    /// The files it changed.
    pub files: usize,
}

/// What can be undone and done again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagHistory {
    /// The step [`Library::undo_tags`] would undo.
    pub undo: Option<TagStep>,
    /// The step [`Library::redo_tags`] would do again.
    pub redo: Option<TagStep>,
}

/// The result of [`Library::edit_tags`].
#[derive(Debug)]
pub struct TagEditReport {
    /// The write, as [`Library::write_tags`] reports it.
    pub write: TagWriteReport,
    /// The step recorded; `None` when no file was changed.
    pub step: Option<TagStepId>,
}

/// Why a file of a step was not changed back (or again).
#[derive(Debug, thiserror::Error)]
pub enum StepProblem {
    /// The file no longer holds what the step left in it (or, to do the step
    /// again, what it held before).
    #[error("the file was changed since")]
    Changed,
    /// The file could not be read.
    #[error("the file cannot be read: {0}")]
    Unreadable(String),
    /// A picture of the step is no longer in the journal.
    #[error("the picture is no longer in the undo journal")]
    PictureGone,
    /// The write failed.
    #[error(transparent)]
    Write(#[from] WriteError),
}

/// The result of [`Library::undo_tags`] and [`Library::redo_tags`].
#[derive(Debug)]
pub struct TagStepReport {
    /// The step.
    pub step: TagStepId,
    /// Each file of the step: changed (or already as wanted), or why not.
    pub files: Vec<(PathBuf, Result<(), StepProblem>)>,
    /// The re-read of the files written; `None` when none was.
    pub refresh: Option<ScanSummary>,
}

impl TagStepReport {
    /// The files that were not changed, and why.
    pub fn failed(&self) -> impl Iterator<Item = (&PathBuf, &StepProblem)> {
        self.files
            .iter()
            .filter_map(|(path, r)| r.as_ref().err().map(|e| (path, e)))
    }
}

/// The fields of one file as the journal keeps them, by journal name.
type Snapshot = BTreeMap<String, String>;

/// Picture bytes by their hex hash.
type Pictures = BTreeMap<String, Vec<u8>>;

/// One file of a step as stored.
#[derive(Debug)]
struct StepFile {
    path: PathBuf,
    before: Snapshot,
    after: Snapshot,
}

/// The journal names of what `changes` touches.
fn names_of(changes: &TagChanges) -> Vec<String> {
    let mut names: Vec<String> = changes.fields().map(|(k, _)| k.name().to_owned()).collect();
    if changes.cover.is_some() {
        names.push(COVER.to_owned());
    }
    if changes.lyrics.is_some() {
        names.push(LYRICS.to_owned());
    }
    names
}

fn join(values: &[String]) -> String {
    let mut text = String::new();
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            text.push(SEPARATOR);
        }
        text.push_str(value);
    }
    text
}

fn split(text: &str) -> Vec<String> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split(SEPARATOR).map(str::to_owned).collect()
    }
}

/// Reads the fields `names` of the file at `path` as stored; the front cover's
/// bytes go into `pictures`.
fn snapshot(
    path: &Path,
    names: &[String],
    pictures: &mut BTreeMap<String, Vec<u8>>,
) -> Result<Snapshot, String> {
    let options = ReadOptions {
        repair_charsets: false,
        legacy_charset: None,
    };
    let file = load_for_editing(path, &options).map_err(|e| e.to_string())?;
    let mut shot = Snapshot::new();
    for name in names {
        let value = match name.as_str() {
            COVER => match read_embedded_covers(path, &CoverLimits::default()) {
                Ok(covers) => match covers.front().map(|c| c.bytes()) {
                    Some(Ok(bytes)) => {
                        let hash = CoverHash::of(bytes).to_hex();
                        pictures
                            .entry(hash.clone())
                            .or_insert_with(|| bytes.to_vec());
                        hash
                    }
                    Some(Err(e)) => return Err(e.to_string()),
                    None => String::new(),
                },
                Err(e) => return Err(e.to_string()),
            },
            LYRICS => file.lyrics.clone().unwrap_or_default(),
            field => join(&file.values(&FieldKey::parse(field))),
        };
        shot.insert(name.clone(), value);
    }
    Ok(shot)
}

/// The changes that make a file hold `target`.
fn changes_to(
    target: &Snapshot,
    pictures: &BTreeMap<String, Vec<u8>>,
) -> Result<TagChanges, StepProblem> {
    let mut changes = TagChanges::new();
    for (name, value) in target {
        match name.as_str() {
            COVER => {
                changes.cover = Some(if value.is_empty() {
                    CoverChange::RemoveFront
                } else {
                    CoverChange::SetFront(
                        pictures
                            .get(value)
                            .cloned()
                            .ok_or(StepProblem::PictureGone)?,
                    )
                });
            }
            LYRICS => changes.lyrics = Some((!value.is_empty()).then(|| value.clone())),
            field => {
                let values = split(value);
                if values.is_empty() {
                    changes.clear(FieldKey::parse(field));
                } else {
                    changes.set(FieldKey::parse(field), values);
                }
            }
        }
    }
    Ok(changes)
}

/// Records a step with `files`, dropping the steps that waited to be done
/// again and the steps past the limits.
fn record(
    c: &Connection,
    files: &[StepFile],
    pictures: &BTreeMap<String, Vec<u8>>,
    now: i64,
) -> rusqlite::Result<i64> {
    c.execute("DELETE FROM tag_journal WHERE undone = 1", [])?;
    let step: i64 = c.query_row(
        "INSERT INTO tag_journal (at, undone) VALUES (?1, 0) RETURNING id",
        [now],
        |row| row.get(0),
    )?;
    for file in files {
        let id: i64 = c.query_row(
            "INSERT INTO tag_journal_files (step_id, path) VALUES (?1, ?2) RETURNING id",
            params![step, path_to_blob(&file.path)],
            |row| row.get(0),
        )?;
        let mut insert = c.prepare_cached(
            "INSERT INTO tag_journal_values (file_id, field, before, after)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (name, before) in &file.before {
            let after = file.after.get(name).map_or("", String::as_str);
            insert.execute(params![id, name, before, after])?;
        }
    }
    let mut insert = c.prepare_cached(
        "INSERT OR IGNORE INTO tag_journal_pictures (hash, data) VALUES (?1, ?2)",
    )?;
    for (hash, data) in pictures {
        insert.execute(params![hash, data])?;
    }
    prune(c, now)?;
    Ok(step)
}

/// Drops the steps past [`KEPT_STEPS`] or older than [`KEPT_DAYS`], and the
/// pictures no step needs.
fn prune(c: &Connection, now: i64) -> rusqlite::Result<()> {
    let oldest = now - i64::try_from(KEPT_DAYS * 24 * 3600).unwrap_or(i64::MAX);
    c.execute(
        "DELETE FROM tag_journal WHERE at < ?1 OR id NOT IN
            (SELECT id FROM tag_journal ORDER BY id DESC LIMIT ?2)",
        params![oldest, KEPT_STEPS],
    )?;
    c.execute(
        "DELETE FROM tag_journal_pictures WHERE hash NOT IN
            (SELECT before FROM tag_journal_values WHERE field = ?1
             UNION SELECT after FROM tag_journal_values WHERE field = ?1)",
        [COVER],
    )?;
    Ok(())
}

/// The step that undoing (`redo` false) or doing again would take.
fn next_step(c: &Connection, redo: bool) -> rusqlite::Result<Option<TagStep>> {
    let sql = if redo {
        "SELECT id, at FROM tag_journal WHERE undone = 1 ORDER BY id ASC LIMIT 1"
    } else {
        "SELECT id, at FROM tag_journal WHERE undone = 0 ORDER BY id DESC LIMIT 1"
    };
    let Some((id, at)) = c
        .query_row(sql, [], |row| Ok((row.get::<_, i64>(0)?, row.get(1)?)))
        .optional()?
    else {
        return Ok(None);
    };
    let files: i64 = c.query_row(
        "SELECT count(*) FROM tag_journal_files WHERE step_id = ?1",
        [id],
        |row| row.get(0),
    )?;
    Ok(Some(TagStep {
        id: TagStepId(id),
        at: from_unix_seconds(at),
        files: usize::try_from(files).unwrap_or(0),
    }))
}

/// The files of `step` and the pictures they need.
fn load_step(c: &Connection, step: i64) -> rusqlite::Result<(Vec<StepFile>, Pictures)> {
    let mut files = Vec::new();
    let mut statement =
        c.prepare_cached("SELECT id, path FROM tag_journal_files WHERE step_id = ?1 ORDER BY id")?;
    let rows: Vec<(i64, Vec<u8>)> = statement
        .query_map([step], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut values =
        c.prepare_cached("SELECT field, before, after FROM tag_journal_values WHERE file_id = ?1")?;
    let mut hashes = BTreeSet::new();
    for (id, path) in rows {
        let mut file = StepFile {
            path: path_from_blob(path),
            before: Snapshot::new(),
            after: Snapshot::new(),
        };
        for row in values.query_map([id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })? {
            let (field, before, after) = row?;
            if field == COVER {
                hashes.extend([before.clone(), after.clone()]);
            }
            file.before.insert(field.clone(), before);
            file.after.insert(field, after);
        }
        files.push(file);
    }
    let mut pictures = BTreeMap::new();
    let mut picture = c.prepare_cached("SELECT data FROM tag_journal_pictures WHERE hash = ?1")?;
    for hash in hashes.into_iter().filter(|h| !h.is_empty()) {
        if let Some(data) = picture
            .query_row([&hash], |row| row.get::<_, Vec<u8>>(0))
            .optional()?
        {
            pictures.insert(hash, data);
        }
    }
    Ok((files, pictures))
}

impl Library {
    /// Writes `items` as [`write_tags`](Library::write_tags) does, and records
    /// what changed in the undo journal as one step.
    pub fn edit_tags(
        &self,
        items: &[BatchItem],
        settings: &TagWriteSettings,
        scan: &ScanOptions,
        progress: &mut dyn FnMut(&BatchProgress),
        cancel: &CancelToken,
    ) -> Result<TagEditReport, ScanError> {
        let mut pictures = BTreeMap::new();
        let before: Vec<Option<Snapshot>> = items
            .iter()
            .map(|item| snapshot(&item.path, &names_of(&item.changes), &mut pictures).ok())
            .collect();
        let write = self.write_tags(items, settings, scan, progress, cancel)?;
        let mut files = Vec::new();
        for ((item, before), (_, result)) in items.iter().zip(before).zip(&write.batch.results) {
            let (Some(before), Ok(outcome)) = (before, result) else {
                continue;
            };
            if outcome.mode == WriteMode::Unchanged {
                continue;
            }
            let names = names_of(&item.changes);
            let Ok(after) = snapshot(&outcome.path, &names, &mut pictures) else {
                continue;
            };
            if after != before {
                files.push(StepFile {
                    path: outcome.path.clone(),
                    before,
                    after,
                });
            }
        }
        let step = if files.is_empty() {
            None
        } else {
            let now = to_unix_seconds(SystemTime::now());
            let id = self
                .database()
                .writer()?
                .write(move |c| record(c, &files, &pictures, now))?;
            Some(TagStepId(id))
        };
        Ok(TagEditReport { write, step })
    }

    /// What can be undone and done again.
    pub fn tag_history(&self) -> Result<TagHistory, DbError> {
        self.database().read(|c| {
            Ok(TagHistory {
                undo: next_step(c, false)?,
                redo: next_step(c, true)?,
            })
        })
    }

    /// Undoes the newest tag edit: each of its files gets back what it held
    /// before, unless it was changed since. `None` when there is nothing to undo.
    pub fn undo_tags(
        &self,
        settings: &TagWriteSettings,
        scan: &ScanOptions,
    ) -> Result<Option<TagStepReport>, ScanError> {
        self.step_tags(false, settings, scan)
    }

    /// Does the newest undone tag edit again. `None` when there is nothing to
    /// do again.
    pub fn redo_tags(
        &self,
        settings: &TagWriteSettings,
        scan: &ScanOptions,
    ) -> Result<Option<TagStepReport>, ScanError> {
        self.step_tags(true, settings, scan)
    }

    fn step_tags(
        &self,
        redo: bool,
        settings: &TagWriteSettings,
        scan: &ScanOptions,
    ) -> Result<Option<TagStepReport>, ScanError> {
        let Some(step) = self.database().read(|c| next_step(c, redo))? else {
            return Ok(None);
        };
        let (files, pictures) = self.database().read(|c| load_step(c, step.id.0))?;
        let mut results = Vec::with_capacity(files.len());
        let mut written = Vec::new();
        for file in files {
            let (expected, target) = if redo {
                (&file.before, &file.after)
            } else {
                (&file.after, &file.before)
            };
            let names: Vec<String> = target.keys().cloned().collect();
            let mut seen = BTreeMap::new();
            let result = match snapshot(&file.path, &names, &mut seen) {
                Err(reason) => Err(StepProblem::Unreadable(reason)),
                Ok(now) if now == *target => Ok(()),
                Ok(now) if now != *expected => Err(StepProblem::Changed),
                Ok(_) => changes_to(target, &pictures).and_then(|changes| {
                    write_tags(&file.path, &changes, None, &settings.write)
                        .map(|outcome| {
                            if outcome.mode != WriteMode::Unchanged {
                                written.push(outcome.path);
                            }
                        })
                        .map_err(StepProblem::from)
                }),
            };
            results.push((file.path, result));
        }
        let id = step.id.0;
        self.database().writer()?.write(move |c| {
            c.execute(
                "UPDATE tag_journal SET undone = ?2 WHERE id = ?1",
                params![id, i64::from(!redo)],
            )
        })?;
        let refresh = if written.is_empty() {
            None
        } else {
            Some(self.refresh_files(&written, scan)?)
        };
        Ok(Some(TagStepReport {
            step: step.id,
            files: results,
            refresh,
        }))
    }

    /// The number of steps in the journal and the age of the oldest.
    pub fn tag_journal_steps(&self) -> Result<(usize, Option<Duration>), DbError> {
        self.database().read(|c| {
            let (count, oldest): (i64, Option<i64>) =
                c.query_row("SELECT count(*), min(at) FROM tag_journal", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?;
            let age =
                oldest.and_then(|at| SystemTime::now().duration_since(from_unix_seconds(at)).ok());
            Ok((usize::try_from(count).unwrap_or(0), age))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_join_and_split() {
        let values = vec!["Folk".to_owned(), "Ambient".to_owned()];
        assert_eq!(split(&join(&values)), values);
        assert_eq!(split(&join(&[])), Vec::<String>::new());
    }

    #[test]
    fn changes_back_set_and_clear() {
        let mut target = Snapshot::new();
        target.insert("title".to_owned(), "Old".to_owned());
        target.insert("genre".to_owned(), String::new());
        target.insert(LYRICS.to_owned(), String::new());
        let changes = changes_to(&target, &BTreeMap::new()).unwrap();
        assert_eq!(changes.fields().count(), 2);
        assert_eq!(changes.lyrics, Some(None));
        let mut cover = Snapshot::new();
        cover.insert(COVER.to_owned(), "ab".to_owned());
        assert!(matches!(
            changes_to(&cover, &BTreeMap::new()),
            Err(StepProblem::PictureGone)
        ));
    }
}
