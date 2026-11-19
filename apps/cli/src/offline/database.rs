// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `--database` option of the library commands, and paths in their JSON output.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Args;
use helikodu_service::files::AppDirs;
use helikodu_service::library::{MusicLibrary, default_database_path};
use helikodu_service::settings::{SETTINGS_FILE, read_settings};
use serde_json::{Map, Value, json};

#[derive(Debug, Args)]
pub(crate) struct DatabaseArgs {
    /// The library database [default: $XDG_DATA_HOME/helikodu/library.db]
    #[arg(long, value_name = "PATH")]
    database: Option<PathBuf>,
}

impl DatabaseArgs {
    /// The database file.
    pub(crate) fn path(&self) -> Result<PathBuf> {
        match &self.database {
            Some(path) => Ok(path.clone()),
            None => Ok(default_database_path()?),
        }
    }

    /// The library, created if there is none yet.
    pub(crate) fn open(&self) -> Result<MusicLibrary> {
        Ok(MusicLibrary::open_sorted_for(
            &self.path()?,
            &interface_language(),
        )?)
    }

    /// The library, which must exist.
    pub(crate) fn open_existing(&self) -> Result<MusicLibrary> {
        Ok(MusicLibrary::open_existing_sorted_for(
            &self.path()?,
            &interface_language(),
        )?)
    }
}

/// The application's settings, as its settings file has them (the defaults
/// when there is none).
pub(crate) fn app_settings() -> helikodu_model::settings::Settings {
    AppDirs::from_env()
        .ok()
        .and_then(|dirs| std::fs::read(dirs.config.join(SETTINGS_FILE)).ok())
        .map(|bytes| read_settings(&bytes).0)
        .unwrap_or_default()
}

/// The interface language of the application (`[appearance] language` in its
/// settings file; empty for the environment's locale), so that the library is
/// sorted as the application sorts it and its sort indexes are not built again
/// for another language.
fn interface_language() -> String {
    let Ok(dirs) = AppDirs::from_env() else {
        return String::new();
    };
    let Ok(bytes) = std::fs::read(dirs.config.join(SETTINGS_FILE)) else {
        return String::new();
    };
    read_settings(&bytes).0.appearance.language.clone()
}

/// Adds `path` to a JSON object: `"path"` as text (bytes that are not UTF-8 become
/// U+FFFD), and `"path_bytes"`, the exact bytes as an array of numbers, only when the
/// path is not valid UTF-8.
pub(crate) fn insert_path(object: &mut Map<String, Value>, path: &Path) {
    object.insert("path".into(), json!(path.to_string_lossy()));
    if path.to_str().is_none() {
        object.insert("path_bytes".into(), json!(path.as_os_str().as_bytes()));
    }
}

/// A JSON object holding only `path`, as [`insert_path`] writes it.
pub(crate) fn path_object(path: &Path) -> Value {
    let mut object = Map::new();
    insert_path(&mut object, path);
    Value::Object(object)
}
