// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a scan reports while it runs and when it ends, and how it is stopped.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::db::DbError;

/// Something that happened during a scan, sent on the event channel.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScanEvent {
    /// The walk of a library folder begins.
    Walking {
        /// The folder.
        root: PathBuf,
    },
    /// A library folder could not be read: it is not mounted, gone or not
    /// accessible. Nothing in it is marked missing.
    RootUnavailable {
        /// The folder.
        root: PathBuf,
        /// Why.
        error: String,
    },
    /// Every folder has been walked and compared with the library.
    Compared {
        /// The files found.
        found: usize,
        /// The files that are new or changed and will be read.
        to_read: usize,
        /// The files that disappeared.
        missing: usize,
    },
    /// Files have been read. Sent at most every 100 ms, and after the last file.
    Progress {
        /// The files read so far, including those that failed.
        done: usize,
        /// The files to read in all.
        total: usize,
    },
    /// A file could not be read. It is recorded as unreadable and not read again
    /// until it changes.
    FileFailed {
        /// The file.
        path: PathBuf,
        /// Why.
        failure: FileFailure,
    },
    /// The scan is over; the same summary is the scan's result.
    Finished(ScanSummary),
}

/// Why a file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFailure {
    /// The kind of failure.
    pub kind: FailureKind,
    /// A description for the user and the log.
    pub message: String,
}

impl fmt::Display for FileFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The kinds of [`FileFailure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FailureKind {
    /// The file could not be opened or read.
    Io,
    /// No reader understood the file.
    Unreadable,
    /// A reader failed unexpectedly, and the worker survived it.
    ReaderFailed,
    /// The worker died while reading the file (a crash, or the memory limit).
    WorkerCrashed,
    /// The worker took longer than the time limit and was killed.
    TimedOut,
    /// The tracks could not be written to the library.
    WriteFailed,
}

/// What a scan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// The library folders walked.
    pub roots: usize,
    /// Library folders that could not be read.
    pub unavailable_roots: usize,
    /// Media files and CUE sheets found.
    pub found: usize,
    /// Files unchanged since the last scan, not read again.
    pub unchanged: usize,
    /// Files read by the workers.
    pub read: usize,
    /// New tracks.
    pub added: usize,
    /// New tracks that got back the identity of a track that had left the library.
    pub returned: usize,
    /// Tracks written again because their file changed.
    pub updated: usize,
    /// Tracks whose file had disappeared and turned up under another path.
    pub moved: usize,
    /// Files newly marked missing.
    pub missing: usize,
    /// Files that had been missing and are back in place.
    pub back: usize,
    /// Files that could not be read in this scan.
    pub failed: usize,
    /// Unchanged files that could not be read before and were not tried again.
    pub skipped_unreadable: usize,
    /// Worker processes started again after a crash or a timeout.
    pub worker_restarts: usize,
    /// Whether the scan was cancelled before it finished.
    pub cancelled: bool,
    /// How long the scan took.
    pub elapsed: Duration,
}

/// Why a scan could not run. A file that cannot be read does not stop a scan; it is
/// a [`ScanEvent::FileFailed`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScanError {
    /// The library database failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// The worker program was not found.
    #[error("the scan worker program was not found (looked for {0:?})")]
    WorkerNotFound(Vec<PathBuf>),
    /// The worker program could not be started, or did not greet.
    #[error("the scan worker {program:?} could not be started: {reason}")]
    WorkerStart {
        /// The program.
        program: PathBuf,
        /// Why.
        reason: String,
    },
    /// The worker speaks another version of the protocol.
    #[error("the scan worker speaks protocol version {found}, this program version {expected}")]
    WorkerVersion {
        /// The worker's version.
        found: u16,
        /// This program's version.
        expected: u16,
    },
}

/// Stops a running scan. Clones share the flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that has not been cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the scan to stop. Files already read are kept; the workers are killed.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether [`cancel`](Self::cancel) was called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
