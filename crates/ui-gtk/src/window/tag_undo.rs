// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Undo and redo of tag edits: the toast after a save offers Undo, and Ctrl+Z
//! and Ctrl+Shift+Z step through the undo journal of tag edits when no toast
//! and no open playlist claims them. A file changed since an edit keeps its
//! tags; the toast says how many.

use gtk::prelude::*;
use gtk::{gio, glib};

use super::MainWindow;
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
                            &tr("{} of {} files kept their tags. {}: {}"),
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
}
