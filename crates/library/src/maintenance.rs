// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Housekeeping on the files of the library: moving files to the desktop trash, and
//! the backups of the database.

pub(crate) mod backups;
mod localtime;
pub(crate) mod trash;

pub use backups::{
    BACKUP_FOLDER, BACKUP_INTERVAL, BackupError, BackupFile, Backups, DatabaseCheck, KEPT_BACKUPS,
    RestoreOutcome, apply_pending_restore, cancel_restore, pending_restore, recover_damaged,
    restore_path, stage_restore,
};
pub use trash::{Trash, TrashError, Trashed};
