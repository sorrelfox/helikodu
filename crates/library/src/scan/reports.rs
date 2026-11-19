// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading files that are not in the library with the worker pool, for tools
//! that need their tags before the files come into the library (the managed
//! folder names files by them). Nothing is written to the database.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Duration;

use helikodu_meta::worker::{FileReport, Request, RequestOptions};

use super::options::ScanOptions;
use super::pool::{Job, Outcome, Pool, WorkerCommand};
use super::program::find_worker_program;
use super::progress::{CancelToken, ScanError};

/// How often a wait for the workers looks at the cancel flag.
const POLL: Duration = Duration::from_millis(100);

/// Reads the tags and audio properties of `paths` with the worker pool of
/// `options`, without covers, chapters or CUE sheets. Each result is in the
/// place of its path; a file that cannot be read gives its reason. Files not
/// reached because `cancel` was set give an empty reason.
pub(crate) fn read_reports(
    options: &ScanOptions,
    paths: &[PathBuf],
    cancel: &CancelToken,
) -> Result<Vec<Result<Box<FileReport>, String>>, ScanError> {
    let mut out: Vec<Result<Box<FileReport>, String>> =
        (0..paths.len()).map(|_| Err(String::new())).collect();
    if paths.is_empty() {
        return Ok(out);
    }
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
    let mut pool = Pool::start(
        command,
        options.workers.workers_for(paths.len()),
        cancel.clone(),
        stop.clone(),
    );
    let request_options = RequestOptions {
        repair_charsets: options.repair_charsets,
        legacy_charset: options.legacy_charset.clone(),
        cue_sheets: Vec::new(),
        embedded_cue: false,
        chapters: false,
        covers: false,
    };
    let feeder = pool.jobs().map(|jobs| {
        let (cancel, stop) = (cancel.clone(), stop.clone());
        let paths = paths.to_vec();
        thread::Builder::new()
            .name("helikodu-read-feeder".to_owned())
            .spawn(move || {
                for (index, path) in paths.into_iter().enumerate() {
                    let job = Job {
                        index,
                        request: Request {
                            id: index as u64,
                            path,
                            options: request_options.clone(),
                        },
                    };
                    if cancel.is_cancelled() || stop.is_cancelled() || jobs.send(job).is_err() {
                        break;
                    }
                }
            })
    });
    pool.close();
    let mut fatal = None;
    loop {
        match pool.results().recv_timeout(POLL) {
            Ok(Outcome::Read { index, result }) => {
                if let Some(slot) = out.get_mut(index) {
                    *slot = result.map_err(|failure| failure.message);
                }
            }
            Ok(Outcome::Fatal(error)) => {
                stop.cancel();
                fatal.get_or_insert(error);
            }
            Err(RecvTimeoutError::Timeout) => {
                if cancel.is_cancelled() {
                    stop.cancel();
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if let Some(Ok(feeder)) = feeder {
        let _ = feeder.join();
    }
    pool.join();
    match fatal {
        Some(error) => Err(error),
        None => Ok(out),
    }
}
