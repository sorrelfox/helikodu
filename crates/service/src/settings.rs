// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings store: `settings.toml` read into the model's [`Settings`], changed
//! through the store, saved by atomic replace and announced on the event bus; and the
//! window state in `ui-state.toml`.
//!
//! - **Loading.** A missing file gives the defaults. A file without a `version` was
//!   written before versions were recorded (version 0); it and every older version
//!   is migrated step by step ([`migrate`]) before it is read. Keys this version
//!   does not know are ignored.
//! - **Bad files never keep the program from starting.** A file that is not valid
//!   TOML, or has a value of the wrong type, gives the defaults; a value out of range
//!   resets its table (`[playback]`, …) to the defaults. Either way
//!   [`SettingsStore::load_notice`] says what happened, and the file is left alone
//!   until a setting changes; it is then kept as `settings.toml.bak` before the new
//!   file replaces it.
//! - **A newer file** (a `version` above [`Settings::VERSION`]) is read as far as this
//!   version understands it and never written: changes are refused with
//!   [`SettingsStoreError::NewerFile`], so that the newer program's settings survive.
//! - **Changes** go through [`SettingsStore::update`]: validated, saved, then
//!   published as [`SettingsEvent::Changed`] with the groups that changed.
//!   [`applies`] tells front ends when a setting takes effect.

mod migrate;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use helikodu_model::settings::{Applies, Settings, SettingsError, SettingsGroup, UiState};
use thiserror::Error;

use crate::Event;
use crate::bus::EventBus;
use crate::files::{read_if_exists, write_atomic};

pub use migrate::{MIGRATIONS, Migration, migrate};

/// The file name of the settings.
pub const SETTINGS_FILE: &str = "settings.toml";

/// The file name of the window state.
pub const UI_STATE_FILE: &str = "ui-state.toml";

/// What the settings store announces.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SettingsEvent {
    /// Settings changed and were saved.
    Changed {
        /// The tables whose values changed.
        groups: Vec<SettingsGroup>,
        /// All settings as they are now.
        settings: Arc<Settings>,
    },
}

/// Why the settings could not be changed or saved.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SettingsStoreError {
    /// A value is not allowed; nothing was changed.
    #[error(transparent)]
    Invalid(#[from] SettingsError),
    /// The file was written by a newer version, which this one does not overwrite.
    #[error(
        "the settings file was written by a newer version (settings version {0}) and is not changed"
    )]
    NewerFile(u32),
    /// The file could not be written.
    #[error("cannot save the settings to {}: {source}", .path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: io::Error,
    },
}

/// Something about the settings file that the user should hear of after loading it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoadNotice {
    /// The file could not be read or parsed; the defaults are used.
    Unreadable {
        /// Why.
        reason: String,
    },
    /// Values were out of range; their tables were reset to the defaults.
    Reset {
        /// The problems found, one per reset table.
        problems: Vec<SettingsError>,
    },
    /// The file comes from a newer version; it is read but not written.
    Newer {
        /// Its settings version.
        version: u32,
    },
}

impl fmt::Display for LoadNotice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { reason } => {
                write!(
                    f,
                    "the settings could not be read ({reason}); the defaults are used"
                )
            }
            Self::Reset { problems } => {
                f.write_str("some settings were reset to their defaults: ")?;
                for (i, problem) in problems.iter().enumerate() {
                    if i > 0 {
                        f.write_str("; ")?;
                    }
                    write!(f, "{problem}")?;
                }
                Ok(())
            }
            Self::Newer { version } => write!(
                f,
                "the settings come from a newer version (settings version {version}); changes are not saved"
            ),
        }
    }
}

#[derive(Debug)]
struct State {
    settings: Arc<Settings>,
    /// The version of a newer file, which is never written.
    newer: Option<u32>,
    /// The file could not be used as it is: keep a copy before replacing it.
    keep_backup: bool,
}

/// The settings, shared by every part of the service. All methods may be called from
/// any thread.
#[derive(Debug)]
pub struct SettingsStore {
    path: PathBuf,
    state: Mutex<State>,
    notice: Option<LoadNotice>,
    bus: EventBus<Event>,
}

impl SettingsStore {
    /// Loads the settings from `path` (see the module documentation). It does not
    /// fail: problems with the file are reported by [`load_notice`](Self::load_notice).
    pub fn open(path: impl Into<PathBuf>, bus: EventBus<Event>) -> Self {
        let path = path.into();
        let (settings, notice) = load(&path);
        let newer = match &notice {
            Some(LoadNotice::Newer { version }) => Some(*version),
            _ => None,
        };
        let keep_backup = matches!(
            notice,
            Some(LoadNotice::Unreadable { .. } | LoadNotice::Reset { .. })
        );
        Self {
            path,
            state: Mutex::new(State {
                settings: Arc::new(settings),
                newer,
                keep_backup,
            }),
            notice,
            bus,
        }
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What was wrong with the file when it was loaded, if anything.
    pub fn load_notice(&self) -> Option<&LoadNotice> {
        self.notice.as_ref()
    }

    /// The settings as they are now.
    pub fn get(&self) -> Arc<Settings> {
        Arc::clone(&self.lock().settings)
    }

    /// Changes the settings with `change`, then validates, saves and announces them.
    /// Returns the groups that changed: none when `change` left everything as it was,
    /// in which case nothing is written or announced.
    pub fn update(
        &self,
        change: impl FnOnce(&mut Settings),
    ) -> Result<Vec<SettingsGroup>, SettingsStoreError> {
        let mut state = self.lock();
        let mut settings = Settings::clone(&state.settings);
        change(&mut settings);
        settings.version = Settings::VERSION;
        self.commit(&mut state, settings)
    }

    /// Puts every setting back to its default, keeping the library folders.
    pub fn reset(&self) -> Result<Vec<SettingsGroup>, SettingsStoreError> {
        let mut state = self.lock();
        let mut settings = Settings::default();
        settings.library.folders = state.settings.library.folders.clone();
        self.commit(&mut state, settings)
    }

    /// Reads the file again after it was edited outside the program, and announces
    /// what changed. A file that cannot be used now is ignored: the settings stay.
    pub fn reload(&self) -> Option<LoadNotice> {
        let (settings, notice) = load(&self.path);
        if matches!(notice, Some(LoadNotice::Unreadable { .. })) {
            return notice;
        }
        let mut state = self.lock();
        let groups = state.settings.changed_groups(&settings);
        state.settings = Arc::new(settings);
        if !groups.is_empty() {
            self.announce(groups, Arc::clone(&state.settings));
        }
        notice
    }

    fn commit(
        &self,
        state: &mut State,
        settings: Settings,
    ) -> Result<Vec<SettingsGroup>, SettingsStoreError> {
        if let Some(version) = state.newer {
            return Err(SettingsStoreError::NewerFile(version));
        }
        settings.validate()?;
        let groups = state.settings.changed_groups(&settings);
        if groups.is_empty() {
            return Ok(groups);
        }
        let write_error = |source| SettingsStoreError::Write {
            path: self.path.clone(),
            source,
        };
        if state.keep_backup {
            if let Some(old) = read_if_exists(&self.path).map_err(write_error)? {
                write_atomic(&backup_path(&self.path), &old).map_err(write_error)?;
            }
            state.keep_backup = false;
        }
        write_atomic(&self.path, to_toml(&settings).as_bytes()).map_err(write_error)?;
        state.settings = Arc::new(settings);
        self.announce(groups.clone(), Arc::clone(&state.settings));
        Ok(groups)
    }

    fn announce(&self, groups: Vec<SettingsGroup>, settings: Arc<Settings>) {
        self.bus.publish(&Event::Settings(SettingsEvent::Changed {
            groups,
            settings,
        }));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// When a change of the setting `key` (as in `settings.toml`, for example
/// `output.device`) takes effect.
///
/// The output device, backend and mode switch at once without stopping. The other
/// output settings and loudness normalization are fixed when the audio engine
/// starts, which for now means the next start of the program.
pub fn applies(key: &str) -> Applies {
    match key.split_once('.').map_or(key, |(group, _)| group) {
        "output" => match key {
            "output.backend" | "output.device" | "output.mode" => Applies::Live,
            _ => Applies::Restart,
        },
        "loudness" => Applies::Restart,
        _ => Applies::Live,
    }
}

/// Reads the settings at `path`, and what was wrong with them.
fn load(path: &Path) -> (Settings, Option<LoadNotice>) {
    let unreadable =
        |reason: String| (Settings::default(), Some(LoadNotice::Unreadable { reason }));
    let bytes = match read_if_exists(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return (Settings::default(), None),
        Err(error) => return unreadable(error.to_string()),
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return unreadable("the file is not UTF-8 text".into());
    };
    let mut table: toml::Table = match toml::from_str(&text) {
        Ok(table) => table,
        Err(error) => return unreadable(error.message().to_owned()),
    };
    let from = match migrate(&mut table, MIGRATIONS) {
        Ok(from) => from,
        Err(error) => return unreadable(error),
    };
    let mut settings: Settings = match table.try_into() {
        Ok(settings) => settings,
        Err(error) => return unreadable(error.message().to_owned()),
    };
    if from > Settings::VERSION {
        return (settings, Some(LoadNotice::Newer { version: from }));
    }
    settings.version = Settings::VERSION;
    let mut problems = Vec::new();
    while let Err(problem) = settings.validate() {
        if !reset_group(&mut settings, &problem.key) {
            return unreadable(problem.to_string());
        }
        problems.push(problem);
    }
    let notice = (!problems.is_empty()).then_some(LoadNotice::Reset { problems });
    (settings, notice)
}

/// Resets the table that `key` belongs to; false if there is no such table.
fn reset_group(settings: &mut Settings, key: &str) -> bool {
    let defaults = Settings::default();
    match key.split('.').next() {
        Some("library") => {
            let folders = std::mem::take(&mut settings.library.folders);
            settings.library = defaults.library;
            // Folders that are absolute paths stay: losing them would empty the library.
            settings.library.folders = folders.into_iter().filter(|p| p.is_absolute()).collect();
        }
        Some("playback") => settings.playback = defaults.playback,
        Some("output") => settings.output = defaults.output,
        Some("loudness") => settings.loudness = defaults.loudness,
        Some("advanced") => settings.advanced = defaults.advanced,
        _ => return false,
    }
    true
}

fn to_toml(settings: &Settings) -> String {
    let body = toml::to_string(settings).unwrap_or_default();
    format!(
        "# Helikodu settings. Written by the program; edits made while it runs are read\n\
         # when it is told to reload them.\n\n{body}"
    )
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".bak");
    PathBuf::from(name)
}

/// The window state in `ui-state.toml`: read when the window opens, written when it
/// closes. A missing or broken file gives the defaults; values out of range are
/// moved into range.
#[derive(Debug, Clone)]
pub struct UiStateStore {
    path: PathBuf,
}

impl UiStateStore {
    /// The store for the file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The saved state, or the defaults.
    pub fn load(&self) -> UiState {
        read_if_exists(&self.path)
            .ok()
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| toml::from_str::<UiState>(&text).ok())
            .unwrap_or_default()
            .clamped()
    }

    /// Saves `state` by atomic replace.
    pub fn save(&self, state: &UiState) -> io::Result<()> {
        let mut state = state.clone().clamped();
        state.version = UiState::VERSION;
        let text = toml::to_string(&state).map_err(io::Error::other)?;
        write_atomic(&self.path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_selection_is_live_and_formats_are_not() {
        assert_eq!(applies("output.device"), Applies::Live);
        assert_eq!(applies("output.buffer_ms"), Applies::Restart);
        assert_eq!(applies("loudness.mode"), Applies::Restart);
        assert_eq!(applies("playback.seek_step_s"), Applies::Live);
        assert_eq!(applies("appearance"), Applies::Live);
    }

    #[test]
    fn backups_sit_beside_the_file() {
        assert_eq!(
            backup_path(Path::new("/c/helikodu/settings.toml")),
            PathBuf::from("/c/helikodu/settings.toml.bak")
        );
    }
}
