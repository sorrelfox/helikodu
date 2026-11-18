// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Saving without ever leaving a half-written file: the edit goes into a copy in
//! the same folder, which replaces the original by a rename once it is complete,
//! verified and on disk.

use std::ffi::OsString;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// The end of the name of a temporary copy: `.<name>.<pid>-<n>.helikodu-tmp`.
const TEMPORARY_SUFFIX: &str = ".helikodu-tmp";

/// How much of the original name a temporary name keeps, so that it stays within
/// the 255 bytes a file name may have.
const MAX_NAME_PART: usize = 200;

/// A file's size and modification time, recorded when the editor reads it and
/// compared before a write: a file that changed meanwhile is not overwritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EditStamp {
    /// The size in bytes.
    pub size: u64,
    /// The modification time.
    pub modified: SystemTime,
}

impl EditStamp {
    /// The stamp of the file at `path` now (following symbolic links).
    pub fn of(path: impl AsRef<Path>) -> io::Result<Self> {
        fs::metadata(path).and_then(|m| Self::from_metadata(&m))
    }

    pub(crate) fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        Ok(Self {
            size: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

/// A temporary copy beside the original; removed when dropped unless it replaced
/// the original.
#[derive(Debug)]
pub(crate) struct Temporary {
    path: PathBuf,
    pub(crate) file: File,
    persisted: bool,
}

impl Temporary {
    /// A copy of `original` (whose metadata is `metadata`) in the same folder, with
    /// the same permissions and, where allowed, the same owner.
    pub(crate) fn copy_of(original: &Path, metadata: &Metadata) -> io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = original.parent().unwrap_or(Path::new("."));
        let name = original.file_name().unwrap_or_default().as_bytes();
        let name = &name[..name.len().min(MAX_NAME_PART)];
        let mut temporary = b".".to_vec();
        temporary.extend_from_slice(name);
        temporary.extend_from_slice(
            format!(
                ".{}-{}{TEMPORARY_SUFFIX}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            )
            .as_bytes(),
        );
        let path = dir.join(OsString::from_vec(temporary));
        // Readable and writable by the owner while it is written; the original's
        // permissions are set once the copy is complete.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let temporary = Self {
            path,
            file: file.try_clone()?,
            persisted: false,
        };
        io::copy(&mut File::open(original)?, &mut file)?;
        let mode = metadata.permissions().mode() & 0o7777;
        temporary
            .file
            .set_permissions(fs::Permissions::from_mode(mode))?;
        // Only root may give a file away; for anyone else the copy is theirs, as a
        // file they create always is.
        let _ =
            std::os::unix::fs::fchown(&temporary.file, Some(metadata.uid()), Some(metadata.gid()));
        Ok(temporary)
    }

    /// The path of the copy.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Makes the copy durable and moves it over `original`, then makes the rename
    /// durable.
    pub(crate) fn replace(mut self, original: &Path) -> io::Result<()> {
        self.file.sync_all()?;
        fs::rename(&self.path, original)?;
        self.persisted = true;
        if let Some(dir) = original.parent() {
            File::open(dir)?.sync_all()?;
        }
        Ok(())
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.persisted {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Whether `name` is the name of a temporary copy.
pub fn is_temporary_name(name: &std::ffi::OsStr) -> bool {
    let bytes = name.as_bytes();
    bytes.starts_with(b".") && bytes.ends_with(TEMPORARY_SUFFIX.as_bytes())
}

/// Removes the temporary copies in `dir` that a write interrupted by a crash or a
/// power loss left behind, when they are older than `min_age` (so that a write in
/// progress is not disturbed). Returns how many were removed. The originals are
/// whole either way: a copy only replaces its original once it is complete.
pub fn remove_stale_temporaries(dir: &Path, min_age: Duration) -> io::Result<usize> {
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !is_temporary_name(&entry.file_name()) {
            continue;
        }
        let metadata = entry.metadata()?;
        let old = metadata
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= min_age);
        if metadata.is_file() && old && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}
