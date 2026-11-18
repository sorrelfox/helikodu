// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a scan looks at and how it reads files.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

/// What to scan and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    /// The library folders. Each is added to the library if it is not there yet.
    pub roots: Vec<PathBuf>,
    /// Folders and names to leave out.
    pub exclude: Vec<ExcludeRule>,
    /// Which symbolic links are followed.
    pub symlinks: SymlinkPolicy,
    /// Whether files and folders whose name starts with a dot are scanned. Off by
    /// default.
    pub include_hidden: bool,
    /// Whether video files are scanned, for their audio. Off by default; their tracks
    /// get the kind [`TrackKind::AudioFromVideo`](crate::TrackKind::AudioFromVideo).
    pub include_video: bool,
    /// A file whose presence leaves its folder, and everything below it, out of the
    /// library. `.nomedia` by default.
    pub ignore_marker: Option<OsString>,
    /// Whether files that could not be read before, and have not changed since, are
    /// read again. Off by default, so a file that crashes a decoder is not retried on
    /// every scan.
    pub retry_unreadable: bool,
    /// Whether text in legacy charsets is repaired. On by default.
    pub repair_charsets: bool,
    /// The legacy charset the user chose for old tags and CUE sheets, as a WHATWG
    /// label such as `windows-1251`. Guessed when unset.
    pub legacy_charset: Option<String>,
    /// Whether covers are looked for: the picture embedded in each file, and the
    /// image file in its folder (`cover.jpg`, `folder.png`, …). On by default.
    pub covers: bool,
    /// The worker processes.
    pub workers: WorkerOptions,
}

impl ScanOptions {
    /// The defaults for the library folders `roots`.
    pub fn new(roots: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        Self {
            roots: roots.into_iter().map(Into::into).collect(),
            exclude: Vec::new(),
            symlinks: SymlinkPolicy::default(),
            include_hidden: false,
            include_video: false,
            ignore_marker: Some(".nomedia".into()),
            retry_unreadable: false,
            repair_charsets: true,
            legacy_charset: None,
            covers: true,
            workers: WorkerOptions::default(),
        }
    }
}

/// Something the scan leaves out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExcludeRule {
    /// A folder and everything below it, or a single file, by its path.
    Path(PathBuf),
    /// Every file or folder whose name matches a pattern: `*` matches any run of
    /// bytes, `?` any one byte, everything else itself. `Podcasts`, `*.part`,
    /// `Sample?`. Case matters.
    Name(String),
}

/// Which symbolic links a scan follows.
///
/// Whatever the policy, a folder is entered once: a link back to a folder that is
/// already being scanned (a loop) or was scanned through another path is skipped,
/// and so is a second path to a file already found (a link or a hard link). Real
/// paths are walked before paths through links, so a file inside the library is
/// recorded under its own path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SymlinkPolicy {
    /// Links are skipped.
    Ignore,
    /// A link is followed when its target is inside one of the library folders.
    #[default]
    WithinRoots,
    /// Every link is followed, wherever it leads.
    Follow,
}

/// The pool of `helikodu-scan-worker` processes that read the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerOptions {
    /// The worker program. When unset, `helikodu-scan-worker` beside the running
    /// program, in `../libexec/helikodu/` from it, or in the folder chosen at
    /// build time (Meson's `helperdir`).
    pub program: Option<PathBuf>,
    /// The number of workers; 0 picks one per processor, at most 8.
    pub count: usize,
    /// How long a worker may take for one file before it is killed and the file is
    /// recorded as unreadable. 30 s by default.
    pub file_timeout: Duration,
    /// The address-space limit of each worker, in bytes; `None` for none. 1 GiB by
    /// default.
    pub memory_limit: Option<u64>,
    /// Extra environment variables for the workers, for diagnostics and tests.
    pub env: Vec<(OsString, OsString)>,
}

impl Default for WorkerOptions {
    fn default() -> Self {
        Self {
            program: None,
            count: 0,
            file_timeout: Duration::from_secs(30),
            memory_limit: Some(1 << 30),
            env: Vec::new(),
        }
    }
}

impl WorkerOptions {
    /// The number of workers to start for `files` files.
    pub(crate) fn workers_for(&self, files: usize) -> usize {
        let wanted = if self.count == 0 {
            std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(2)
                .min(8)
        } else {
            self.count
        };
        wanted.min(files).max(1)
    }
}
