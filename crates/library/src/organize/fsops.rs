// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Moving one file without ever replacing another, and tidying the folders a move
//! leaves empty.
//!
//! - On one file system a file is renamed with `RENAME_NOREPLACE`, so a file that
//!   appeared at the destination in the meantime is never replaced. Where the file
//!   system cannot do that, the destination is checked just before the rename.
//! - A name that differs only in capital letters on a file system that ignores them
//!   (the destination is the file itself) is renamed in two steps, through a
//!   temporary name beside it.
//! - Across file systems the file is copied to a temporary name in the destination
//!   folder, flushed to the disk, compared with the original byte for byte, given
//!   the original's permissions and times, renamed into place without replacing
//!   anything, and only then is the original removed. If the original cannot be
//!   removed, the copy is, and the file stays where it was.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rustix::fs::{CWD, RenameFlags, renameat_with};
use rustix::io::Errno;

/// Why a file was not moved.
#[derive(Debug)]
pub(crate) enum MoveFailure {
    /// The file is not there (or is not a regular file).
    SourceGone,
    /// Something is at the destination already.
    Taken,
    /// Reading, writing or renaming failed.
    Io(io::Error),
}

impl From<io::Error> for MoveFailure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// How a move should be made (for tests, which need the copy across file
/// systems on one file system).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    /// Rename where possible, copy across file systems.
    Auto,
    /// Always copy and remove.
    Copy,
}

/// The suffix of the temporary file a copy is written to.
pub(crate) const PART_SUFFIX: &str = ".helikodu-part";

/// The temporary name of a copy to `to`: hidden, beside it.
pub(crate) fn part_path(to: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(to.file_name().unwrap_or_default());
    name.push(PART_SUFFIX);
    to.with_file_name(name)
}

/// Moves the file at `from` to `to`, creating the folders `to` needs; returns the
/// folders it created, outermost first. Never replaces a file at `to`.
pub(crate) fn move_file(
    from: &Path,
    to: &Path,
    method: Method,
) -> Result<Vec<PathBuf>, MoveFailure> {
    let source = match fs::symlink_metadata(from) {
        Ok(m) if m.file_type().is_file() => m,
        Ok(_) => return Err(MoveFailure::SourceGone),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(MoveFailure::SourceGone),
        Err(e) => return Err(MoveFailure::Io(e)),
    };
    let created = make_parents(to)?;
    let result = move_existing(from, to, &source, method);
    if result.is_err() {
        // Leave no folder behind that only this move made.
        remove_empty(&created);
        return result.map(|()| Vec::new());
    }
    Ok(created)
}

fn move_existing(
    from: &Path,
    to: &Path,
    source: &fs::Metadata,
    method: Method,
) -> Result<(), MoveFailure> {
    if let Ok(there) = fs::symlink_metadata(to) {
        // The same file under a name in other capitals, on a file system that
        // ignores them: a rename through a temporary name.
        if there.dev() == source.dev() && there.ino() == source.ino() && from != to {
            return rename_through_temporary(from, to);
        }
        return Err(MoveFailure::Taken);
    }
    if method == Method::Copy {
        return copy_and_remove(from, to, source);
    }
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) | Err(Errno::NOTEMPTY) => Err(MoveFailure::Taken),
        Err(Errno::XDEV) => copy_and_remove(from, to, source),
        Err(Errno::INVAL) | Err(Errno::NOSYS) | Err(Errno::OPNOTSUPP) => {
            // The file system cannot refuse to replace: check just before.
            if fs::symlink_metadata(to).is_ok() {
                return Err(MoveFailure::Taken);
            }
            match fs::rename(from, to) {
                Ok(()) => Ok(()),
                Err(e) if e.raw_os_error() == Some(Errno::XDEV.raw_os_error()) => {
                    copy_and_remove(from, to, source)
                }
                Err(e) => Err(MoveFailure::Io(e)),
            }
        }
        Err(Errno::NOENT) => Err(MoveFailure::SourceGone),
        Err(e) => Err(MoveFailure::Io(e.into())),
    }
}

/// Renames `from` to `to`, which is the same file, through a temporary name.
fn rename_through_temporary(from: &Path, to: &Path) -> Result<(), MoveFailure> {
    let temporary = part_path(from);
    if fs::symlink_metadata(&temporary).is_ok() {
        return Err(MoveFailure::Taken);
    }
    fs::rename(from, &temporary)?;
    if let Err(e) = fs::rename(&temporary, to) {
        let _ = fs::rename(&temporary, from);
        return Err(MoveFailure::Io(e));
    }
    Ok(())
}

/// Copies `from` to `to` (on another file system), then removes `from`.
fn copy_and_remove(from: &Path, to: &Path, source: &fs::Metadata) -> Result<(), MoveFailure> {
    let part = part_path(to);
    let copied = copy_checked(from, &part, source);
    if let Err(e) = copied {
        let _ = fs::remove_file(&part);
        return Err(MoveFailure::Io(e));
    }
    // Into place without replacing anything.
    let placed = match renameat_with(CWD, &part, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(MoveFailure::Taken),
        Err(Errno::INVAL) | Err(Errno::NOSYS) | Err(Errno::OPNOTSUPP) => {
            if fs::symlink_metadata(to).is_ok() {
                Err(MoveFailure::Taken)
            } else {
                fs::rename(&part, to).map_err(MoveFailure::Io)
            }
        }
        Err(e) => Err(MoveFailure::Io(e.into())),
    };
    if let Err(failure) = placed {
        let _ = fs::remove_file(&part);
        return Err(failure);
    }
    // The original goes last; if it cannot, the copy goes instead.
    if let Err(e) = fs::remove_file(from) {
        let _ = fs::remove_file(to);
        return Err(MoveFailure::Io(e));
    }
    Ok(())
}

/// Writes a copy of `from` to `part` (a new file), flushed to the disk, with the
/// original's permissions and times, and checks it byte for byte.
fn copy_checked(from: &Path, part: &Path, source: &fs::Metadata) -> io::Result<()> {
    let mut input = File::open(from)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(part)?;
    io::copy(&mut input, &mut output)?;
    output.flush()?;
    output.set_permissions(source.permissions())?;
    let times = fs::FileTimes::new()
        .set_accessed(source.accessed()?)
        .set_modified(source.modified()?);
    output.set_times(times)?;
    output.sync_all()?;
    drop(output);
    if !identical(from, part)? {
        return Err(io::Error::other("the copy differs from the original"));
    }
    Ok(())
}

/// Whether the files at `a` and `b` hold the same bytes.
pub(crate) fn identical(a: &Path, b: &Path) -> io::Result<bool> {
    let mut a = File::open(a)?;
    let mut b = File::open(b)?;
    if a.metadata()?.len() != b.metadata()?.len() {
        return Ok(false);
    }
    let mut left = vec![0_u8; 1 << 20];
    let mut right = vec![0_u8; 1 << 20];
    loop {
        let n = read_full(&mut a, &mut left)?;
        let m = read_full(&mut b, &mut right)?;
        if n != m || left[..n] != right[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

fn read_full(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Creates the folders above `path` that do not exist; returns them, outermost
/// first.
fn make_parents(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut missing = Vec::new();
    let mut folder = path.parent();
    while let Some(dir) = folder {
        if dir.as_os_str().is_empty() || fs::symlink_metadata(dir).is_ok() {
            break;
        }
        missing.push(dir.to_path_buf());
        folder = dir.parent();
    }
    missing.reverse();
    for dir in &missing {
        match fs::create_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                remove_empty(&missing);
                return Err(e);
            }
        }
    }
    Ok(missing)
}

/// Removes the folders `dirs` that are empty, innermost first.
pub(crate) fn remove_empty(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut sorted: Vec<&PathBuf> = dirs.iter().collect();
    sorted.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    sorted.dedup();
    let mut removed = Vec::new();
    for dir in sorted {
        if fs::remove_dir(dir).is_ok() {
            removed.push(dir.clone());
        }
    }
    removed
}

/// Removes `dir` and the folders above it while they are empty, up to (not
/// including) `stop`. Returns the folders removed. Only folders below `stop` are
/// ever touched.
pub(crate) fn remove_empty_upwards(dir: &Path, stop: &Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let mut current = Some(dir);
    while let Some(folder) = current {
        if folder == stop || !folder.starts_with(stop) {
            break;
        }
        if fs::remove_dir(folder).is_err() {
            break;
        }
        removed.push(folder.to_path_buf());
        current = folder.parent();
    }
    removed
}

/// A path from its bytes.
pub(crate) fn path_of(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("helikodu-fsops-{}", std::process::id()))
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_rename_creates_folders_and_never_replaces() {
        let dir = folder("rename");
        fs::write(dir.join("a.flac"), b"a").unwrap();
        fs::write(dir.join("b.flac"), b"b").unwrap();
        let created =
            move_file(&dir.join("a.flac"), &dir.join("x/y/a.flac"), Method::Auto).unwrap();
        assert_eq!(created, [dir.join("x"), dir.join("x/y")]);
        assert_eq!(fs::read(dir.join("x/y/a.flac")).unwrap(), b"a");
        assert!(matches!(
            move_file(&dir.join("b.flac"), &dir.join("x/y/a.flac"), Method::Auto),
            Err(MoveFailure::Taken)
        ));
        assert_eq!(fs::read(dir.join("b.flac")).unwrap(), b"b");
        assert_eq!(fs::read(dir.join("x/y/a.flac")).unwrap(), b"a");
        assert!(matches!(
            move_file(
                &dir.join("gone.flac"),
                &dir.join("z/gone.flac"),
                Method::Auto
            ),
            Err(MoveFailure::SourceGone)
        ));
        assert!(
            !dir.join("z").exists(),
            "no folder is left for a failed move"
        );
    }

    #[test]
    fn a_copy_keeps_bytes_permissions_and_times_and_removes_the_original() {
        use std::os::unix::fs::PermissionsExt;
        let dir = folder("copy");
        let from = dir.join("a.flac");
        fs::write(&from, vec![7_u8; 3 << 20]).unwrap();
        fs::set_permissions(&from, fs::Permissions::from_mode(0o640)).unwrap();
        let before = fs::metadata(&from).unwrap();
        let to = dir.join("b/a.flac");
        move_file(&from, &to, Method::Copy).unwrap();
        assert!(!from.exists());
        let after = fs::metadata(&to).unwrap();
        assert_eq!(after.len(), 3 << 20);
        assert_eq!(after.permissions().mode() & 0o777, 0o640);
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        assert!(!part_path(&to).exists());
    }

    #[test]
    fn a_copy_to_a_taken_name_leaves_both_files() {
        let dir = folder("copy-taken");
        fs::write(dir.join("a"), b"a").unwrap();
        fs::write(dir.join("b"), b"b").unwrap();
        assert!(matches!(
            move_file(&dir.join("a"), &dir.join("b"), Method::Copy),
            Err(MoveFailure::Taken)
        ));
        assert_eq!(fs::read(dir.join("a")).unwrap(), b"a");
        assert_eq!(fs::read(dir.join("b")).unwrap(), b"b");
        assert!(!part_path(&dir.join("b")).exists());
    }

    #[test]
    fn a_copy_whose_original_cannot_go_removes_the_copy() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let dir = folder("copy-locked");
        let locked = dir.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("a"), b"a").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let result = move_file(&locked.join("a"), &dir.join("out/a"), Method::Copy);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(result, Err(MoveFailure::Io(_))));
        assert!(locked.join("a").exists());
        assert!(!dir.join("out/a").exists());
        assert!(!dir.join("out").exists());
    }

    #[test]
    fn empty_folders_are_removed_up_to_the_stop_only() {
        let dir = folder("tidy");
        fs::create_dir_all(dir.join("a/b/c")).unwrap();
        fs::write(dir.join("a/keep"), b"").unwrap();
        let removed = remove_empty_upwards(&dir.join("a/b/c"), &dir);
        assert_eq!(removed, [dir.join("a/b/c"), dir.join("a/b")]);
        assert!(dir.join("a").exists());
        fs::create_dir_all(dir.join("e")).unwrap();
        assert!(remove_empty_upwards(&dir, &dir).is_empty());
        assert!(remove_empty_upwards(Path::new("/tmp"), &dir).is_empty());
    }
}
