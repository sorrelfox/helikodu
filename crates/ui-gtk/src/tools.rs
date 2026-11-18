// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tool windows: separate, non-modal windows in the layout of Supported
//! formats (a list at the left, options, a table, the action in the footer),
//! and the per-track copies window built from the same table. Nothing changes
//! before the footer's button is used, and files only ever go to the desktop
//! trash after a confirmation.

pub(crate) mod converter;
pub(crate) mod copies;
pub(crate) mod cover_manager;
pub(crate) mod duplicates;
pub(crate) mod file_tools;
pub(crate) mod jobs;
pub(crate) mod missing_files;
pub(crate) mod organizer;
pub(crate) mod rename;
pub(crate) mod sync_export;
pub(crate) mod tag_preview;
pub(crate) mod tag_tools;
pub(crate) mod tool_window;
pub(crate) mod trash;
pub(crate) mod verify_files;

use crate::backend::export::{PatternError, PatternErrorKind};
use crate::i18n::{fill, tr};

/// Why a naming pattern cannot be read, in a sentence.
pub(crate) fn naming_problem(error: &PatternError) -> String {
    let at = (error.position + 1).to_string();
    match &error.kind {
        PatternErrorKind::Empty => tr("The pattern is empty."),
        PatternErrorKind::UnknownField(name) => fill(
            &tr("{} is not a field (at {})."),
            &[&format!("{{{name}}}"), &at],
        ),
        PatternErrorKind::UnclosedField => fill(&tr("A { is not closed (at {})."), &[&at]),
        PatternErrorKind::UnclosedBracket => fill(&tr("A [ is not closed (at {})."), &[&at]),
        PatternErrorKind::Unopened(c) => {
            fill(&tr("{} was not opened (at {})."), &[&c.to_string(), &at])
        }
        PatternErrorKind::NestedBracket => {
            fill(&tr("Square brackets cannot be nested (at {})."), &[&at])
        }
        PatternErrorKind::FolderInBracket => {
            fill(&tr("A / cannot be inside square brackets (at {})."), &[&at])
        }
        PatternErrorKind::NoFileName => tr("The pattern ends with a folder: add a file name."),
        _ => error.to_string(),
    }
}
