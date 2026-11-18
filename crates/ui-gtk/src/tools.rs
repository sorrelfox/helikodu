// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tool windows: separate, non-modal windows in the layout of Supported
//! formats (a list at the left, options, a table, the action in the footer),
//! and the per-track copies window built from the same table. Nothing changes
//! before the footer's button is used, and files only ever go to the desktop
//! trash after a confirmation.

pub(crate) mod copies;
pub(crate) mod duplicates;
pub(crate) mod tool_window;
pub(crate) mod trash;
