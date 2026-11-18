// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Moving a file to the desktop trash, by the freedesktop.org Trash specification
//! (version 1.0).
//!
//! - A file on the volume of the home trash (`$XDG_DATA_HOME/Trash`, by default
//!   `~/.local/share/Trash`) goes there, its `.trashinfo` holding its absolute
//!   path.
//! - A file on another volume goes to that volume's trash: `$topdir/.Trash/$uid`
//!   when `$topdir/.Trash` is a real folder (not a symbolic link) with the sticky
//!   bit, or else `$topdir/.Trash-$uid`, created with mode 0700 if needed. Its
//!   `.trashinfo` holds the path relative to `$topdir`.
//! - The file is renamed into `files/`, never copied, so it cannot be lost half
//!   way; when no trash on its volume can be used, it stays where it is and the
//!   error says why. Nothing here deletes a file.
//! - The `.trashinfo` is created first, exclusively, which reserves the name; a name
//!   already taken gets a number (`song.2.flac`). The deletion date is local time.

use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::localtime;
use crate::db::unix_time::to_unix_seconds;

/// Where files go to the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trash {
    home_trash: PathBuf,
    uid: u32,
    /// Folders taken to be the top folders of volumes of their own (for tests).
    volumes: Vec<PathBuf>,
}

/// A file moved to the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trashed {
    /// The trash folder used (with its `files` and `info` folders).
    pub trash: PathBuf,
    /// Where the file is now.
    pub path: PathBuf,
    /// Its `.trashinfo` file.
    pub info: PathBuf,
}

/// Why a file was not moved to the trash. It is still where it was.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TrashError {
    /// The path is not a regular file (a folder, a symbolic link, or gone).
    #[error("{} is not a file", .0.display())]
    NotAFile(PathBuf),
    /// No trash folder on the file's volume can be used.
    #[error("there is no usable trash on the volume of {}: {reason}", .path.display())]
    NoTrash {
        /// The file.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// Reading or moving failed (for example, no permission).
    #[error("cannot move {} to the trash: {source}", .path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The error.
        #[source]
        source: io::Error,
    },
}

impl Trash {
    /// The trash of the user running the program: the home trash in
    /// `$XDG_DATA_HOME` (or `$HOME/.local/share`). `None` without either.
    pub fn of_user() -> Option<Self> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|home| home.join(".local/share"))
            })?;
        Some(Self::with_home_trash(data.join("Trash")))
    }

    /// The trash with the home trash at `home_trash`, for the user running the
    /// program.
    pub fn with_home_trash(home_trash: impl Into<PathBuf>) -> Self {
        Self {
            home_trash: home_trash.into(),
            uid: rustix::process::getuid().as_raw(),
            volumes: Vec::new(),
        }
    }

    /// Takes `topdir` to be the top folder of a volume of its own, as if it were a
    /// mount point: files below it go to its trash. For tests, which cannot mount.
    #[doc(hidden)]
    #[must_use]
    pub fn with_volume(mut self, topdir: impl Into<PathBuf>) -> Self {
        self.volumes.push(topdir.into());
        self
    }

    /// The home trash folder.
    pub fn home_trash(&self) -> &Path {
        &self.home_trash
    }

    /// Moves the file at `path` to the trash.
    pub fn trash(&self, path: &Path) -> Result<Trashed, TrashError> {
        let io_error = |source| TrashError::Io {
            path: path.to_path_buf(),
            source,
        };
        let path = std::path::absolute(path).map_err(io_error)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_file() => m,
            Ok(_) => return Err(TrashError::NotAFile(path)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(TrashError::NotAFile(path));
            }
            Err(e) => return Err(io_error(e)),
        };
        let parent = path
            .parent()
            .ok_or_else(|| TrashError::NotAFile(path.clone()))?;
        let real_parent = fs::canonicalize(parent).map_err(io_error)?;
        let name = path
            .file_name()
            .ok_or_else(|| TrashError::NotAFile(path.clone()))?;
        let real_path = real_parent.join(name);

        let (trash, info_path) = self.trash_for(&real_path, metadata.dev())?;
        let (stored, info) = reserve(&trash, name, &info_path).map_err(io_error)?;
        if let Err(e) = fs::rename(&real_path, &stored) {
            let _ = fs::remove_file(&info);
            return Err(io_error(e));
        }
        Ok(Trashed {
            trash,
            path: stored,
            info,
        })
    }

    /// The trash folder for the file at `path` (with its links resolved) on the
    /// device `dev`, and the path its `.trashinfo` records.
    fn trash_for(&self, path: &Path, dev: u64) -> Result<(PathBuf, OsString), TrashError> {
        let no_trash = |reason: String| TrashError::NoTrash {
            path: path.to_path_buf(),
            reason,
        };
        let volume = self.volumes.iter().find(|v| path.starts_with(v));
        if volume.is_none() && device_of_nearest(&self.home_trash) == Some(dev) {
            ensure_private_dir(&self.home_trash)
                .map_err(|e| no_trash(format!("the home trash: {e}")))?;
            return Ok((self.home_trash.clone(), encode(path.as_os_str())));
        }
        let topdir = match volume {
            Some(v) => v.clone(),
            None => top_dir(path, dev).ok_or_else(|| no_trash("no top folder".into()))?,
        };
        let relative = path
            .strip_prefix(&topdir)
            .map_err(|_| no_trash("the file is outside its volume".into()))?;
        let recorded = encode(relative.as_os_str());
        // $topdir/.Trash/$uid, if $topdir/.Trash is a real folder with the sticky bit.
        let shared = topdir.join(".Trash");
        if let Ok(m) = fs::symlink_metadata(&shared)
            && m.file_type().is_dir()
            && m.permissions().mode() & 0o1000 != 0
        {
            let own = shared.join(self.uid.to_string());
            if ensure_private_dir(&own).is_ok() {
                return Ok((own, recorded));
            }
        }
        let own = topdir.join(format!(".Trash-{}", self.uid));
        ensure_private_dir(&own).map_err(|e| no_trash(format!("{}: {e}", own.display())))?;
        Ok((own, recorded))
    }
}

/// The device of `path`, or of its nearest ancestor that exists.
fn device_of_nearest(path: &Path) -> Option<u64> {
    path.ancestors()
        .find_map(|p| fs::metadata(p).ok())
        .map(|m| m.dev())
}

/// The top folder of the volume of `path`, on the device `dev`: the highest
/// ancestor on the same device.
fn top_dir(path: &Path, dev: u64) -> Option<PathBuf> {
    let mut top = None;
    for ancestor in path.ancestors().skip(1) {
        match fs::metadata(ancestor) {
            Ok(m) if m.dev() == dev => top = Some(ancestor.to_path_buf()),
            _ => break,
        }
    }
    top
}

/// Makes sure `dir` is a real folder (not a symbolic link), creating it and its
/// `files` and `info` folders with mode 0700 as needed.
fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(dir)?;
    let m = fs::symlink_metadata(dir)?;
    if !m.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the trash is not a folder",
        ));
    }
    for sub in ["files", "info"] {
        let sub = dir.join(sub);
        builder.create(&sub)?;
        if !fs::symlink_metadata(&sub)?.file_type().is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the trash is not a folder",
            ));
        }
    }
    Ok(())
}

/// Reserves a name in `trash` for `name` by creating its `.trashinfo` exclusively,
/// and returns where the file goes and the info file.
fn reserve(trash: &Path, name: &OsStr, recorded: &OsStr) -> io::Result<(PathBuf, PathBuf)> {
    let date = localtime::format_local(to_unix_seconds(SystemTime::now()));
    let mut content = b"[Trash Info]\nPath=".to_vec();
    content.extend_from_slice(recorded.as_bytes());
    content.extend_from_slice(b"\nDeletionDate=");
    content.extend_from_slice(date.as_bytes());
    content.push(b'\n');
    for n in 1_u32..10_000 {
        let candidate = numbered(name, n);
        let mut info_name = candidate.clone();
        info_name.push(".trashinfo");
        let info = trash.join("info").join(&info_name);
        let stored = trash.join("files").join(&candidate);
        if fs::symlink_metadata(&stored).is_ok() {
            continue;
        }
        match OpenOptions::new().write(true).create_new(true).open(&info) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(&content).and_then(|()| file.sync_all()) {
                    let _ = fs::remove_file(&info);
                    return Err(e);
                }
                return Ok((stored, info));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name in the trash",
    ))
}

/// `name`, or with `n` above 1 `stem.n.ext`.
fn numbered(name: &OsStr, n: u32) -> OsString {
    if n == 1 {
        return name.to_os_string();
    }
    let bytes = name.as_bytes();
    let dot = bytes.iter().rposition(|&b| b == b'.').filter(|&i| i > 0);
    let mut out = Vec::with_capacity(bytes.len() + 4);
    match dot {
        Some(i) => {
            out.extend_from_slice(&bytes[..i]);
            out.extend_from_slice(format!(".{n}").as_bytes());
            out.extend_from_slice(&bytes[i..]);
        }
        None => {
            out.extend_from_slice(bytes);
            out.extend_from_slice(format!(".{n}").as_bytes());
        }
    }
    OsString::from_vec(out)
}

/// The path encoded as the path part of a URI: every byte except the unreserved
/// characters and `/` as `%XX`.
fn encode(path: &OsStr) -> OsString {
    let mut out = Vec::with_capacity(path.len());
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b);
        } else {
            out.extend_from_slice(format!("%{b:02X}").as_bytes());
        }
    }
    OsString::from_vec(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_numbered_before_the_extension() {
        assert_eq!(numbered(OsStr::new("a.flac"), 1), "a.flac");
        assert_eq!(numbered(OsStr::new("a.flac"), 2), "a.2.flac");
        assert_eq!(numbered(OsStr::new("noext"), 3), "noext.3");
        assert_eq!(numbered(OsStr::new(".hidden"), 2), ".hidden.2");
    }

    #[test]
    fn paths_are_percent_encoded() {
        assert_eq!(
            encode(OsStr::new("/m/Björk/A b%c.flac")),
            "/m/Bj%C3%B6rk/A%20b%25c.flac"
        );
        assert_eq!(encode(OsStr::from_bytes(b"/x/\xe9")), "/x/%E9");
    }
}
