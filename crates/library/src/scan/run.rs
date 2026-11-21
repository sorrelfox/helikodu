// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! One scan from start to end: walk, compare, read, write.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use std::{fs, io};

use helikodu_meta::worker::{Request, RequestOptions};

use super::compare::{Plan, ToRead};
use super::options::ScanOptions;
use super::pool::{Job, Outcome, Pool, WorkerCommand};
use super::program::find_worker_program;
use super::progress::{CancelToken, FailureKind, FileFailure, ScanError, ScanEvent, ScanSummary};
use super::records::tracks_of;
use super::scope::{ScanTarget, Scope, normalize};
use super::store::{self, STATE_CUE_SHEET, STATE_UNREADABLE};
use super::walk::{Walk, Walker};
use std::os::unix::fs::MetadataExt;

use rusqlite::OptionalExtension;

use super::filter::classify;
use super::lyrics_files;
use super::walk::{Found, Stamp};
use crate::catalog::{self, RootId, TrackChange};
use crate::copies::{precheck, reconcile};
use crate::db::unix_time::to_unix_seconds;
use crate::db::{Database, DbError, WriteTicket, Writer, path_from_blob};
use helikodu_model::settings::DuplicateScan;

/// How often progress is reported.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// The most writes waiting for their batch before the scan waits for the oldest.
const MAX_PENDING_WRITES: usize = 2048;

/// Runs a scan of `options.roots` into `db`.
pub(crate) fn run(
    db: &Database,
    options: &ScanOptions,
    events: Option<&Sender<ScanEvent>>,
    cancel: &CancelToken,
) -> Result<ScanSummary, ScanError> {
    let targets: Vec<ScanTarget> = options
        .roots
        .iter()
        .map(|root| ScanTarget {
            root: root.clone(),
            scopes: vec![Scope::Root],
        })
        .collect();
    run_targets(db, options, &targets, events, cancel)
}

/// Runs a scan of the parts `targets` of library folders into `db`, with the rules
/// of `options`. Files that disappeared from one part and turned up in another are
/// moved, as in a full scan.
pub(crate) fn run_targets(
    db: &Database,
    options: &ScanOptions,
    targets: &[ScanTarget],
    events: Option<&Sender<ScanEvent>>,
    cancel: &CancelToken,
) -> Result<ScanSummary, ScanError> {
    let started = Instant::now();
    let emit = |event: ScanEvent| {
        if let Some(events) = events {
            let _ = events.send(event);
        }
    };
    let writer = db.writer()?;
    let walker = Walker::new(options);
    let mut summary = ScanSummary::default();
    let mut plan = Plan::default();
    // The parts walked, whose lyrics files are checked at the end.
    let mut walked: Vec<(RootId, Scope)> = Vec::new();

    'roots: for target in targets {
        if cancel.is_cancelled() {
            break;
        }
        let root = std::path::absolute(&target.root).unwrap_or_else(|_| target.root.clone());
        emit(ScanEvent::Walking { root: root.clone() });
        summary.roots += 1;
        let root_for_writer = root.clone();
        let root_id = writer.write(move |c| catalog::write::add_root(c, &root_for_writer))?;
        let scopes = normalize(&root, target.scopes.iter().cloned());
        let unavailable = |error: io::Error, summary: &mut ScanSummary| {
            summary.unavailable_roots += 1;
            emit(ScanEvent::RootUnavailable {
                root: root.clone(),
                error: error.to_string(),
            });
        };
        if scopes.first() != Some(&Scope::Root)
            && let Err(e) = readable_folder(&root)
        {
            unavailable(e, &mut summary);
            continue;
        }
        for scope in scopes {
            if cancel.is_cancelled() {
                break 'roots;
            }
            let (walk, known) = match &scope {
                Scope::Root => match walker.walk(&root, &|| cancel.is_cancelled()) {
                    Ok(walk) => (walk, db.read(|c| store::known_files(c, root_id))?),
                    Err(e) => {
                        unavailable(e, &mut summary);
                        continue 'roots;
                    }
                },
                Scope::Subtree(dir) => {
                    let walk = if walker.admits(&root, dir) {
                        match walker.walk(dir, &|| cancel.is_cancelled()) {
                            Ok(walk) => walk,
                            Err(e) => gone_or_unreadable(&root, dir, &e),
                        }
                    } else {
                        Walk::default()
                    };
                    let known = db.read(|c| store::known_files_under(c, root_id, dir, true))?;
                    (walk, known)
                }
                Scope::Folder(dir) => {
                    let walk = if !walker.admits(&root, dir) {
                        Walk::default()
                    } else {
                        match readable_folder(dir) {
                            Ok(()) => walker.walk_folder(dir),
                            Err(e) => gone_or_unreadable(&root, dir, &e),
                        }
                    };
                    let known = db.read(|c| store::known_files_under(c, root_id, dir, false))?;
                    (walk, known)
                }
            };
            if cancel.is_cancelled() {
                break 'roots;
            }
            plan.add_root(root_id, walk, known, options.retry_unreadable);
            walked.push((root_id, scope));
        }
    }
    if cancel.is_cancelled() {
        return Ok(finish(summary, started, cancel, &emit));
    }
    plan.sort();
    summary.found = plan.found;
    summary.unchanged = plan.unchanged;
    summary.skipped_unreadable = plan.skipped_unreadable;
    summary.missing = plan.missing.len();
    summary.back = plan.back.len();
    // Groups of copies with files that went or came back pick their main copy again
    // at the end, after moved files have been found.
    let touched: Vec<i64> = plan.missing.iter().chain(&plan.back).copied().collect();
    write_bookkeeping(writer, &mut plan)?;
    // While copies are skipped, new copies are recognised before they are read.
    let aside = if !plan.to_read.is_empty()
        && db.read(crate::copies::settings::on_scan)? == DuplicateScan::Skip
    {
        precheck::set_aside(db, &mut plan.to_read, hash_threads(), cancel)?
    } else {
        Vec::new()
    };
    emit(ScanEvent::Compared {
        found: plan.found,
        to_read: plan.to_read.len(),
        missing: summary.missing,
    });

    let sheets = std::mem::take(&mut plan.cue_sheets);
    if !plan.to_read.is_empty() {
        read_files(writer, options, plan.to_read, &mut summary, &emit, cancel)?;
    }
    // The sheets are recorded last: if the scan stops before the files of their
    // folders are read, the next scan still sees them as changed.
    if !cancel.is_cancelled() && !sheets.is_empty() {
        writer.write(move |c| {
            for (root, sheet) in &sheets {
                store::write_file_state(c, *root, &sheet.path, sheet.stamp, STATE_CUE_SHEET, None)?;
            }
            Ok(())
        })?;
    }
    if !cancel.is_cancelled() && !aside.is_empty() {
        writer.write(move |c| precheck::record(c, &aside))?;
    }
    let mut read_copies = Vec::new();
    if !cancel.is_cancelled() {
        copies_pass(
            db,
            options,
            &touched,
            false,
            &mut read_copies,
            &mut summary,
            &emit,
            cancel,
        )?;
    }
    if !cancel.is_cancelled() {
        lyrics_files::refresh_scopes(db, &walked)?;
        lyrics_files::refresh_files(db, &read_copies)?;
    }
    Ok(finish(summary, started, cancel, &emit))
}

/// Confirms copies and refreshes the groups of `touched` files (every group with
/// `all`), then reads the files that need tracks (a main copy without any, or the
/// copies when copies get tracks) and refreshes their groups again.
pub(crate) fn confirm_copies(
    db: &Database,
    options: &ScanOptions,
    touched: &[i64],
    all: bool,
    events: Option<&Sender<ScanEvent>>,
    cancel: &CancelToken,
) -> Result<ScanSummary, ScanError> {
    let started = Instant::now();
    let emit = |event: ScanEvent| {
        if let Some(events) = events {
            let _ = events.send(event);
        }
    };
    let mut summary = ScanSummary::default();
    let mut read = Vec::new();
    copies_pass(
        db,
        options,
        touched,
        all,
        &mut read,
        &mut summary,
        &emit,
        cancel,
    )?;
    if !cancel.is_cancelled() {
        lyrics_files::refresh_files(db, &read)?;
    }
    Ok(finish(summary, started, cancel, &emit))
}

/// The files it reads go into `read`.
#[allow(clippy::too_many_arguments)]
fn copies_pass(
    db: &Database,
    options: &ScanOptions,
    touched: &[i64],
    all: bool,
    read: &mut Vec<i64>,
    summary: &mut ScanSummary,
    emit: &dyn Fn(ScanEvent),
    cancel: &CancelToken,
) -> Result<(), ScanError> {
    let threads = hash_threads();
    let mut touched = touched.to_vec();
    let mut all = all;
    // A read can leave work for one more round (tracks to merge); never more.
    for _ in 0..3 {
        let reconciled = reconcile::reconcile(db, &touched, all, threads, cancel)?;
        summary.copies += reconciled.joined;
        if let Some(reason) = reconciled.backup_failed {
            emit(ScanEvent::CopiesNotMerged { reason });
        }
        if reconciled.needs_read.is_empty() || cancel.is_cancelled() {
            break;
        }
        let ids = reconciled.needs_read;
        let to_read = db.read(|c| files_to_read(c, &ids))?;
        if to_read.is_empty() {
            break;
        }
        read_files(db.writer()?, options, to_read, summary, emit, cancel)?;
        read.extend_from_slice(&ids);
        touched = ids;
        all = false;
    }
    Ok(())
}

/// How many threads hash files: one per processor, at most 4.
fn hash_threads() -> usize {
    thread::available_parallelism().map_or(2, |n| n.get().min(4))
}

/// The files `ids` as the workers read them, with their stamps from the disk now.
fn files_to_read(c: &rusqlite::Connection, ids: &[i64]) -> rusqlite::Result<Vec<ToRead>> {
    let mut file = c.prepare_cached(
        "SELECT d.root_id, d.path, f.name, f.dir_id FROM files f JOIN dirs d ON d.id = f.dir_id
         WHERE f.id = ?1",
    )?;
    let mut sheets = c.prepare_cached(
        "SELECT name FROM files WHERE dir_id = ?1 AND scan_state = ?2 ORDER BY name",
    )?;
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        let row: Option<(i64, Vec<u8>, Vec<u8>, i64)> = file
            .query_row([id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()?;
        let Some((root, dir, name, dir_id)) = row else {
            continue;
        };
        let dir = path_from_blob(dir);
        let name = path_from_blob(name);
        let Some(class) = classify(name.as_os_str()) else {
            continue;
        };
        let path = dir.join(&name);
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        let cue_sheets = sheets
            .query_map(rusqlite::params![dir_id, STATE_CUE_SHEET], |row| {
                Ok(dir.join(path_from_blob(row.get(0)?)))
            })?
            .collect::<rusqlite::Result<_>>()?;
        out.push(ToRead {
            root: RootId(root),
            found: Found {
                stamp: Stamp::of(&metadata),
                dev: metadata.dev(),
                path,
                class,
            },
            cue_sheets,
            known: true,
        });
    }
    Ok(out)
}

/// Fails unless `dir` is a folder that can be listed.
fn readable_folder(dir: &Path) -> io::Result<()> {
    if !fs::metadata(dir)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "the library folder is not a folder",
        ));
    }
    fs::read_dir(dir).map(drop)
}

/// The walk of the folder `dir` of `root` that could not be walked because of `error`:
/// empty when it is gone, so what the library had in it is missing, but nothing
/// when it cannot be read, or when `root` itself has gone meanwhile.
fn gone_or_unreadable(root: &Path, dir: &Path, error: &io::Error) -> Walk {
    let gone = matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    );
    if gone && readable_folder(root).is_ok() {
        Walk::default()
    } else {
        Walk {
            files: Vec::new(),
            unreadable: vec![dir.to_path_buf()],
        }
    }
}

fn finish(
    mut summary: ScanSummary,
    started: Instant,
    cancel: &CancelToken,
    emit: &dyn Fn(ScanEvent),
) -> ScanSummary {
    summary.cancelled = cancel.is_cancelled();
    summary.elapsed = started.elapsed();
    emit(ScanEvent::Finished(summary.clone()));
    summary
}

/// Records missing, returning and forgotten files before any track is written: a
/// file that moved is then found among the missing ones.
fn write_bookkeeping(writer: &Writer, plan: &mut Plan) -> Result<(), DbError> {
    let missing = std::mem::take(&mut plan.missing);
    let back = std::mem::take(&mut plan.back);
    let forget = std::mem::take(&mut plan.forget);
    let now = to_unix_seconds(SystemTime::now());
    writer.write(move |c| {
        store::mark_missing(c, &missing, now)?;
        store::mark_present(c, &back)?;
        store::forget_files(c, &forget)
    })
}

/// A write waiting for its batch.
enum Pending {
    Tracks(usize, WriteTicket<Vec<TrackChange>>),
    Failure(WriteTicket<()>),
}

/// Reads `to_read` with the worker pool and writes what comes back.
fn read_files(
    writer: &Writer,
    options: &ScanOptions,
    to_read: Vec<ToRead>,
    summary: &mut ScanSummary,
    emit: &dyn Fn(ScanEvent),
    cancel: &CancelToken,
) -> Result<(), ScanError> {
    let program = find_worker_program(options.workers.program.as_deref())?;
    let mut args: Vec<OsString> = vec!["--memory-limit".into()];
    args.push(options.workers.memory_limit.unwrap_or(0).to_string().into());
    let command = WorkerCommand {
        program,
        args,
        env: options.workers.env.clone(),
        file_timeout: options.workers.file_timeout,
    };
    let stop = CancelToken::new();
    let total = to_read.len();
    let mut pool = Pool::start(
        command,
        options.workers.workers_for(total),
        cancel.clone(),
        stop.clone(),
    );

    let request_options = RequestOptions {
        repair_charsets: options.repair_charsets,
        legacy_charset: options.legacy_charset.clone(),
        covers: options.covers,
        ..RequestOptions::default()
    };
    // The feeder makes each job as it hands it over: made up front, the jobs of a
    // large first scan held a second copy of every path.
    let to_read = Arc::new(to_read);
    let feeder = pool.jobs().map(|jobs| {
        let (cancel, stop) = (cancel.clone(), stop.clone());
        let to_read = Arc::clone(&to_read);
        thread::Builder::new()
            .name("helikodu-scan-feeder".to_owned())
            .spawn(move || {
                for (index, item) in to_read.iter().enumerate() {
                    let job = Job {
                        index,
                        request: Request {
                            id: index as u64,
                            path: item.found.path.clone(),
                            options: RequestOptions {
                                cue_sheets: item.cue_sheets.clone(),
                                ..request_options.clone()
                            },
                        },
                    };
                    if cancel.is_cancelled() || stop.is_cancelled() || jobs.send(job).is_err() {
                        break;
                    }
                }
            })
    });
    pool.close();

    let mut pending: VecDeque<Pending> = VecDeque::new();
    let mut fatal = None;
    let mut done = 0;
    let mut last_progress = Instant::now();
    let now = to_unix_seconds(SystemTime::now());
    loop {
        let outcome = match pool.results().recv_timeout(PROGRESS_INTERVAL) {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => {
                if cancel.is_cancelled() {
                    stop.cancel();
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match outcome {
            Outcome::Fatal(e) => {
                stop.cancel();
                fatal.get_or_insert(e);
            }
            Outcome::Read { index, result } => {
                done += 1;
                summary.read += 1;
                let item = &to_read[index];
                match result {
                    Ok(report) => {
                        let records = tracks_of(item.root, &item.found, &report, options.covers);
                        let dev = Some(item.found.dev);
                        let ticket = writer
                            .submit(move |c| store::write_file_tracks(c, &records, now, dev))?;
                        pending.push_back(Pending::Tracks(index, ticket));
                    }
                    Err(failure) => {
                        summary.failed += 1;
                        let (root, path, stamp) =
                            (item.root, item.found.path.clone(), item.found.stamp);
                        let message = failure.message.clone();
                        emit(ScanEvent::FileFailed {
                            path: item.found.path.clone(),
                            failure,
                        });
                        let ticket = writer.submit(move |c| {
                            store::write_file_state(
                                c,
                                root,
                                &path,
                                stamp,
                                STATE_UNREADABLE,
                                Some(&message),
                            )
                        })?;
                        pending.push_back(Pending::Failure(ticket));
                    }
                }
            }
        }
        settle(&mut pending, false, &to_read, summary, emit);
        if pending.len() > MAX_PENDING_WRITES {
            settle_one(pending.pop_front(), &to_read, summary, emit);
        }
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            last_progress = Instant::now();
            emit(ScanEvent::Progress { done, total });
        }
    }
    if let Some(Ok(feeder)) = feeder {
        let _ = feeder.join();
    }
    summary.worker_restarts = pool.join();
    settle(&mut pending, true, &to_read, summary, emit);
    emit(ScanEvent::Progress { done, total });
    match fatal {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Takes the results of the writes that have committed, in order; waits for all of
/// them when `all` is set.
fn settle(
    pending: &mut VecDeque<Pending>,
    all: bool,
    to_read: &[ToRead],
    summary: &mut ScanSummary,
    emit: &dyn Fn(ScanEvent),
) {
    while let Some(front) = pending.front() {
        let result = match front {
            Pending::Tracks(index, ticket) => ticket.try_wait().map(|r| Settled::Tracks(*index, r)),
            Pending::Failure(ticket) => ticket.try_wait().map(|_| Settled::Failure),
        };
        match result {
            Some(result) => {
                pending.pop_front();
                record(result, to_read, summary, emit);
            }
            None if all => settle_one(pending.pop_front(), to_read, summary, emit),
            None => break,
        }
    }
}

/// A write whose batch has committed or failed.
enum Settled {
    Tracks(usize, Result<Vec<TrackChange>, DbError>),
    Failure,
}

/// Waits for `pending` and takes its result.
fn settle_one(
    pending: Option<Pending>,
    to_read: &[ToRead],
    summary: &mut ScanSummary,
    emit: &dyn Fn(ScanEvent),
) {
    let settled = match pending {
        Some(Pending::Tracks(index, ticket)) => Settled::Tracks(index, ticket.wait()),
        // A failure that cannot even be recorded is read again by the next scan.
        Some(Pending::Failure(ticket)) => {
            let _ = ticket.wait();
            Settled::Failure
        }
        None => return,
    };
    record(settled, to_read, summary, emit);
}

fn record(
    settled: Settled,
    to_read: &[ToRead],
    summary: &mut ScanSummary,
    emit: &dyn Fn(ScanEvent),
) {
    match settled {
        Settled::Tracks(_, Ok(changes)) => {
            for change in changes {
                match change {
                    TrackChange::Added => summary.added += 1,
                    TrackChange::Returned => summary.returned += 1,
                    TrackChange::Updated => summary.updated += 1,
                    TrackChange::Moved => summary.moved += 1,
                }
            }
        }
        Settled::Tracks(index, Err(e)) => {
            summary.failed += 1;
            emit(ScanEvent::FileFailed {
                path: to_read[index].found.path.clone(),
                failure: FileFailure {
                    kind: FailureKind::WriteFailed,
                    message: e.to_string(),
                },
            });
        }
        Settled::Failure => {}
    }
}
