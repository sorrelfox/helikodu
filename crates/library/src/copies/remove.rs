// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Moving copies and duplicates to the desktop trash, safely.
//!
//! Each file is checked right before it moves:
//!
//! - it is in the library, present, and its size, modification time and inode are
//!   still those the library recorded;
//! - it is not the same file as any other library file or any file of its group
//!   (the same device and inode: a hard link, a bind mount, a folder reached
//!   through a symbolic link), checked on the disk as well as in the database;
//! - for copies: it is not the main copy, and its bytes are still identical to the
//!   main copy's, compared in full from the disk.
//!
//! A file that fails a check, or that no trash on its volume takes, stays where it
//! is, and its result says why. Nothing is deleted. After a file has moved, its
//! rows leave the library and its group is brought up to date.

use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::SystemTime;

use super::groups::{self, Work};
use super::hash::mtime_ns;
use super::list::{self, FileFacts};
use super::{FileId, TrashOutcome, TrashRefusal};
use crate::db::unix_time::to_unix_seconds;
use crate::db::{Database, DbError};
use crate::maintenance::Trash;

/// What may be moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Copies of a main copy: byte-identical to it, never the main copy.
    Copies,
    /// Duplicates of other kinds: different files, checked only for being the same
    /// file as another one.
    Duplicates,
}

/// Moves `files` to `trash` one after the other, with the checks of `kind`.
pub(crate) fn trash_files(
    db: &Database,
    files: &[FileId],
    trash: &Trash,
    kind: Kind,
) -> Result<Vec<TrashOutcome>, DbError> {
    let mut outcomes = Vec::with_capacity(files.len());
    for &file in files {
        let facts = db.read(|c| list::file_facts(c, file.0))?;
        let Some(facts) = facts else {
            outcomes.push(TrashOutcome {
                file,
                path: None,
                result: Err(TrashRefusal::NotInLibrary),
            });
            continue;
        };
        let related: Vec<FileFacts> = db.read(|c| {
            let mut out = Vec::new();
            for other in list::related_files(c, file.0)? {
                if let Some(f) = list::file_facts(c, other)? {
                    out.push(f);
                }
            }
            Ok(out)
        })?;
        let main = match kind {
            Kind::Copies => match facts.primary {
                Some(primary) => db.read(|c| list::file_facts(c, primary))?,
                None => None,
            },
            Kind::Duplicates => None,
        };
        let path = facts.path.clone();
        let checked = check(file, &facts, main.as_ref(), &related, kind);
        let result = match checked {
            Err(refusal) => Err(refusal),
            Ok(()) => match trash.trash(&path) {
                Ok(trashed) => {
                    let now = to_unix_seconds(SystemTime::now());
                    let id = file.0;
                    db.writer()?.write(move |c| {
                        let mut work = Work::new(c, now)?;
                        groups::before_removal(c, &[id], &mut work)?;
                        work.regroup_file(c, id)?;
                        c.prepare_cached("DELETE FROM files WHERE id = ?1")?
                            .execute([id])?;
                        work.finish(c).map(drop)
                    })?;
                    Ok(trashed)
                }
                Err(e) => Err(TrashRefusal::Trash(e.to_string())),
            },
        };
        outcomes.push(TrashOutcome {
            file,
            path: Some(path),
            result,
        });
    }
    Ok(outcomes)
}

/// The checks before `file` moves.
fn check(
    file: FileId,
    facts: &FileFacts,
    main: Option<&FileFacts>,
    related: &[FileFacts],
    kind: Kind,
) -> Result<(), TrashRefusal> {
    if facts.missing {
        return Err(TrashRefusal::Missing);
    }
    if kind == Kind::Copies {
        if facts.group.is_none() {
            return Err(TrashRefusal::NotACopy);
        }
        if facts.primary == Some(file.0) {
            return Err(TrashRefusal::MainCopy);
        }
    }
    let on_disk = fs::symlink_metadata(&facts.path).map_err(|_| TrashRefusal::Changed)?;
    if !on_disk.file_type().is_file() || !same_stamp(facts, &on_disk) {
        return Err(TrashRefusal::Changed);
    }
    // The same file as another: recorded, or found so on the disk now.
    let key = (on_disk.dev(), on_disk.ino());
    let real = fs::canonicalize(&facts.path).map_err(|_| TrashRefusal::Changed)?;
    let others = related.iter().chain(main);
    for other in others {
        if other.dev.is_some()
            && other.dev == facts.dev
            && other.inode == facts.inode
            && other.path != facts.path
        {
            return Err(TrashRefusal::SameFile);
        }
        if let Ok(m) = fs::metadata(&other.path)
            && (m.dev(), m.ino()) == key
        {
            return Err(TrashRefusal::SameFile);
        }
        if fs::canonicalize(&other.path).is_ok_and(|p| p == real) {
            return Err(TrashRefusal::SameFile);
        }
    }
    if kind == Kind::Copies {
        let Some(main) = main else {
            return Err(TrashRefusal::MainUnavailable);
        };
        if main.missing {
            return Err(TrashRefusal::MainUnavailable);
        }
        match identical(&facts.path, &main.path) {
            Ok(true) => {}
            Ok(false) => return Err(TrashRefusal::Changed),
            Err(_) => return Err(TrashRefusal::MainUnavailable),
        }
        // Still the file that was compared.
        let again = fs::symlink_metadata(&facts.path).map_err(|_| TrashRefusal::Changed)?;
        if !same_stamp(facts, &again) {
            return Err(TrashRefusal::Changed);
        }
    }
    Ok(())
}

/// Whether the file on the disk still has the stamp the library recorded.
fn same_stamp(facts: &FileFacts, m: &fs::Metadata) -> bool {
    facts.size == i64::try_from(m.len()).ok()
        && facts.mtime_ns == Some(mtime_ns(m))
        && facts.inode == Some(m.ino().cast_signed())
}

/// Whether the files at `a` and `b` hold the same bytes, compared in full.
fn identical(a: &Path, b: &Path) -> io::Result<bool> {
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

/// Reads until `buffer` is full or the file ends.
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
