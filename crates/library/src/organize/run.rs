// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs of the organizer, their undo and redo, and the journal behind them.
//!
//! A run, an undo or a redo sets its run's state to 0 while it works. Each move is
//! in the journal before its file moves; the transaction that records the move
//! as made also moves the library row. A run found in state 0 was cut short:
//! [`recover`] settles each of its moves from what is on the disk.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, params};

use super::fsops::{self, Method, MoveFailure};
use super::plan::{root_of, roots};
use super::{
    FileRunId, FileRunSummary, MoveOutcome, MoveProblem, MoveRequest, OrganizeError, Progress,
    RunKind, RunReport,
};
use crate::catalog::RootId;
use crate::catalog::write::{file_at, move_file, remove_file};
use crate::copies::FileId;
use crate::db::unix_time::to_unix_seconds;
use crate::db::{Database, DbError, path_to_blob};
use crate::scan::CancelToken;

/// Journal states of a move.
const PLANNED: i64 = 0;
const MOVED: i64 = 1;
const FAILED: i64 = 2;
const UNDONE: i64 = 3;

/// Journal states of a run.
const WORKING: i64 = 0;
const DONE: i64 = 1;
const ALL_UNDONE: i64 = 2;

/// The extensions of the covers and booklets that move with their folder.
const COMPANIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "pdf"];

/// One move of the journal.
#[derive(Debug, Clone)]
struct Row {
    position: i64,
    file: Option<i64>,
    from: PathBuf,
    to: PathBuf,
    state: i64,
}

fn kind_number(kind: RunKind) -> i64 {
    match kind {
        RunKind::Organize => 0,
        RunKind::Rename => 1,
    }
}

fn rows(c: &Connection, run: i64) -> rusqlite::Result<Vec<Row>> {
    c.prepare_cached(
        "SELECT position, file_id, from_path, to_path, state FROM file_moves
         WHERE run_id = ?1 ORDER BY position",
    )?
    .query_map([run], |row| {
        Ok(Row {
            position: row.get(0)?,
            file: row.get(1)?,
            from: fsops::path_of(row.get(2)?),
            to: fsops::path_of(row.get(3)?),
            state: row.get(4)?,
        })
    })?
    .collect()
}

fn set_run_state(c: &Connection, run: i64, state: i64) -> rusqlite::Result<()> {
    c.prepare_cached("UPDATE file_runs SET state = ?2 WHERE id = ?1")?
        .execute(params![run, state])?;
    Ok(())
}

fn set_move_state(
    c: &Connection,
    run: i64,
    position: i64,
    state: i64,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "UPDATE file_moves SET state = ?3, error = ?4 WHERE run_id = ?1 AND position = ?2",
    )?
    .execute(params![run, position, state, error])?;
    Ok(())
}

/// The modification time of `m` in nanoseconds, as the library stores it.
fn mtime_ns(m: &fs::Metadata) -> i64 {
    m.mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(m.mtime_nsec())
}

/// Moves the library row of `file` from `from` to `to` (where the file now is),
/// and records the new stamp. A row the watcher made at `to` for the same file in
/// the meantime gives way. Without a library folder at `to`, the row stays and
/// the next scan finds the file missing.
fn follow_in_library(c: &Connection, file: i64, from: &Path, to: &Path) -> rusqlite::Result<()> {
    let roots = roots(c)?;
    let Some((root, _)) = root_of(&roots, to) else {
        return Ok(());
    };
    let here = file_at(c, from)?;
    let there = file_at(c, to)?;
    if here == Some(file) {
        if there.is_some_and(|t| t != file) {
            remove_file(c, to)?;
        }
        move_file(c, RootId(root), from, to)?;
    } else if there != Some(file) {
        // The library no longer has the file at either place: nothing to follow.
        return Ok(());
    }
    if let Ok(m) = fs::symlink_metadata(to) {
        c.prepare_cached(
            "UPDATE files SET mtime_ns = ?2, inode = ?3, dev = ?4, missing_since = NULL
             WHERE id = ?1",
        )?
        .execute(params![
            file,
            mtime_ns(&m),
            m.ino().cast_signed(),
            m.dev().cast_signed()
        ])?;
    }
    Ok(())
}

/// Records a move made on the disk: its state, and the library row.
fn record(
    db: &Database,
    run: i64,
    row: &Row,
    state: i64,
    from: &Path,
    to: &Path,
) -> Result<(), DbError> {
    let (file, from, to, position) = (row.file, from.to_path_buf(), to.to_path_buf(), row.position);
    db.writer()?.write(move |c| {
        set_move_state(c, run, position, state, None)?;
        if let Some(file) = file {
            follow_in_library(c, file, &from, &to)?;
        }
        Ok(())
    })
}

fn record_failure(
    db: &Database,
    run: i64,
    position: i64,
    state: i64,
    problem: &MoveProblem,
) -> Result<(), DbError> {
    let text = problem.to_string();
    db.writer()?
        .write(move |c| set_move_state(c, run, position, state, Some(&text)))
}

fn problem_of(failure: MoveFailure) -> MoveProblem {
    match failure {
        MoveFailure::SourceGone => MoveProblem::Gone,
        MoveFailure::Taken => MoveProblem::NameTaken,
        MoveFailure::Io(e) => MoveProblem::Failed(e.to_string()),
    }
}

/// The covers and booklets of the folders whose library files all move to one
/// other folder: (from, to) pairs.
fn companions(
    db: &Database,
    moves: &[MoveRequest],
    claimed: &HashSet<PathBuf>,
) -> Result<Vec<(PathBuf, PathBuf)>, DbError> {
    let mut by_folder: HashMap<PathBuf, (HashSet<i64>, BTreeSet<PathBuf>)> = HashMap::new();
    for request in moves {
        let (Some(from_dir), Some(to_dir)) = (request.from.parent(), request.to.parent()) else {
            continue;
        };
        let entry = by_folder.entry(from_dir.to_path_buf()).or_default();
        entry.0.insert(request.file.0);
        entry.1.insert(to_dir.to_path_buf());
    }
    let mut out = Vec::new();
    let mut taken = claimed.clone();
    let mut folders: Vec<_> = by_folder.into_iter().collect();
    folders.sort_by(|a, b| a.0.cmp(&b.0));
    for (folder, (files, targets)) in folders {
        if targets.len() != 1 {
            continue;
        }
        let Some(target) = targets.into_iter().next() else {
            continue;
        };
        if target == folder {
            continue;
        }
        let blob = path_to_blob(&folder).to_vec();
        let (present, names): (Vec<i64>, HashSet<Vec<u8>>) = db.read(|c| {
            let mut statement = c.prepare_cached(
                "SELECT f.id, f.name, f.missing_since IS NULL FROM files f
                 JOIN dirs d ON d.id = f.dir_id WHERE d.path = ?1",
            )?;
            let mut present = Vec::new();
            let mut names = HashSet::new();
            for row in statement.query_map([&blob], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })? {
                let (id, name, here) = row?;
                if here {
                    present.push(id);
                }
                names.insert(name);
            }
            Ok((present, names))
        })?;
        // Only when every file of the folder goes.
        if !present.iter().all(|id| files.contains(id)) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        let mut found: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_file() {
                continue;
            }
            let name = entry.file_name();
            if names.contains(path_to_blob(Path::new(&name)).as_ref()) {
                continue;
            }
            let extension = Path::new(&name)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase());
            if extension.is_some_and(|e| COMPANIONS.contains(&e.as_str())) {
                found.push(entry.path());
            }
        }
        found.sort();
        for from in found {
            let Some(name) = from.file_name() else {
                continue;
            };
            let to = target.join(name);
            if fs::symlink_metadata(&to).is_ok() || !taken.insert(to.clone()) {
                continue;
            }
            out.push((from, to));
        }
    }
    Ok(out)
}

/// Removes the folders that moves out of `folders` left empty, up to their library
/// folders, and their rows when no file is left in them.
fn tidy_sources(db: &Database, folders: &BTreeSet<PathBuf>) -> Result<(), DbError> {
    let roots = db.read(roots)?;
    let mut removed = Vec::new();
    for folder in folders.iter().rev() {
        let Some((_, root)) = root_of(&roots, folder) else {
            continue;
        };
        removed.extend(fsops::remove_empty_upwards(folder, &root));
    }
    forget_folders(db, removed)
}

fn forget_folders(db: &Database, folders: Vec<PathBuf>) -> Result<(), DbError> {
    if folders.is_empty() {
        return Ok(());
    }
    db.writer()?.write(move |c| {
        let mut statement = c.prepare_cached(
            "DELETE FROM dirs WHERE path = ?1
             AND NOT EXISTS (SELECT 1 FROM files f WHERE f.dir_id = dirs.id)",
        )?;
        for folder in &folders {
            statement.execute([path_to_blob(folder)])?;
        }
        Ok(())
    })
}

fn remember_folders(db: &Database, run: i64, folders: Vec<PathBuf>) -> Result<(), DbError> {
    if folders.is_empty() {
        return Ok(());
    }
    db.writer()?.write(move |c| {
        let mut statement =
            c.prepare_cached("INSERT OR IGNORE INTO file_run_dirs (run_id, path) VALUES (?1, ?2)")?;
        for folder in &folders {
            statement.execute(params![run, path_to_blob(folder)])?;
        }
        Ok(())
    })
}

/// Runs `moves` as a new run of the journal.
#[allow(clippy::too_many_arguments)]
pub(super) fn organize(
    db: &Database,
    moves: &[MoveRequest],
    kind: RunKind,
    pattern: Option<&str>,
    with_companions: bool,
    method: Method,
    progress: Progress<'_>,
    cancel: &CancelToken,
) -> Result<RunReport, OrganizeError> {
    let roots = db.read(roots)?;
    let mut outcomes = Vec::new();
    let mut accepted: Vec<&MoveRequest> = Vec::new();
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    for request in moves {
        let problem = if root_of(&roots, &request.to).is_none() {
            Some(MoveProblem::OutsideLibrary)
        } else if request.from == request.to {
            None
        } else if !claimed.insert(request.to.clone()) {
            Some(MoveProblem::NameTaken)
        } else {
            None
        };
        if request.from == request.to {
            continue;
        }
        match problem {
            Some(problem) => outcomes.push(MoveOutcome {
                file: Some(request.file),
                from: request.from.clone(),
                to: request.to.clone(),
                problem: Some(problem),
            }),
            None => accepted.push(request),
        }
    }
    let accepted_owned: Vec<MoveRequest> = accepted.iter().map(|r| (*r).clone()).collect();
    let extra = if with_companions {
        companions(db, &accepted_owned, &claimed)?
    } else {
        Vec::new()
    };
    let mut planned: Vec<Row> = accepted_owned
        .iter()
        .map(|r| (Some(r.file.0), r.from.clone(), r.to.clone()))
        .chain(extra.into_iter().map(|(from, to)| (None, from, to)))
        .enumerate()
        .map(|(i, (file, from, to))| Row {
            position: i64::try_from(i).unwrap_or(i64::MAX),
            file,
            from,
            to,
            state: PLANNED,
        })
        .collect();
    if planned.is_empty() {
        return Ok(RunReport {
            run: None,
            outcomes,
        });
    }
    // The journal first.
    let now = to_unix_seconds(SystemTime::now());
    let pattern = pattern.map(str::to_owned);
    let journal = planned.clone();
    let run = db.writer()?.write(move |c| {
        let run: i64 = c
            .prepare_cached(&format!(
                "INSERT INTO file_runs (started_at, kind, pattern, state, seq)
                 VALUES (?1, ?2, ?3, 0, {}) RETURNING id",
                crate::history::next_seq(4)
            ))?
            .query_row(
                params![now, kind_number(kind), pattern, crate::history::now_ms()],
                |row| row.get(0),
            )?;
        // As a new tag edit does: what waited to be done again cannot be now.
        c.execute("DELETE FROM tag_journal WHERE undone = 1", [])?;
        let mut insert = c.prepare_cached(
            "INSERT INTO file_moves (run_id, position, file_id, from_path, to_path, state)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        )?;
        for row in &journal {
            insert.execute(params![
                run,
                row.position,
                row.file,
                path_to_blob(&row.from),
                path_to_blob(&row.to)
            ])?;
        }
        Ok(run)
    })?;

    let total = planned.len();
    let mut sources = BTreeSet::new();
    let mut created_all = Vec::new();
    let mut any_moved = false;
    for (index, row) in planned.iter_mut().enumerate() {
        let problem = if cancel.is_cancelled() {
            Some(MoveProblem::Stopped)
        } else {
            match check_library(db, row.file, &row.from)? {
                false => Some(MoveProblem::Changed),
                true => match fsops::move_file(&row.from, &row.to, method) {
                    Ok(created) => {
                        record(db, run, row, MOVED, &row.from, &row.to)?;
                        created_all.extend(created);
                        if let Some(parent) = row.from.parent() {
                            sources.insert(parent.to_path_buf());
                        }
                        row.state = MOVED;
                        any_moved = true;
                        None
                    }
                    Err(failure) => Some(problem_of(failure)),
                },
            }
        };
        if let Some(problem) = &problem {
            record_failure(db, run, row.position, FAILED, problem)?;
            row.state = FAILED;
        }
        outcomes.push(MoveOutcome {
            file: row.file.map(FileId),
            from: row.from.clone(),
            to: row.to.clone(),
            problem,
        });
        progress(index + 1, total);
    }
    remember_folders(db, run, created_all)?;
    tidy_sources(db, &sources)?;
    let run_id = if any_moved {
        db.writer()?.write(move |c| set_run_state(c, run, DONE))?;
        Some(FileRunId(run))
    } else {
        db.writer()?.write(move |c| {
            c.prepare_cached("DELETE FROM file_runs WHERE id = ?1")?
                .execute([run])
                .map(drop)
        })?;
        None
    };
    Ok(RunReport {
        run: run_id,
        outcomes,
    })
}

/// Whether the library still has `file` at `at` (always true for a cover or a
/// booklet, which the library does not hold).
fn check_library(db: &Database, file: Option<i64>, at: &Path) -> Result<bool, DbError> {
    let Some(file) = file else {
        return Ok(true);
    };
    let at = at.to_path_buf();
    let found = db.read(move |c| file_at(c, &at))?;
    Ok(found == Some(file))
}

fn newest_run(c: &Connection) -> rusqlite::Result<Option<i64>> {
    c.prepare_cached("SELECT max(id) FROM file_runs")?
        .query_row([], |row| row.get(0))
}

/// Undoes (`forward` false) or redoes the newest run `run`.
pub(super) fn step(
    db: &Database,
    run: FileRunId,
    forward: bool,
    method: Method,
    progress: Progress<'_>,
) -> Result<RunReport, OrganizeError> {
    let id = run.0;
    let (newest, all) = db.read(|c| Ok((newest_run(c)?, rows(c, id)?)))?;
    if newest != Some(id) {
        return Err(OrganizeError::NothingToDo);
    }
    let wanted = if forward { UNDONE } else { MOVED };
    let mut todo: Vec<Row> = all.into_iter().filter(|r| r.state == wanted).collect();
    if todo.is_empty() {
        return Err(OrganizeError::NothingToDo);
    }
    if !forward {
        todo.reverse();
    }
    db.writer()?.write(move |c| set_run_state(c, id, WORKING))?;
    let total = todo.len();
    let mut outcomes = Vec::with_capacity(total);
    let mut sources = BTreeSet::new();
    let mut created_all = Vec::new();
    for (index, row) in todo.iter().enumerate() {
        let (from, to) = if forward {
            (&row.from, &row.to)
        } else {
            (&row.to, &row.from)
        };
        let problem = if check_library(db, row.file, from)? {
            match fsops::move_file(from, to, method) {
                Ok(created) => {
                    record(db, id, row, if forward { MOVED } else { UNDONE }, from, to)?;
                    if forward {
                        created_all.extend(created);
                    }
                    if let Some(parent) = from.parent() {
                        sources.insert(parent.to_path_buf());
                    }
                    None
                }
                Err(failure) => Some(problem_of(failure)),
            }
        } else {
            Some(MoveProblem::Changed)
        };
        if let Some(problem) = &problem {
            // The move stays as it was; its error tells why it did not go.
            record_failure(db, id, row.position, wanted, problem)?;
        }
        outcomes.push(MoveOutcome {
            file: row.file.map(FileId),
            from: from.clone(),
            to: to.clone(),
            problem,
        });
        progress(index + 1, total);
    }
    if forward {
        remember_folders(db, id, created_all)?;
        tidy_sources(db, &sources)?;
    } else {
        // The folders the run made go, when nothing else is in them.
        let made: Vec<PathBuf> = db.read(|c| {
            c.prepare_cached("SELECT path FROM file_run_dirs WHERE run_id = ?1")?
                .query_map([id], |row| Ok(fsops::path_of(row.get(0)?)))?
                .collect()
        })?;
        let removed = fsops::remove_empty(&made);
        forget_folders(db, removed)?;
        // When it was undone, for the shared undo history.
        let now = crate::history::now_ms();
        db.writer()?.write(move |c| {
            c.execute(
                &format!(
                    "UPDATE file_runs SET undone_seq = {} WHERE id = ?1",
                    crate::history::next_seq(2)
                ),
                params![id, now],
            )
        })?;
    }
    settle_run_state(db, id)?;
    Ok(RunReport {
        run: Some(run),
        outcomes,
    })
}

/// Sets the state of `run` from the states of its moves: done while a file is
/// moved, undone when every moved file is back, and gone when nothing moved.
fn settle_run_state(db: &Database, run: i64) -> Result<(), DbError> {
    db.writer()?.write(move |c| {
        let (moved, undone): (i64, i64) = c
            .prepare_cached(
                "SELECT count(*) FILTER (WHERE state = 1), count(*) FILTER (WHERE state = 3)
                 FROM file_moves WHERE run_id = ?1",
            )?
            .query_row([run], |row| Ok((row.get(0)?, row.get(1)?)))?;
        if moved > 0 {
            set_run_state(c, run, DONE)
        } else if undone > 0 {
            set_run_state(c, run, ALL_UNDONE)
        } else {
            c.prepare_cached("DELETE FROM file_runs WHERE id = ?1")?
                .execute([run])
                .map(drop)
        }
    })
}

/// Settles the runs left at work by a crash. Returns how many moves changed state.
pub(super) fn recover(db: &Database) -> Result<usize, DbError> {
    let runs: Vec<i64> = db.read(|c| {
        c.prepare_cached("SELECT id FROM file_runs WHERE state = 0")?
            .query_map([], |row| row.get(0))?
            .collect()
    })?;
    let mut settled = 0;
    for run in runs {
        let all = db.read(|c| rows(c, run))?;
        for row in all {
            let at_from = fs::symlink_metadata(&row.from).is_ok_and(|m| m.is_file());
            let at_to = fs::symlink_metadata(&row.to).is_ok_and(|m| m.is_file());
            match row.state {
                PLANNED => {
                    settled += 1;
                    match (at_from, at_to) {
                        (true, false) => {
                            let _ = fs::remove_file(fsops::part_path(&row.to));
                            record_failure(db, run, row.position, FAILED, &MoveProblem::Stopped)?;
                        }
                        (false, true) => record(db, run, &row, MOVED, &row.from, &row.to)?,
                        (true, true) => {
                            // A copy across file systems that was placed but whose
                            // original was not removed yet: finished only when the
                            // two are the same bytes.
                            if fsops::identical(&row.from, &row.to).unwrap_or(false)
                                && fs::remove_file(&row.from).is_ok()
                            {
                                record(db, run, &row, MOVED, &row.from, &row.to)?;
                            } else {
                                record_failure(
                                    db,
                                    run,
                                    row.position,
                                    FAILED,
                                    &MoveProblem::NameTaken,
                                )?;
                            }
                        }
                        (false, false) => {
                            record_failure(db, run, row.position, FAILED, &MoveProblem::Gone)?;
                        }
                    }
                }
                MOVED if at_from && !at_to => {
                    settled += 1;
                    record(db, run, &row, UNDONE, &row.to, &row.from)?;
                }
                UNDONE if at_to && !at_from => {
                    settled += 1;
                    record(db, run, &row, MOVED, &row.from, &row.to)?;
                }
                _ => {}
            }
        }
        settle_run_state(db, run)?;
    }
    Ok(settled)
}

/// The newest run and its counts.
pub(super) fn last_run(db: &Database) -> Result<Option<FileRunSummary>, DbError> {
    db.read(|c| {
        let Some(run) = newest_run(c)? else {
            return Ok(None);
        };
        c.prepare_cached(
            "SELECT r.kind, r.started_at, r.pattern,
                    (SELECT count(*) FROM file_moves m WHERE m.run_id = r.id AND m.state = 1),
                    (SELECT count(*) FROM file_moves m WHERE m.run_id = r.id AND m.state = 3),
                    (SELECT count(*) FROM file_moves m WHERE m.run_id = r.id AND m.state = 2)
             FROM file_runs r WHERE r.id = ?1",
        )?
        .query_row([run], |row| {
            let count = |i: usize| -> rusqlite::Result<usize> {
                Ok(usize::try_from(row.get::<_, i64>(i)?).unwrap_or(0))
            };
            Ok(FileRunSummary {
                run: FileRunId(run),
                kind: if row.get::<_, i64>(0)? == 1 {
                    RunKind::Rename
                } else {
                    RunKind::Organize
                },
                started_at: row.get(1)?,
                pattern: row.get(2)?,
                moved: count(3)?,
                undone: count(4)?,
                failed: count(5)?,
            })
        })
        .optional()
    })
}
