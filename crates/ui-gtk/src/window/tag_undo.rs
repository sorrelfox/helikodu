// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Undo and redo of tag edits and file runs: the toast after a save offers
//! Undo, and Ctrl+Z and Ctrl+Shift+Z step through one history of tag edits and
//! runs of the file organizer, newest first, when no toast and no open playlist
//! claims them; a toast says what was undone. A file changed since an edit
//! keeps its tags, and a file that cannot move stays; the toast says how many.

use gtk::prelude::*;
use gtk::{gio, glib};

use super::MainWindow;
use crate::backend::services::{HistoryStepResult, RunKind, RunReport};
use crate::format;
use crate::i18n::{fill, ntr, tr};

impl MainWindow {
    /// A toast saying `text`, whose Undo undoes the newest tag edit.
    pub(crate) fn toast_tags_undo(&self, text: &str) {
        let weak = self.downgrade();
        self.toast_undo(text, move || {
            if let Some(window) = weak.upgrade() {
                window.step_tags(false);
            }
        });
    }

    /// Undoes the newest tag edit, or with `redo` does the newest undone one
    /// again.
    pub(crate) fn step_tags(&self, redo: bool) {
        let Some(backend) = self.backend() else {
            return;
        };
        let services = backend.services();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || services.step_tags(redo))
                .await
                .unwrap_or(Ok(None));
            let Some(window) = weak.upgrade() else { return };
            match result {
                Ok(None) => window.toast(
                    &if redo {
                        tr("There is nothing to redo.")
                    } else {
                        tr("There is nothing to undo.")
                    },
                    false,
                ),
                Ok(Some(step)) if step.problems.is_empty() => {
                    let n = step.files;
                    let text = if redo {
                        ntr(
                            "Changed the tags of {} file again",
                            "Changed the tags of {} files again",
                            n,
                        )
                    } else {
                        ntr(
                            "Put back the tags of {} file",
                            "Put back the tags of {} files",
                            n,
                        )
                    };
                    let text = fill(&text, &[&n.to_string()]);
                    if redo {
                        window.toast_tags_undo(&text);
                    } else {
                        window.toast(&text, false);
                    }
                }
                Ok(Some(step)) => {
                    let (file, reason) = &step.problems[0];
                    window.toast(
                        &fill(
                            &ntr(
                                // Translators: {0} is the number of files that
                                // could not be changed back, which chooses the
                                // form, {1} the number of all files, {2} the first
                                // of them, {3} why.
                                "{} of {} files kept its tags. {}: {}",
                                "{} of {} files kept their tags. {}: {}",
                                step.problems.len(),
                            ),
                            &[
                                &step.problems.len().to_string(),
                                &step.files.to_string(),
                                file,
                                reason,
                            ],
                        ),
                        true,
                    );
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// Undoes the newest tag edit or file run, whichever came last, or with
    /// `redo` does the one undone last again, and says so in a toast.
    pub(crate) fn step_undo_history(&self, redo: bool) {
        let Some(backend) = self.backend() else {
            return;
        };
        let services = backend.services();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || services.step_history(redo))
                .await
                .unwrap_or(Ok(None));
            let Some(window) = weak.upgrade() else { return };
            match result {
                Ok(None) => window.toast(
                    &if redo {
                        tr("There is nothing to redo.")
                    } else {
                        tr("There is nothing to undo.")
                    },
                    false,
                ),
                Ok(Some(HistoryStepResult::Tags(step))) if !step.problems.is_empty() => {
                    let (file, reason) = &step.problems[0];
                    window.toast(
                        &fill(
                            &ntr(
                                // Translators: {0} is the number of files that
                                // could not be changed back, which chooses the
                                // form, {1} the number of all files, {2} the first
                                // of them, {3} why.
                                "{} of {} files kept its tags. {}: {}",
                                "{} of {} files kept their tags. {}: {}",
                                step.problems.len(),
                            ),
                            &[
                                &step.problems.len().to_string(),
                                &step.files.to_string(),
                                file,
                                reason,
                            ],
                        ),
                        true,
                    );
                }
                Ok(Some(step)) => {
                    let (text, problem) = history_text(&step, redo);
                    if redo && !problem {
                        let again = window.downgrade();
                        window.toast_undo(&text, move || {
                            if let Some(window) = again.upgrade() {
                                window.step_undo_history(false);
                            }
                        });
                    } else {
                        window.toast(&text, problem);
                    }
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }
}

/// What a step of the history did, in a sentence, and whether something stayed
/// as it was.
fn history_text(step: &HistoryStepResult, redo: bool) -> (String, bool) {
    match step {
        HistoryStepResult::Tags(step) => {
            let n = step.files;
            let text = if redo {
                ntr("Redid tag edit of {} file", "Redid tag edit of {} files", n)
            } else {
                ntr("Undid tag edit of {} file", "Undid tag edit of {} files", n)
            };
            (fill(&text, &[&format::count(n)]), false)
        }
        HistoryStepResult::Files { kind, report } => {
            let text = files_text(*kind, report, redo);
            let stayed = report.outcomes.len() - report.moved();
            if stayed == 0 {
                return (text, false);
            }
            let stayed_text = fill(
                &ntr(
                    "{} file stayed where it was",
                    "{} files stayed where they were",
                    stayed,
                ),
                &[&format::count(stayed)],
            );
            (fill(&tr("{} · {}"), &[&text, &stayed_text]), true)
        }
    }
}

fn files_text(kind: RunKind, report: &RunReport, redo: bool) -> String {
    let n = report.moved_files();
    match (kind, redo) {
        (RunKind::Rename, false) => tr("Undid renaming a file"),
        (RunKind::Rename, true) => tr("Redid renaming a file"),
        (_, false) => fill(
            &ntr("Undid moving {} file", "Undid moving {} files", n),
            &[&format::count(n)],
        ),
        (_, true) => fill(
            &ntr("Redid moving {} file", "Redid moving {} files", n),
            &[&format::count(n)],
        ),
    }
}
