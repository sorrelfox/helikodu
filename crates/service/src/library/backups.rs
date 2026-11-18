// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening the library at start, its backups, and putting a backup back.
//!
//! - [`MusicLibrary::open_at_start`] opens the database the way the application
//!   wants it: a backup chosen to be put back is put back first; a damaged
//!   database is replaced with the newest good backup; a database written by a
//!   newer version, or one on a medium that cannot be written, is opened
//!   read-only; organizer runs a crash cut short are settled. Each of these is
//!   reported as a [`StartNotice`] for the window to show.
//! - A backup is made once a day while the program runs ([`BackupScheduler`]),
//!   and on demand ([`MusicLibrary::back_up_now`]); the newest
//!   [`KEPT_BACKUPS`] are kept in `backups/` beside the database.
//! - [`MusicLibrary::stage_restore`] checks a backup and stages it; it is put
//!   back at the next start, never under the running program.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use helikodu_library::backups::{self as store};
pub use helikodu_library::backups::{
    BACKUP_FOLDER, BACKUP_INTERVAL, BackupError, BackupFile, Backups, DatabaseCheck, KEPT_BACKUPS,
};
use helikodu_library::{DbError, Library};

use super::{LibraryError, MusicLibrary, config};

/// Something the window should tell after the library was opened.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartNotice {
    /// The database can only be read; changes are refused. The text says why.
    ReadOnly(String),
    /// The backup chosen before was put back. The replaced database was saved
    /// as `saved_copy`.
    Restored {
        /// Where the replaced database went, if it could be saved.
        saved_copy: Option<PathBuf>,
    },
    /// Putting back the chosen backup failed; the database is as it was.
    RestoreFailed(String),
    /// The database was damaged and was replaced with the backup made at
    /// `made`. The damaged files were set aside.
    Recovered {
        /// When the backup was made.
        made: SystemTime,
    },
    /// The library could not be opened at all, so Helikodu runs without one.
    /// The text says why.
    Unavailable(String),
}

/// Backups are made one at a time, by the scheduler or on demand.
static BACKING_UP: Mutex<()> = Mutex::new(());

impl MusicLibrary {
    /// Opens the library database at `path` for the application, as the
    /// [module documentation](self) describes, with what the window should say.
    /// It is sorted for the interface language `language` (the `[appearance]
    /// language` setting; empty for the environment's locale).
    pub fn open_at_start(
        path: &Path,
        language: &str,
    ) -> (Result<Self, LibraryError>, Vec<StartNotice>) {
        let mut notices = Vec::new();
        let backups = Backups::beside(path);
        if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty())
            && let Err(source) = std::fs::create_dir_all(folder)
        {
            let error = LibraryError::Io {
                context: format!("cannot create {}", folder.display()),
                source,
            };
            notices.push(StartNotice::Unavailable(error.to_string()));
            return (Err(error), notices);
        }
        match store::apply_pending_restore(path, &backups, SystemTime::now()) {
            Ok(Some(outcome)) => {
                log::info!("put a backup back; the replaced database is {outcome:?}");
                notices.push(StartNotice::Restored {
                    saved_copy: outcome.saved_copy,
                });
            }
            Ok(None) => {}
            Err(error) => {
                log::error!("cannot put the chosen backup back: {error}");
                // Trying again at every start would fail the same way.
                let _ = store::cancel_restore(path);
                notices.push(StartNotice::RestoreFailed(error.to_string()));
            }
        }
        let config = config(path, language);
        let opened = match Library::open(&config) {
            Err(error) if error.is_damaged_database() => {
                log::error!("the library database is damaged: {error}");
                match store::recover_damaged(path, &backups, SystemTime::now()) {
                    Ok(Some((backup, _))) => {
                        log::warn!(
                            "replaced the damaged database with {}",
                            backup.path.display()
                        );
                        notices.push(StartNotice::Recovered { made: backup.made });
                        Library::open(&config)
                    }
                    Ok(None) => Err(error),
                    Err(recovery) => {
                        log::error!("cannot recover the database from a backup: {recovery}");
                        Err(error)
                    }
                }
            }
            other => other,
        };
        let opened = match opened {
            Err(error @ DbError::NewerSchema { .. }) => {
                log::warn!("{error}; opening the library read-only");
                notices.push(StartNotice::ReadOnly(error.to_string()));
                Library::open_read_only(&config)
            }
            Err(error) if error.is_read_only_medium() && path.is_file() => {
                log::warn!("{error}; opening the library read-only");
                notices.push(StartNotice::ReadOnly(error.to_string()));
                Library::open_read_only(&config)
            }
            other => other,
        };
        match opened {
            Ok(library) => {
                if library.database().writer().is_ok() {
                    match library.recover_file_runs() {
                        Ok(0) => {}
                        Ok(n) => log::info!("settled {n} file moves a crash cut short"),
                        Err(e) => log::warn!("cannot settle the file moves a crash cut short: {e}"),
                    }
                }
                let music = Self {
                    library: Arc::new(library),
                    path: path.to_path_buf(),
                };
                (Ok(music), notices)
            }
            Err(error) => {
                let error = LibraryError::from(error);
                log::error!("cannot open the library at {}: {error}", path.display());
                notices.retain(|n| !matches!(n, StartNotice::ReadOnly(_)));
                notices.push(StartNotice::Unavailable(error.to_string()));
                (Err(error), notices)
            }
        }
    }

    /// The backups folder of this library.
    pub fn backups(&self) -> Backups {
        Backups::beside(&self.path)
    }

    /// Whether the library can only be read.
    pub fn is_read_only(&self) -> bool {
        self.library.database().writer().is_err()
    }

    /// Makes a backup now and removes the oldest beyond the kept number.
    pub fn back_up_now(&self) -> Result<BackupFile, LibraryError> {
        let _one = BACKING_UP.lock().unwrap_or_else(PoisonError::into_inner);
        let made = self.library.back_up(&self.backups(), SystemTime::now());
        match &made {
            Ok(file) => log::info!("made the backup {}", file.path.display()),
            Err(error) => log::warn!("cannot make a backup: {error}"),
        }
        Ok(made?)
    }

    /// Checks the backup at `backup` and stages it to be put back at the next
    /// start.
    pub fn stage_restore(&self, backup: &Path) -> Result<(), LibraryError> {
        store::stage_restore(&self.path, backup)?;
        log::info!(
            "staged {} to be put back at the next start",
            backup.display()
        );
        Ok(())
    }

    /// The backup waiting to be put back at the next start.
    pub fn pending_restore(&self) -> Option<PathBuf> {
        store::pending_restore(&self.path)
    }

    /// Drops the backup waiting to be put back.
    pub fn cancel_restore(&self) -> Result<(), LibraryError> {
        Ok(store::cancel_restore(&self.path)?)
    }
}

/// Makes a backup whenever one is due, on its own thread, while it lives.
#[derive(Debug)]
pub struct BackupScheduler {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl BackupScheduler {
    /// Starts the schedule for `library`. The first check waits `first_delay`,
    /// so that a backup does not compete with the start of the program.
    pub fn start(library: MusicLibrary, first_delay: Duration) -> std::io::Result<Self> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let thread = {
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("helikodu-backups".into())
                .spawn(move || schedule(&library, &stop, first_delay))?
        };
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

fn schedule(library: &MusicLibrary, stop: &(Mutex<bool>, Condvar), first_delay: Duration) {
    let (stopped, wake) = stop;
    let mut wait = first_delay;
    loop {
        let guard = stopped.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = wake
            .wait_timeout_while(guard, wait, |stopped| !*stopped)
            .unwrap_or_else(PoisonError::into_inner);
        if *guard {
            return;
        }
        drop(guard);
        let backups = library.backups();
        let now = SystemTime::now();
        wait = if backups.is_due(now) {
            match library.back_up_now() {
                Ok(_) => BACKUP_INTERVAL,
                // A full disk or a damaged database: try again in an hour.
                Err(_) => Duration::from_secs(60 * 60),
            }
        } else {
            backups
                .next_due(now)
                .duration_since(now)
                .unwrap_or_default()
                .max(Duration::from_secs(60))
        };
    }
}

impl Drop for BackupScheduler {
    fn drop(&mut self) {
        let (stopped, wake) = &*self.stop;
        *stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
