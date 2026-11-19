// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Placing one file in the managed folder, atomically and without ever
//! replacing anything.
//!
//! - A copy is written to a hidden temporary name beside its destination
//!   (created new, never opened over another file), flushed to the disk, given the
//!   original's permissions and times, renamed into place with
//!   `RENAME_NOREPLACE`, and the folder is flushed too. Then the copy is compared
//!   with what it should hold, byte for byte; a copy that differs is removed.
//! - The identity (device and inode) of the temporary file is handed to the caller
//!   before anything is written into it, so that the journal holds it before the
//!   file can appear at its destination: recovery after a crash then knows which
//!   files the run made. A rename keeps the identity.
//! - A move on one file system is a rename without replacing; across file systems
//!   it is a copy as above, and the caller removes the original afterwards.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rustix::fs::{CWD, RenameFlags, renameat_with};
use rustix::io::Errno;

use crate::db::DbError;
use crate::organize::fsops::{identical, make_parents, part_path, remove_empty};

/// What the destination gets.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Content<'a> {
    /// The bytes of this file.
    File(&'a Path),
    /// These bytes (a rewritten CUE sheet).
    Bytes(&'a [u8]),
}

/// Why a file was not placed.
#[derive(Debug)]
pub(crate) enum PlaceFailure {
    /// The source is not there (or is not a regular file).
    SourceGone,
    /// Something is at the destination already.
    Taken,
    /// The rename would cross file systems (only from [`rename`]).
    CrossDevice,
    /// The copy did not hold the same bytes as its source.
    Differs,
    /// Reading, writing or renaming failed.
    Io(io::Error),
    /// The journal could not be written.
    Db(DbError),
}

impl From<io::Error> for PlaceFailure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for PlaceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceGone => write!(f, "the file is not where it was"),
            Self::Taken => write!(f, "a file with that name is already there"),
            Self::CrossDevice => write!(f, "the destination is on another file system"),
            Self::Differs => write!(f, "the copy differs from the original"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Db(e) => write!(f, "{e}"),
        }
    }
}

/// The identity of a file: its device and inode.
pub(crate) fn identity(path: &Path) -> Option<(u64, u64)> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.file_type().is_file())
        .map(|m| (m.dev(), m.ino()))
}

/// Whether the file at `path` holds `content`.
pub(crate) fn holds(path: &Path, content: Content<'_>) -> io::Result<bool> {
    match content {
        Content::File(source) => identical(source, path),
        Content::Bytes(bytes) => {
            let m = fs::symlink_metadata(path)?;
            if m.len() != bytes.len() as u64 {
                return Ok(false);
            }
            Ok(fs::read(path)? == bytes)
        }
    }
}

/// Renames `from` to `to` on one file system without replacing anything,
/// creating the folders `to` needs.
pub(crate) fn rename(from: &Path, to: &Path) -> Result<(), PlaceFailure> {
    let created = make_parents(to)?;
    let result = match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) | Err(Errno::NOTEMPTY) => Err(PlaceFailure::Taken),
        Err(Errno::XDEV) => Err(PlaceFailure::CrossDevice),
        Err(Errno::NOENT) => Err(PlaceFailure::SourceGone),
        Err(Errno::INVAL) | Err(Errno::NOSYS) | Err(Errno::OPNOTSUPP) => {
            if fs::symlink_metadata(to).is_ok() {
                Err(PlaceFailure::Taken)
            } else {
                match fs::rename(from, to) {
                    Ok(()) => Ok(()),
                    Err(e) if e.raw_os_error() == Some(Errno::XDEV.raw_os_error()) => {
                        Err(PlaceFailure::CrossDevice)
                    }
                    Err(e) => Err(PlaceFailure::Io(e)),
                }
            }
        }
        Err(e) => Err(PlaceFailure::Io(e.into())),
    };
    if result.is_ok() {
        sync_folder(to);
    } else {
        remove_empty(&created);
    }
    result
}

/// Writes `content` to the new file `to` through its temporary name. `made` is
/// called with the identity of the temporary file once it exists and before
/// anything is written into it. The copy is checked before this returns.
pub(crate) fn place(
    content: Content<'_>,
    to: &Path,
    made: &mut dyn FnMut(u64, u64) -> Result<(), DbError>,
) -> Result<(), PlaceFailure> {
    let source = match content {
        Content::File(from) => match fs::symlink_metadata(from) {
            Ok(m) if m.file_type().is_file() => Some(m),
            Ok(_) => return Err(PlaceFailure::SourceGone),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(PlaceFailure::SourceGone),
            Err(e) => return Err(PlaceFailure::Io(e)),
        },
        Content::Bytes(_) => None,
    };
    if fs::symlink_metadata(to).is_ok() {
        return Err(PlaceFailure::Taken);
    }
    let created = make_parents(to)?;
    let part = part_path(to);
    let result = write_part(content, source.as_ref(), &part, made)
        .and_then(|()| into_place(&part, to))
        .and_then(|()| {
            sync_folder(to);
            match holds(to, content) {
                Ok(true) => Ok(()),
                Ok(false) => {
                    let _ = fs::remove_file(to);
                    Err(PlaceFailure::Differs)
                }
                Err(e) => {
                    let _ = fs::remove_file(to);
                    Err(PlaceFailure::Io(e))
                }
            }
        });
    if result.is_err() {
        let _ = fs::remove_file(&part);
        remove_empty(&created);
    }
    result
}

fn write_part(
    content: Content<'_>,
    source: Option<&fs::Metadata>,
    part: &Path,
    made: &mut dyn FnMut(u64, u64) -> Result<(), DbError>,
) -> Result<(), PlaceFailure> {
    // A temporary file left by an earlier attempt is the run's own.
    match fs::remove_file(part) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(PlaceFailure::Io(e)),
    }
    let mut output = OpenOptions::new().write(true).create_new(true).open(part)?;
    let m = output.metadata()?;
    made(m.dev(), m.ino()).map_err(PlaceFailure::Db)?;
    match content {
        Content::File(from) => {
            let mut input = File::open(from)?;
            io::copy(&mut input, &mut output)?;
        }
        Content::Bytes(bytes) => output.write_all(bytes)?,
    }
    output.flush()?;
    if let Some(source) = source {
        output.set_permissions(source.permissions())?;
        let times = fs::FileTimes::new()
            .set_accessed(source.accessed()?)
            .set_modified(source.modified()?);
        output.set_times(times)?;
    }
    output.sync_all()?;
    Ok(())
}

fn into_place(part: &Path, to: &Path) -> Result<(), PlaceFailure> {
    match renameat_with(CWD, part, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(PlaceFailure::Taken),
        Err(Errno::INVAL) | Err(Errno::NOSYS) | Err(Errno::OPNOTSUPP) => {
            if fs::symlink_metadata(to).is_ok() {
                Err(PlaceFailure::Taken)
            } else {
                fs::rename(part, to).map_err(PlaceFailure::Io)
            }
        }
        Err(e) => Err(PlaceFailure::Io(e.into())),
    }
}

/// Flushes the folder of `path` to the disk, so that a rename in it survives a
/// power cut. A folder that cannot be flushed is not an error.
fn sync_folder(path: &Path) {
    if let Some(dir) = path.parent()
        && let Ok(dir) = File::open(dir)
    {
        let _ = dir.sync_all();
    }
}

/// `path` with ` (n)` before its extension.
pub(crate) fn numbered(path: &Path, n: u32) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default();
    let mut name = stem.to_os_string();
    name.push(format!(" ({n})"));
    if let Some(ext) = path.extension() {
        name.push(".");
        name.push(ext);
    }
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("helikodu-managed-transfer-{}", std::process::id()))
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_copy_is_placed_checked_and_never_replaces() {
        let dir = folder("copy");
        let from = dir.join("a.flac");
        fs::write(&from, vec![3_u8; 70_000]).unwrap();
        let to = dir.join("x/y/a.flac");
        let mut seen = None;
        place(Content::File(&from), &to, &mut |dev, ino| {
            seen = Some((dev, ino));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, identity(&to));
        assert!(from.exists());
        assert!(!part_path(&to).exists());
        assert!(matches!(
            place(Content::Bytes(b"other"), &to, &mut |_, _| Ok(())),
            Err(PlaceFailure::Taken)
        ));
        assert_eq!(fs::read(&to).unwrap().len(), 70_000);
    }

    #[test]
    fn a_rename_never_replaces() {
        let dir = folder("rename");
        fs::write(dir.join("a"), b"a").unwrap();
        fs::write(dir.join("b"), b"b").unwrap();
        assert!(matches!(
            rename(&dir.join("a"), &dir.join("b")),
            Err(PlaceFailure::Taken)
        ));
        rename(&dir.join("a"), &dir.join("c/a")).unwrap();
        assert_eq!(fs::read(dir.join("c/a")).unwrap(), b"a");
    }

    #[test]
    fn numbers_go_before_the_extension() {
        assert_eq!(
            numbered(Path::new("/m/01 A.flac"), 2),
            PathBuf::from("/m/01 A (2).flac")
        );
        assert_eq!(numbered(Path::new("/m/x"), 3), PathBuf::from("/m/x (3)"));
    }
}
