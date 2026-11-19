// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs of the managed folder and their journal: carrying out a plan, going on
//! with a run that was cut short, and rolling one back.
//!
//! Every file of a run is in the journal before the run touches it. A file placed
//! by a copy is recorded by the identity of its temporary file before anything is
//! written into it, so recovery only ever removes or moves files the run made. The
//! states of a row: planned, placed (a moved file's copy is in place and its source
//! is still to go), done, skipped, rolled back. A consolidated file's library row
//! follows it in the transaction that marks it done, and goes back in the one that
//! marks it rolled back.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, params};

use super::plan::rewritten;
use super::transfer::{Content, PlaceFailure, holds, identity, numbered, place, rename};
use super::{
    ItemAction, ItemKind, ItemResult, ManagedError, ManagedOperation, ManagedOutcome, ManagedPlan,
    ManagedReport, ManagedRunId, ManagedRunSummary, SkipReason, Transfer,
};
use crate::catalog::write::{add_root, file_at, remove_file};
use crate::copies::FileId;
use crate::db::unix_time::to_unix_seconds;
use crate::db::{Database, DbError, path_from_blob, path_to_blob};
use crate::organize::Progress;
use crate::organize::fsops::{self, Method, MoveFailure, part_path, remove_empty_upwards};
use crate::organize::plan::{root_of, roots};
use crate::organize::run::follow_in_library;
use crate::scan::{CancelToken, ScanOptions, ScanTarget, Scope, run_targets};

/// Journal states of a file.
const PLANNED: i64 = 0;
const PLACED: i64 = 1;
const DONE: i64 = 2;
const SKIPPED: i64 = 3;
const ROLLED_BACK: i64 = 4;

/// Journal states of a run.
const WORKING: i64 = 0;
const FINISHED: i64 = 1;
const ALL_ROLLED_BACK: i64 = 2;

/// How many numbered names are tried when a destination was taken after the plan.
const MAX_NUMBER: u32 = 9999;

/// One file of the journal.
#[derive(Debug, Clone)]
struct Op {
    position: i64,
    kind: ItemKind,
    file: Option<i64>,
    from: PathBuf,
    to: PathBuf,
    move_file: bool,
    identity: Option<(u64, u64)>,
    original: Option<Vec<u8>>,
    content: Option<Vec<u8>>,
    state: i64,
    kept: bool,
    error: Option<String>,
}

/// What a run knows of itself.
#[derive(Debug, Clone)]
struct RunFacts {
    id: i64,
    operation: ManagedOperation,
    transfer: Transfer,
    folder: PathBuf,
    bases: Vec<PathBuf>,
}

fn operation_number(operation: ManagedOperation) -> i64 {
    match operation {
        ManagedOperation::Import => 0,
        ManagedOperation::Consolidate => 1,
    }
}

fn join_paths(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        if i > 0 {
            out.push(0);
        }
        out.extend_from_slice(path_to_blob(path).as_ref());
    }
    out
}

fn split_paths(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| path_from_blob(p.to_vec()))
        .collect()
}

fn ops(c: &Connection, run: i64) -> rusqlite::Result<Vec<Op>> {
    c.prepare_cached(
        "SELECT position, kind, file_id, from_path, to_path, move_file, dev, ino, original,
                content, state, kept, error
         FROM managed_ops WHERE run_id = ?1 ORDER BY position",
    )?
    .query_map([run], |row| {
        let dev: Option<i64> = row.get(6)?;
        let ino: Option<i64> = row.get(7)?;
        Ok(Op {
            position: row.get(0)?,
            kind: ItemKind::from_number(row.get(1)?),
            file: row.get(2)?,
            from: path_from_blob(row.get(3)?),
            to: path_from_blob(row.get(4)?),
            move_file: row.get(5)?,
            identity: dev
                .zip(ino)
                .map(|(d, i)| (d.cast_unsigned(), i.cast_unsigned())),
            original: row.get(8)?,
            content: row.get(9)?,
            state: row.get(10)?,
            kept: row.get(11)?,
            error: row.get(12)?,
        })
    })?
    .collect()
}

fn run_facts(c: &Connection, run: i64) -> rusqlite::Result<Option<RunFacts>> {
    c.prepare_cached("SELECT operation, move_files, folder, bases FROM managed_runs WHERE id = ?1")?
        .query_row([run], |row| {
            Ok(RunFacts {
                id: run,
                operation: if row.get::<_, i64>(0)? == 1 {
                    ManagedOperation::Consolidate
                } else {
                    ManagedOperation::Import
                },
                transfer: if row.get::<_, bool>(1)? {
                    Transfer::Move
                } else {
                    Transfer::Copy
                },
                folder: path_from_blob(row.get(2)?),
                bases: split_paths(&row.get::<_, Vec<u8>>(3)?),
            })
        })
        .optional()
}

fn interrupted_id(c: &Connection) -> rusqlite::Result<Option<i64>> {
    c.prepare_cached("SELECT max(id) FROM managed_runs WHERE state = 0")?
        .query_row([], |row| row.get(0))
}

/// Sets the state of a row, with its error and whether its source was kept.
fn set_state(
    db: &Database,
    run: i64,
    op: &Op,
    state: i64,
    follow: Option<(i64, PathBuf, PathBuf)>,
    forget: Option<PathBuf>,
) -> Result<(), DbError> {
    let (position, kept, error) = (op.position, op.kept, op.error.clone());
    db.writer()?.write(move |c| {
        c.prepare_cached(
            "UPDATE managed_ops SET state = ?3, kept = ?4, error = ?5
             WHERE run_id = ?1 AND position = ?2",
        )?
        .execute(params![run, position, state, kept, error])?;
        if let Some((file, from, to)) = follow {
            follow_in_library(c, file, &from, &to)?;
        }
        if let Some(path) = forget {
            remove_file(c, &path)?;
        }
        Ok(())
    })
}

fn set_to(db: &Database, run: i64, position: i64, to: &Path) -> Result<(), DbError> {
    let to = path_to_blob(to).to_vec();
    db.writer()?.write(move |c| {
        c.prepare_cached("UPDATE managed_ops SET to_path = ?3 WHERE run_id = ?1 AND position = ?2")?
            .execute(params![run, position, to])
            .map(drop)
    })
}

fn set_identity(db: &Database, run: i64, position: i64, dev: u64, ino: u64) -> Result<(), DbError> {
    db.writer()?.write(move |c| {
        c.prepare_cached(
            "UPDATE managed_ops SET dev = ?3, ino = ?4 WHERE run_id = ?1 AND position = ?2",
        )?
        .execute(params![run, position, dev.cast_signed(), ino.cast_signed()])
        .map(drop)
    })
}

fn set_content(db: &Database, run: i64, position: i64, content: &[u8]) -> Result<(), DbError> {
    let content = content.to_vec();
    db.writer()?.write(move |c| {
        c.prepare_cached("UPDATE managed_ops SET content = ?3 WHERE run_id = ?1 AND position = ?2")?
            .execute(params![run, position, content])
            .map(drop)
    })
}

fn set_run_state(db: &Database, run: i64, state: i64) -> Result<(), DbError> {
    db.writer()?.write(move |c| {
        c.prepare_cached("UPDATE managed_runs SET state = ?2 WHERE id = ?1")?
            .execute(params![run, state])
            .map(drop)
    })
}

/// The skip reason a row's error text stands for.
fn reason_of(error: Option<&str>) -> SkipReason {
    match error {
        Some("identical") => SkipReason::Identical,
        Some("gone") => SkipReason::Gone,
        Some("changed") => SkipReason::Changed,
        Some(other) => SkipReason::Failed(other.to_owned()),
        None => SkipReason::Failed(String::new()),
    }
}

fn reason_text(reason: &SkipReason) -> String {
    match reason {
        SkipReason::Identical => "identical".to_owned(),
        SkipReason::Gone => "gone".to_owned(),
        SkipReason::Changed => "changed".to_owned(),
        other => other.to_string(),
    }
}

/// What a row's state says of its file, for a report.
fn result_of(op: &Op) -> ItemResult {
    match op.state {
        DONE if op.kept => ItemResult::SourceKept(op.error.clone().unwrap_or_default()),
        DONE if op.move_file => ItemResult::Moved,
        DONE => ItemResult::Copied,
        SKIPPED => ItemResult::Skipped(reason_of(op.error.as_deref())),
        ROLLED_BACK => ItemResult::RolledBack,
        _ => ItemResult::Waiting,
    }
}

fn outcome(op: &Op, result: ItemResult) -> ManagedOutcome {
    ManagedOutcome {
        kind: op.kind,
        file: op.file.map(FileId),
        from: op.from.clone(),
        to: op.to.clone(),
        result,
    }
}

/// Whether the file at `path` is the one this row placed.
fn ours(op: &Op, path: &Path) -> bool {
    op.identity.is_some() && identity(path) == op.identity
}

/// The free bytes on the file system of `folder`.
fn free_bytes(folder: &Path) -> Option<u64> {
    let dir = folder.ancestors().find(|p| p.is_dir())?;
    rustix::fs::statvfs(dir)
        .ok()
        .map(|s| s.f_bavail.saturating_mul(s.f_frsize))
}

pub(super) fn apply(
    db: &Database,
    plan: &ManagedPlan,
    scan: &ScanOptions,
    progress: Progress<'_>,
    cancel: &CancelToken,
) -> Result<ManagedReport, ManagedError> {
    if let Some(run) = db.read(interrupted_id)? {
        return Err(ManagedError::Interrupted(ManagedRunId(run)));
    }
    let options = &plan.options;
    if !options.folder.starts_with(&options.library_folder) {
        return Err(ManagedError::OutsideLibrary {
            folder: options.folder.clone(),
            library_folder: options.library_folder.clone(),
        });
    }
    if let Some(free) = free_bytes(&options.folder)
        && plan.bytes_needed > free
    {
        return Err(ManagedError::NoSpace {
            needed: plan.bytes_needed,
            free,
        });
    }
    let journal: Vec<_> = plan
        .items
        .iter()
        .filter(|i| {
            matches!(
                i.action,
                ItemAction::Copy
                    | ItemAction::Move
                    | ItemAction::CopyKeepingSource
                    | ItemAction::Skip(SkipReason::Identical)
            )
        })
        .cloned()
        .collect();
    if !journal.iter().any(super::ManagedItem::transfers) {
        return Ok(ManagedReport {
            finished: true,
            ..ManagedReport::default()
        });
    }
    // The managed folder is in the library before anything moves into it.
    let (folder, library_folder) = (options.folder.clone(), options.library_folder.clone());
    db.writer()?.write(move |c| {
        let known = roots(c)?;
        if root_of(&known, &folder).is_none() && !known.iter().any(|(_, r)| *r == folder) {
            add_root(c, &library_folder)?;
        }
        Ok(())
    })?;
    let now = to_unix_seconds(SystemTime::now());
    let (operation, transfer) = (plan.operation, options.transfer);
    let folder = options.folder.clone();
    let bases = join_paths(&plan.bases);
    let run = db.writer()?.write(move |c| {
        let run: i64 = c
            .prepare_cached(
                "INSERT INTO managed_runs (started_at, operation, move_files, folder, bases)
                 VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
            )?
            .query_row(
                params![
                    now,
                    operation_number(operation),
                    transfer == Transfer::Move,
                    path_to_blob(&folder),
                    bases
                ],
                |row| row.get(0),
            )?;
        let mut insert = c.prepare_cached(
            "INSERT INTO managed_ops (run_id, position, kind, file_id, from_path, to_path,
                                      move_file, size, dev, ino, original, state, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?;
        for (position, item) in journal.iter().enumerate() {
            let move_file = item.action == ItemAction::Move;
            // A rename keeps the source's identity.
            let source = if move_file && item.kind != ItemKind::CueSheet {
                identity(&item.from)
            } else {
                None
            };
            let original = (item.kind == ItemKind::CueSheet)
                .then(|| fs::read(&item.from).ok())
                .flatten();
            let skipped = !item.transfers();
            insert.execute(params![
                run,
                position as i64,
                item.kind.number(),
                item.file.map(|f| f.0),
                path_to_blob(&item.from),
                path_to_blob(&item.to),
                move_file,
                item.size.cast_signed(),
                source.map(|(d, _)| d.cast_signed()),
                source.map(|(_, i)| i.cast_signed()),
                original,
                if skipped { SKIPPED } else { PLANNED },
                skipped.then_some("identical"),
            ])?;
        }
        Ok(run)
    })?;
    let facts = db
        .read(move |c| run_facts(c, run))?
        .ok_or(ManagedError::NotInterrupted)?;
    drive(db, &facts, scan, progress, cancel)
}

/// Works through the rows of `run` that wait, then lets the library learn the new
/// files and tidies the folders moves emptied.
fn drive(
    db: &Database,
    facts: &RunFacts,
    scan: &ScanOptions,
    progress: Progress<'_>,
    cancel: &CancelToken,
) -> Result<ManagedReport, ManagedError> {
    let run = facts.id;
    let mut all = db.read(move |c| ops(c, run))?;
    let total = all
        .iter()
        .filter(|o| o.state == PLANNED || o.state == PLACED)
        .count();
    let mut done = 0;
    let mut stopped = false;
    for index in 0..all.len() {
        if all[index].state != PLANNED && all[index].state != PLACED {
            continue;
        }
        if cancel.is_cancelled() {
            stopped = true;
            break;
        }
        let audio: Vec<(PathBuf, PathBuf)> = if all[index].kind == ItemKind::CueSheet {
            all.iter()
                .filter(|o| o.kind == ItemKind::Audio)
                .map(|o| (o.from.clone(), o.to.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let mut op = all[index].clone();
        work(db, facts, &mut op, &audio)?;
        all[index] = op;
        done += 1;
        progress(done, total);
    }
    let mut report = ManagedReport {
        run: Some(ManagedRunId(run)),
        outcomes: all.iter().map(|o| outcome(o, result_of(o))).collect(),
        finished: !stopped,
        library_error: None,
    };
    report.library_error = learn(db, facts, &all, scan);
    if facts.transfer == Transfer::Move {
        tidy(db, facts, &all)?;
    }
    if !stopped {
        set_run_state(db, run, FINISHED)?;
    }
    Ok(report)
}

/// Settles one row that waits: finishes what a cut-short run left, or does it.
fn work(
    db: &Database,
    facts: &RunFacts,
    op: &mut Op,
    audio: &[(PathBuf, PathBuf)],
) -> Result<(), DbError> {
    let run = facts.id;
    // A temporary file left by a run that was cut short.
    if op.state == PLANNED {
        let _ = fs::remove_file(part_path(&op.to));
    }
    let from_here = fs::symlink_metadata(&op.from).is_ok_and(|m| m.is_file());
    let placed_here = ours(op, &op.to);
    if op.state == PLACED || placed_here {
        if !from_here {
            if placed_here {
                // Renamed on one file system, or copied and its source removed.
                return finish_done(db, facts, op);
            }
            return skip(db, run, op, SkipReason::Gone);
        }
        if placed_here && verified(op) {
            return after_placed(db, facts, op);
        }
        if placed_here {
            // A copy that did not finish its check: it goes, and the file again.
            let _ = fs::remove_file(&op.to);
        }
        op.state = PLANNED;
    }
    if !from_here {
        return skip(db, run, op, SkipReason::Gone);
    }
    if facts.operation == ManagedOperation::Consolidate
        && let Some(file) = op.file
    {
        let at = op.from.clone();
        if db.read(move |c| file_at(c, &at))? != Some(file) {
            return skip(db, run, op, SkipReason::Changed);
        }
    }
    if op.kind == ItemKind::CueSheet
        && let Some(original) = &op.original
    {
        let froms: Vec<PathBuf> = audio
            .iter()
            .filter(|(from, _)| from.parent() == op.from.parent())
            .map(|(from, _)| from.clone())
            .collect();
        let content = rewritten(original, &op.to, &froms, &|from| {
            audio
                .iter()
                .find(|(f, _)| f == from)
                .map(|(_, to)| to.clone())
        });
        set_content(db, run, op.position, &content)?;
        op.content = Some(content);
    }
    let planned = op.to.clone();
    for n in 1..=MAX_NUMBER {
        let candidate = if n == 1 {
            planned.clone()
        } else {
            numbered(&planned, n)
        };
        if candidate != op.to {
            // The journal knows the new name before the file gets it.
            set_to(db, run, op.position, &candidate)?;
            op.to = candidate;
        }
        if fs::symlink_metadata(&op.to).is_ok() {
            if holds(&op.to, content_of(op)).unwrap_or(false) {
                return skip(db, run, op, SkipReason::Identical);
            }
            continue;
        }
        let attempt = if op.move_file && op.content.is_none() && op.kind != ItemKind::CueSheet {
            match rename(&op.from, &op.to) {
                Ok(()) => return finish_done(db, facts, op),
                Err(PlaceFailure::CrossDevice) => copy_into_place(db, run, op),
                Err(other) => Err(other),
            }
        } else {
            copy_into_place(db, run, op)
        };
        match attempt {
            Ok(()) => return after_placed(db, facts, op),
            Err(PlaceFailure::Taken) => {}
            Err(PlaceFailure::SourceGone) => return skip(db, run, op, SkipReason::Gone),
            Err(PlaceFailure::Db(e)) => return Err(e),
            Err(other) => return skip(db, run, op, SkipReason::Failed(other.to_string())),
        }
    }
    skip(
        db,
        run,
        op,
        SkipReason::Failed("there is no free name for the file".to_owned()),
    )
}

fn content_of(op: &Op) -> Content<'_> {
    match &op.content {
        Some(bytes) => Content::Bytes(bytes),
        None => Content::File(&op.from),
    }
}

/// Whether the file at the destination holds what it should.
fn verified(op: &Op) -> bool {
    holds(&op.to, content_of(op)).unwrap_or(false)
}

fn copy_into_place(db: &Database, run: i64, op: &mut Op) -> Result<(), PlaceFailure> {
    let position = op.position;
    let mut made = |dev: u64, ino: u64| set_identity(db, run, position, dev, ino);
    let content = match &op.content {
        Some(bytes) => Content::Bytes(bytes),
        None => Content::File(&op.from),
    };
    place(content, &op.to, &mut made)?;
    op.identity = identity(&op.to);
    Ok(())
}

/// The copy is in place and checked: a copy is done, a moved file's source goes.
fn after_placed(db: &Database, facts: &RunFacts, op: &mut Op) -> Result<(), DbError> {
    if !op.move_file {
        return finish_done(db, facts, op);
    }
    if op.state != PLACED {
        op.state = PLACED;
        set_state(db, facts.id, op, PLACED, None, None)?;
    }
    match fs::remove_file(&op.from) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            op.kept = true;
            op.error = Some(e.to_string());
        }
    }
    finish_done(db, facts, op)
}

/// Marks the row done; a consolidated file's library row follows it.
fn finish_done(db: &Database, facts: &RunFacts, op: &mut Op) -> Result<(), DbError> {
    op.identity = op.identity.or_else(|| identity(&op.to));
    let follow = (facts.operation == ManagedOperation::Consolidate)
        .then_some(op.file)
        .flatten()
        .map(|file| (file, op.from.clone(), op.to.clone()));
    op.state = DONE;
    set_state(db, facts.id, op, DONE, follow, None)
}

fn skip(db: &Database, run: i64, op: &mut Op, reason: SkipReason) -> Result<(), DbError> {
    op.state = SKIPPED;
    op.error = Some(reason_text(&reason));
    set_state(db, run, op, SKIPPED, None, None)
}

/// Has the library read the folders the run put files into (and, for a
/// consolidation that moved files, the folders they left). Returns why it could
/// not, if it could not.
fn learn(db: &Database, facts: &RunFacts, all: &[Op], scan: &ScanOptions) -> Option<String> {
    let known = db.read(roots).ok()?;
    let mut targets: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    let mut add = |dir: &Path| {
        if let Some((_, root)) =
            root_of(&known, dir).or_else(|| known.iter().find(|(_, r)| r.as_path() == dir).cloned())
        {
            targets.entry(root).or_default().insert(dir.to_path_buf());
        }
    };
    for op in all.iter().filter(|o| o.state == DONE) {
        if let Some(dir) = op.to.parent() {
            add(dir);
        }
        if facts.operation == ManagedOperation::Consolidate
            && op.move_file
            && !op.kept
            && let Some(dir) = op.from.parent()
        {
            add(dir);
        }
    }
    if targets.is_empty() {
        return None;
    }
    let targets: Vec<ScanTarget> = targets
        .into_iter()
        .map(|(root, dirs)| ScanTarget {
            root,
            scopes: dirs.into_iter().map(Scope::Folder).collect(),
        })
        .collect();
    run_targets(db, scan, &targets, None, &CancelToken::new())
        .err()
        .map(|e| e.to_string())
}

/// Removes the folders that moves emptied: up to the library folder for a
/// consolidation, up to (and with) the folders given for an import.
fn tidy(db: &Database, facts: &RunFacts, all: &[Op]) -> Result<(), DbError> {
    let known = db.read(roots)?;
    let mut folders = BTreeSet::new();
    for op in all
        .iter()
        .filter(|o| o.state == DONE && o.move_file && !o.kept)
    {
        if let Some(dir) = op.from.parent() {
            folders.insert(dir.to_path_buf());
        }
    }
    for folder in folders.iter().rev() {
        let stop = match facts.operation {
            ManagedOperation::Consolidate => root_of(&known, folder).map(|(_, r)| r),
            // The deepest folder given that holds it; that folder may go too.
            ManagedOperation::Import => facts
                .bases
                .iter()
                .filter(|b| b.is_dir() || !b.exists())
                .filter(|b| folder.starts_with(b))
                .max_by_key(|b| b.as_os_str().len())
                .and_then(|b| b.parent().map(Path::to_path_buf)),
        };
        if let Some(stop) = stop {
            remove_empty_upwards(folder, &stop);
        }
    }
    Ok(())
}

pub(super) fn interrupted(db: &Database) -> Result<Option<ManagedRunSummary>, DbError> {
    db.read(|c| {
        let Some(run) = interrupted_id(c)? else {
            return Ok(None);
        };
        let Some(facts) = run_facts(c, run)? else {
            return Ok(None);
        };
        let (started_at, done, waiting, skipped): (i64, i64, i64, i64) = c
            .prepare_cached(
                "SELECT r.started_at,
                        (SELECT count(*) FROM managed_ops WHERE run_id = r.id AND state = 2),
                        (SELECT count(*) FROM managed_ops WHERE run_id = r.id AND state IN (0, 1)),
                        (SELECT count(*) FROM managed_ops WHERE run_id = r.id AND state = 3)
                 FROM managed_runs r WHERE r.id = ?1",
            )?
            .query_row([run], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
        let count = |n: i64| usize::try_from(n).unwrap_or(0);
        Ok(Some(ManagedRunSummary {
            run: ManagedRunId(run),
            operation: facts.operation,
            transfer: facts.transfer,
            started_at,
            folder: facts.folder,
            done: count(done),
            waiting: count(waiting),
            skipped: count(skipped),
        }))
    })
}

fn interrupted_facts(db: &Database, run: ManagedRunId) -> Result<RunFacts, ManagedError> {
    let id = run.0;
    let (state, facts) = db.read(move |c| {
        let state: Option<i64> = c
            .prepare_cached("SELECT state FROM managed_runs WHERE id = ?1")?
            .query_row([id], |row| row.get(0))
            .optional()?;
        Ok((state, run_facts(c, id)?))
    })?;
    match (state, facts) {
        (Some(WORKING), Some(facts)) => Ok(facts),
        _ => Err(ManagedError::NotInterrupted),
    }
}

pub(super) fn resume(
    db: &Database,
    run: ManagedRunId,
    scan: &ScanOptions,
    progress: Progress<'_>,
    cancel: &CancelToken,
) -> Result<ManagedReport, ManagedError> {
    let facts = interrupted_facts(db, run)?;
    drive(db, &facts, scan, progress, cancel)
}

pub(super) fn roll_back(
    db: &Database,
    run: ManagedRunId,
    progress: Progress<'_>,
) -> Result<ManagedReport, ManagedError> {
    let facts = interrupted_facts(db, run)?;
    let id = facts.id;
    let mut all = db.read(move |c| ops(c, id))?;
    let total = all
        .iter()
        .filter(|o| matches!(o.state, PLANNED | PLACED | DONE))
        .count();
    let mut done = 0;
    let mut problems = 0;
    let mut outcomes = Vec::with_capacity(all.len());
    let mut touched = BTreeSet::new();
    for op in all.iter_mut().rev() {
        if !matches!(op.state, PLANNED | PLACED | DONE) {
            outcomes.push(outcome(op, result_of(op)));
            continue;
        }
        let result = undo(db, &facts, op)?;
        if let Some(dir) = op.to.parent() {
            touched.insert(dir.to_path_buf());
        }
        if matches!(result, ItemResult::NotRolledBack(_)) {
            problems += 1;
        }
        outcomes.push(outcome(op, result));
        done += 1;
        progress(done, total);
    }
    outcomes.reverse();
    for dir in touched.iter().rev() {
        remove_empty_upwards(dir, &facts.folder);
    }
    if problems == 0 {
        set_run_state(db, id, ALL_ROLLED_BACK)?;
    }
    Ok(ManagedReport {
        run: Some(run),
        outcomes,
        finished: problems == 0,
        library_error: None,
    })
}

/// Rolls back one row; returns what became of it.
fn undo(db: &Database, facts: &RunFacts, op: &mut Op) -> Result<ItemResult, DbError> {
    let _ = fs::remove_file(part_path(&op.to));
    let from_here = fs::symlink_metadata(&op.from).is_ok_and(|m| m.is_file());
    let to_here = fs::symlink_metadata(&op.to).is_ok_and(|m| m.is_file());
    // The file at the destination is the run's when its identity says so, or
    // (after a tag edit replaced it) when it still holds the source's bytes.
    let mine = to_here && (ours(op, &op.to) || (op.state != PLANNED && from_here && verified(op)));
    let consolidated = (facts.operation == ManagedOperation::Consolidate)
        .then_some(op.file)
        .flatten();
    // What the library row does as the file goes back.
    let back = |op: &Op| -> (Option<(i64, PathBuf, PathBuf)>, Option<PathBuf>) {
        if op.state != DONE {
            return (None, None);
        }
        match consolidated {
            Some(file) => (Some((file, op.to.clone(), op.from.clone())), None),
            None if facts.operation == ManagedOperation::Import && op.kind == ItemKind::Audio => {
                (None, Some(op.to.clone()))
            }
            None => (None, None),
        }
    };
    if !mine {
        if from_here {
            // Nothing of the run is at the destination.
            let (follow, forget) = back(op);
            let forget = forget.filter(|_| !to_here);
            op.state = ROLLED_BACK;
            set_state(db, facts.id, op, ROLLED_BACK, follow, forget)?;
            return Ok(ItemResult::RolledBack);
        }
        return Ok(ItemResult::NotRolledBack(
            "neither the file nor its copy is where the run left them".to_owned(),
        ));
    }
    if from_here {
        if let Err(e) = fs::remove_file(&op.to) {
            return Ok(ItemResult::NotRolledBack(e.to_string()));
        }
    } else {
        let moved_back = match (&op.original, &op.content) {
            (Some(original), Some(_)) => {
                let result = place(Content::Bytes(original), &op.from, &mut |_, _| Ok(()));
                match result {
                    Ok(()) => fs::remove_file(&op.to).map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                }
            }
            _ => fsops::move_file(&op.to, &op.from, Method::Auto)
                .map(drop)
                .map_err(|e| match e {
                    MoveFailure::SourceGone => "the file is not where the run put it".to_owned(),
                    MoveFailure::Taken => "a file with its old name is there now".to_owned(),
                    MoveFailure::Io(e) => e.to_string(),
                }),
        };
        if let Err(reason) = moved_back {
            return Ok(ItemResult::NotRolledBack(reason));
        }
    }
    let (follow, forget) = back(op);
    op.state = ROLLED_BACK;
    set_state(db, facts.id, op, ROLLED_BACK, follow, forget)?;
    Ok(ItemResult::RolledBack)
}
