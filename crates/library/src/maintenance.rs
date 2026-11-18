// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Housekeeping on the files of the library: moving files to the desktop trash.

mod localtime;
pub(crate) mod trash;

pub use trash::{Trash, TrashError, Trashed};
