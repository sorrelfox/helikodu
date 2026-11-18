// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing many files: one result per file, progress, cancellation between files.

use std::path::PathBuf;

use super::changes::TagChanges;
use super::editable::EditableFile;
use super::safe_save::EditStamp;
use super::{WriteError, WriteOptions, WriteOutcome, write_tags};

/// One file of a batch and what to write into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchItem {
    /// The file.
    pub path: PathBuf,
    /// Its stamp when it was read; a file that changed since is refused.
    pub expected: Option<EditStamp>,
    /// The changes. A selection usually gives every file the same ones.
    pub changes: TagChanges,
}

impl BatchItem {
    /// `changes` for a file loaded with [`load_for_editing`](super::load_for_editing).
    pub fn for_file(file: &EditableFile, changes: TagChanges) -> Self {
        Self {
            path: file.path.clone(),
            expected: Some(file.stamp),
            changes,
        }
    }
}

/// How far a batch has come, reported after each file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchProgress {
    /// The files finished (written, unchanged or failed).
    pub done: usize,
    /// The files in the batch.
    pub total: usize,
    /// The file just finished.
    pub path: PathBuf,
    /// Whether it failed.
    pub failed: bool,
}

/// The result of a batch.
#[derive(Debug)]
pub struct BatchReport {
    /// Each file's result, in the order of the batch. Files not reached because
    /// the batch was cancelled have [`WriteError::Cancelled`].
    pub results: Vec<(PathBuf, Result<WriteOutcome, WriteError>)>,
    /// Whether the batch was cancelled before its end.
    pub cancelled: bool,
}

impl BatchReport {
    /// The files written (or left unchanged because nothing changed).
    pub fn written(&self) -> impl Iterator<Item = &WriteOutcome> {
        self.results.iter().filter_map(|(_, r)| r.as_ref().ok())
    }

    /// The files that failed, other than those not reached after a cancellation.
    pub fn failed(&self) -> impl Iterator<Item = (&PathBuf, &WriteError)> {
        self.results.iter().filter_map(|(path, r)| match r {
            Err(WriteError::Cancelled) | Ok(_) => None,
            Err(e) => Some((path, e)),
        })
    }
}

/// Writes each item in turn. A failure is recorded and the batch goes on;
/// `cancelled` is asked before each file, and once it says yes the remaining
/// files are left untouched. `progress` is called after each file.
pub fn write_batch(
    items: &[BatchItem],
    options: &WriteOptions,
    progress: &mut dyn FnMut(&BatchProgress),
    cancelled: &dyn Fn() -> bool,
) -> BatchReport {
    let mut results = Vec::with_capacity(items.len());
    let mut stopped = false;
    for (index, item) in items.iter().enumerate() {
        stopped = stopped || cancelled();
        let result = if stopped {
            Err(WriteError::Cancelled)
        } else {
            write_tags(&item.path, &item.changes, item.expected, options)
        };
        if !stopped {
            progress(&BatchProgress {
                done: index + 1,
                total: items.len(),
                path: item.path.clone(),
                failed: result.is_err(),
            });
        }
        results.push((item.path.clone(), result));
    }
    BatchReport {
        results,
        cancelled: stopped,
    }
}
