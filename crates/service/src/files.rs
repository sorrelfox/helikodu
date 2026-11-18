// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the service keeps its files (the XDG base directories), and writing them so
//! that a crash or power loss leaves either the old or the new file, never a part.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

/// The application's folder inside each XDG base directory.
pub const APP_FOLDER: &str = "helikodu";

/// The folders the service uses: each already ends in `helikodu`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// Settings: `$XDG_CONFIG_HOME/helikodu` (`~/.config/helikodu`).
    pub config: PathBuf,
    /// The session, the crash sentinel, the window state and logs:
    /// `$XDG_STATE_HOME/helikodu` (`~/.local/state/helikodu`).
    pub state: PathBuf,
    /// The library database: `$XDG_DATA_HOME/helikodu` (`~/.local/share/helikodu`).
    pub data: PathBuf,
    /// Thumbnails and analysis data: `$XDG_CACHE_HOME/helikodu` (`~/.cache/helikodu`).
    pub cache: PathBuf,
}

/// Neither the XDG variable nor `HOME` names a folder.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("cannot find the {0} folder: neither {0} nor HOME is set to an absolute path")]
pub struct NoHomeFolder(pub &'static str);

impl AppDirs {
    /// The folders from the environment: each XDG variable when it holds an absolute
    /// path, otherwise its default under `HOME`.
    pub fn from_env() -> Result<Self, NoHomeFolder> {
        Self::from_vars(|name| std::env::var_os(name))
    }

    /// The folders under one root, `<root>/config/helikodu` and so on: for tests and
    /// for running from a scratch folder.
    pub fn under(root: &Path) -> Self {
        Self {
            config: root.join("config").join(APP_FOLDER),
            state: root.join("state").join(APP_FOLDER),
            data: root.join("data").join(APP_FOLDER),
            cache: root.join("cache").join(APP_FOLDER),
        }
    }

    fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<Self, NoHomeFolder> {
        let home = var("HOME").map(PathBuf::from).filter(|p| p.is_absolute());
        let dir = |name: &'static str, fallback: &str| {
            var(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| home.as_ref().map(|home| home.join(fallback)))
                .map(|base| base.join(APP_FOLDER))
                .ok_or(NoHomeFolder(name))
        };
        Ok(Self {
            config: dir("XDG_CONFIG_HOME", ".config")?,
            state: dir("XDG_STATE_HOME", ".local/state")?,
            data: dir("XDG_DATA_HOME", ".local/share")?,
            cache: dir("XDG_CACHE_HOME", ".cache")?,
        })
    }
}

/// Replaces the file at `path` with `contents`: written to a temporary file in the
/// same folder, synced, renamed over the old one, and the folder synced. The folder
/// is created if needed. Readers see the old file or the new one, never a mixture.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let folder = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(folder)?;
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = folder.join(name);
    let written = (|| {
        let mut file = File::create(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
        return written;
    }
    // The rename is durable once the folder is synced. Some file systems cannot sync
    // a folder; the file itself is already safe there.
    if let Ok(dir) = File::open(folder) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Reads the file at `path`, or `None` when there is none.
pub(crate) fn read_if_exists(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Removes the file at `path` if it is there.
pub(crate) fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn dirs(vars: &[(&str, &str)]) -> Result<AppDirs, NoHomeFolder> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        AppDirs::from_vars(|name| vars.get(name).cloned())
    }

    #[test]
    fn xdg_variables_come_first() {
        let found = dirs(&[
            ("HOME", "/home/a"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_STATE_HOME", "relative"),
        ])
        .unwrap();
        assert_eq!(found.config, PathBuf::from("/cfg/helikodu"));
        assert_eq!(found.state, PathBuf::from("/home/a/.local/state/helikodu"));
        assert_eq!(found.data, PathBuf::from("/home/a/.local/share/helikodu"));
        assert_eq!(found.cache, PathBuf::from("/home/a/.cache/helikodu"));
        assert_eq!(dirs(&[]), Err(NoHomeFolder("XDG_CONFIG_HOME")));
    }

    #[test]
    fn atomic_writes_replace_whole_files() {
        let dir = std::env::temp_dir().join(format!("hk-service-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("file.toml");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        // No temporary file is left behind.
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        assert_eq!(read_if_exists(&dir.join("none")).unwrap(), None);
        remove_if_exists(&path).unwrap();
        remove_if_exists(&path).unwrap();
        assert!(!path.exists());
    }
}
