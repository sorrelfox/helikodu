// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The scanner: walks the library folders, compares them with the database, has
//! changed files read by worker processes and writes what they read.
//!
//! - **Walk.** Paths stay bytes. Exclude rules, hidden files, an ignore marker
//!   (`.nomedia`), symbolic links by [`SymlinkPolicy`] with loops and duplicates
//!   detected, and video files only when asked for ([`ScanOptions`]).
//! - **Compare.** A file is read when it is new, or its size, modification time or
//!   inode changed, or a CUE sheet in its folder changed. Files that disappeared are
//!   marked missing; their tracks stay until the grace period ends (the watcher's
//!   job), and a missing file that turns up elsewhere is moved with its tracks.
//! - **Read.** A pool of `helikodu-scan-worker` processes reads the files, one file
//!   per worker at a time, each with an address-space limit and a time limit per
//!   file. A worker that crashes or hangs is replaced, and the file is recorded as
//!   unreadable and not read again until it changes.
//! - **Write.** Each file's tracks are one write; the database writer commits them in
//!   batches.
//! - **Report.** [`ScanEvent`]s go to a channel while the scan runs; a
//!   [`CancelToken`] stops it.

pub(crate) mod compare;
pub(crate) mod filter;
mod lyrics_files;
mod options;
mod pool;
mod program;
mod progress;
mod records;
mod reports;
mod run;
pub(crate) mod scope;
pub(crate) mod store;
mod walk;

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

pub use options::{ExcludeRule, ScanOptions, SymlinkPolicy, WorkerOptions};
pub use program::{WORKER_PROGRAM, find_worker_program};
pub use progress::{CancelToken, FailureKind, FileFailure, ScanError, ScanEvent, ScanSummary};
pub(crate) use reports::read_reports;
pub(crate) use run::{confirm_copies, run, run_targets};
pub(crate) use scope::{ScanTarget, Scope};

use crate::Library;

/// A scan running on its own thread.
#[derive(Debug)]
pub struct ScanHandle {
    events: Receiver<ScanEvent>,
    cancel: CancelToken,
    thread: JoinHandle<Result<ScanSummary, ScanError>>,
}

impl ScanHandle {
    /// Starts scanning `library` with `options` on a thread named `helikodu-scan`.
    pub fn spawn(library: Arc<Library>, options: ScanOptions) -> std::io::Result<Self> {
        let (sender, events) = mpsc::channel();
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let thread = thread::Builder::new()
            .name("helikodu-scan".to_owned())
            .spawn(move || library.scan(&options, Some(&sender), &token))?;
        Ok(Self {
            events,
            cancel,
            thread,
        })
    }

    /// The events of the scan. The channel ends with the scan, after
    /// [`ScanEvent::Finished`] (or after an error).
    pub fn events(&self) -> &Receiver<ScanEvent> {
        &self.events
    }

    /// Asks the scan to stop; [`join`](Self::join) then returns soon.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether the scan has ended.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Waits for the scan to end and returns its summary.
    pub fn join(self) -> Result<ScanSummary, ScanError> {
        self.thread.join().unwrap_or_else(|panic| {
            std::panic::resume_unwind(panic);
        })
    }
}
