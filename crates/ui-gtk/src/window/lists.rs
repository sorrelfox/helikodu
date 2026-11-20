// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the tests and screenshots need of the Tracks table's columns beyond the
//! tags: the Labels column and the columns of the user's fields.

use super::MainWindow;
use crate::backend::TrackColumn;

impl MainWindow {
    /// The actions of the user's fields in the Columns menu of Tracks, in
    /// order (for tests).
    #[doc(hidden)]
    pub fn track_field_columns(&self) -> Vec<String> {
        self.pages()
            .map(|p| p.tracks().field_column_names())
            .unwrap_or_default()
    }

    /// Shows or hides a column of the Tracks table, as its item in the Columns
    /// menu would (for tests).
    #[doc(hidden)]
    pub fn set_track_column_shown(&self, column: TrackColumn, shown: bool) {
        if let Some(pages) = self.pages() {
            pages.tracks().set_column_shown(column, shown);
        }
    }

    /// The texts of the shown cells of a column of the Tracks table by row, and
    /// the tooltip of its header (for tests).
    #[doc(hidden)]
    pub fn track_column_texts(&self, column: TrackColumn) -> (Vec<(u32, String)>, Option<String>) {
        self.pages()
            .map(|p| p.tracks().column_texts(column))
            .unwrap_or_default()
    }

    /// Whether a header of the Tracks table sorts when clicked (for tests).
    #[doc(hidden)]
    pub fn track_column_sorts(&self, column: TrackColumn) -> bool {
        self.pages()
            .is_some_and(|p| p.tracks().column_sorts(column))
    }

    /// Opens the Columns menu of Tracks (for screenshots).
    #[doc(hidden)]
    pub fn demo_columns_menu(&self) -> Option<gtk::Popover> {
        let button = self.pages()?.tracks().columns_button()?;
        button.popup();
        button.popover()
    }
}
