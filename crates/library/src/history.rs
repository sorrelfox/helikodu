// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! One undo history over tag edits and file runs: [`Library::undo_last`]
//! undoes the newest of either, [`Library::redo_last`] does the most recently
//! undone one again, as Ctrl+Z and Ctrl+Shift+Z do outside an open playlist.
//!
//! Both journals keep their own steps (see `tag_journal` and `organize`); a
//! shared `seq` orders them. Undo takes the step done last. Redo takes, of the
//! undone steps, the one done earliest (the one undone last). Something new
//! after an undo ends what can be done again, as in any undo history: a new run
//! drops the tag edits that wait to be done again (as a new tag edit does), and
//! a run cannot be done again once a tag edit was made after it was undone.

use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension};

use crate::Library;
use crate::db::DbError;
use crate::organize::{FileRunSummary, OrganizeError, Progress, RunReport};
use crate::scan::{ScanError, ScanOptions};
use crate::tag_journal::{TagStep, TagStepReport, next_step};
use crate::tag_writes::TagWriteSettings;

/// SQL for the `seq` of a new step, with the time in milliseconds as the
/// parameter `?{param}`: that time, or one more than the newest step of either
/// journal when that is later.
pub(crate) fn next_seq(param: usize) -> String {
    format!(
        "max(?{param},
             coalesce((SELECT max(seq) FROM tag_journal), 0) + 1,
             coalesce((SELECT max(seq) FROM file_runs), 0) + 1,
             coalesce((SELECT max(undone_seq) FROM file_runs), 0) + 1)"
    )
}

/// Now, in milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A step of the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryAction {
    /// A saved tag edit.
    Tags(TagStep),
    /// A run of the file organizer, or a file renamed.
    Files(FileRunSummary),
}

/// What Ctrl+Z and Ctrl+Shift+Z would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UndoHistory {
    /// What [`Library::undo_last`] would undo.
    pub undo: Option<HistoryAction>,
    /// What [`Library::redo_last`] would do again.
    pub redo: Option<HistoryAction>,
}

/// What an undo or a redo of the history did.
#[derive(Debug)]
pub enum HistoryReport {
    /// A tag edit was undone or done again.
    Tags(TagStepReport),
    /// A run was undone or done again.
    Files {
        /// The run, as it was before.
        run: FileRunSummary,
        /// What happened to its files.
        report: RunReport,
    },
}

/// Why the history could not take a step.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A tag edit could not be undone or done again.
    #[error(transparent)]
    Tags(#[from] ScanError),
    /// A run could not be undone or done again.
    #[error(transparent)]
    Files(#[from] OrganizeError),
}

fn seq_of(c: &Connection, id: i64) -> rusqlite::Result<i64> {
    Ok(
        c.query_row("SELECT seq FROM tag_journal WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()?
        .unwrap_or(0),
    )
}

/// The candidates, each with its `seq`.
struct Candidates {
    tag_undo: Option<(TagStep, i64)>,
    tag_redo: Option<(TagStep, i64)>,
    /// The newest run, its `seq`, and whether it can still be done again.
    run: Option<(FileRunSummary, i64, bool)>,
}

/// The older (`older`) or the newer of a tag step and a run.
fn pick(
    tag: Option<&(TagStep, i64)>,
    run: Option<(&FileRunSummary, i64)>,
    older: bool,
) -> Option<HistoryAction> {
    match (tag, run) {
        (Some((tag, a)), Some((run, b))) => Some(if (*a < b) == older {
            HistoryAction::Tags(tag.clone())
        } else {
            HistoryAction::Files(run.clone())
        }),
        (Some((tag, _)), None) => Some(HistoryAction::Tags(tag.clone())),
        (None, Some((run, _))) => Some(HistoryAction::Files(run.clone())),
        (None, None) => None,
    }
}

impl Candidates {
    fn undo(&self) -> Option<HistoryAction> {
        let run = self
            .run
            .as_ref()
            .filter(|(r, _, _)| r.can_undo())
            .map(|(r, s, _)| (r, *s));
        pick(self.tag_undo.as_ref(), run, false)
    }

    fn redo(&self) -> Option<HistoryAction> {
        let run = self
            .run
            .as_ref()
            .filter(|(r, _, open)| r.can_redo() && *open)
            .map(|(r, s, _)| (r, *s));
        pick(self.tag_redo.as_ref(), run, true)
    }
}

impl Library {
    fn candidates(&self) -> Result<Candidates, HistoryError> {
        let run = self.last_file_run()?;
        Ok(self.database().read(move |c| {
            let tag = |redo: bool| -> rusqlite::Result<Option<(TagStep, i64)>> {
                next_step(c, redo)?
                    .map(|step| seq_of(c, step.id.0).map(|seq| (step.clone(), seq)))
                    .transpose()
            };
            Ok(Candidates {
                tag_undo: tag(false)?,
                tag_redo: tag(true)?,
                run: match run {
                    None => None,
                    Some(run) => {
                        let (seq, undone_seq, newest_tag): (i64, i64, Option<i64>) = c.query_row(
                            "SELECT seq, undone_seq, (SELECT max(seq) FROM tag_journal)
                                 FROM file_runs WHERE id = ?1",
                            [run.run.get()],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )?;
                        let open = newest_tag.is_none_or(|t| t < undone_seq);
                        Some((run, seq, open))
                    }
                },
            })
        })?)
    }

    /// What Ctrl+Z and Ctrl+Shift+Z would undo and do again, across tag edits
    /// and file runs.
    pub fn undo_history(&self) -> Result<UndoHistory, HistoryError> {
        let candidates = self.candidates()?;
        Ok(UndoHistory {
            undo: candidates.undo(),
            redo: candidates.redo(),
        })
    }

    /// Undoes the newest tag edit or file run, whichever was done last.
    /// `None` when there is nothing to undo.
    pub fn undo_last(
        &self,
        settings: &TagWriteSettings,
        scan: &ScanOptions,
        progress: Progress<'_>,
    ) -> Result<Option<HistoryReport>, HistoryError> {
        let action = self.candidates()?.undo();
        self.take_step(action, false, settings, scan, progress)
    }

    /// Does again the tag edit or file run that was undone last, unless
    /// something was done since. `None` when there is nothing to do again.
    pub fn redo_last(
        &self,
        settings: &TagWriteSettings,
        scan: &ScanOptions,
        progress: Progress<'_>,
    ) -> Result<Option<HistoryReport>, HistoryError> {
        let action = self.candidates()?.redo();
        self.take_step(action, true, settings, scan, progress)
    }

    fn take_step(
        &self,
        action: Option<HistoryAction>,
        redo: bool,
        settings: &TagWriteSettings,
        scan: &ScanOptions,
        progress: Progress<'_>,
    ) -> Result<Option<HistoryReport>, HistoryError> {
        Ok(match action {
            None => None,
            Some(HistoryAction::Tags(_)) => {
                let report = if redo {
                    self.redo_tags(settings, scan)?
                } else {
                    self.undo_tags(settings, scan)?
                };
                report.map(HistoryReport::Tags)
            }
            Some(HistoryAction::Files(run)) => {
                let report = if redo {
                    self.redo_file_run(run.run, progress)?
                } else {
                    self.undo_file_run(run.run, progress)?
                };
                Some(HistoryReport::Files { run, report })
            }
        })
    }
}
