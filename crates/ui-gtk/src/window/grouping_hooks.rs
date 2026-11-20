// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What tests and screenshots reach of the grouped track table.

use super::MainWindow;
use crate::backend::{TrackGrouping, TrackKey};

impl MainWindow {
    /// Groups the track table by `by`, or not (for tests and screenshots).
    #[doc(hidden)]
    pub fn group_tracks(&self, by: Option<TrackGrouping>) {
        if let Some(pages) = self.pages() {
            pages.tracks().set_grouping(by);
        }
    }

    /// The group rows of the track table: value, count and whether open (for
    /// tests).
    #[doc(hidden)]
    pub fn track_groups(&self) -> Vec<(String, String, bool)> {
        self.pages()
            .map(|p| p.tracks().group_rows())
            .unwrap_or_default()
    }

    /// Activates the group row shown as `value`; whether there is one (for
    /// tests).
    #[doc(hidden)]
    pub fn activate_track_group(&self, value: &str) -> bool {
        self.pages()
            .is_some_and(|p| p.tracks().demo_activate_group(value))
    }

    /// Focuses and selects the group row shown as `value`; whether there is
    /// one (for tests).
    #[doc(hidden)]
    pub fn focus_track_group(&self, value: &str) -> bool {
        self.pages()
            .is_some_and(|p| p.tracks().demo_focus_group(value))
    }

    /// Presses Right, or Left, on the focused row of the track table;
    /// whether a group row took it (for tests).
    #[doc(hidden)]
    pub fn press_on_track_group(&self, right: bool) -> bool {
        self.pages()
            .is_some_and(|p| p.tracks().group_key_pressed(right))
    }

    /// Selects the group row shown as `value` (for tests and screenshots).
    #[doc(hidden)]
    pub fn select_track_group(&self, value: &str) -> bool {
        let Some(pages) = self.pages() else {
            return false;
        };
        let Some(position) = pages.tracks().group_row_position(value) else {
            return false;
        };
        pages.tracks().select_rows(position, 1);
        true
    }

    /// The tracks the track table's context menu acts on (for tests).
    #[doc(hidden)]
    pub fn selected_track_keys(&self) -> Vec<TrackKey> {
        self.pages()
            .map(|p| p.tracks().selected_tracks())
            .unwrap_or_default()
    }

    /// The text of the "Group by" button and the left of the status bar (for
    /// tests).
    #[doc(hidden)]
    pub fn grouping_texts(&self) -> (String, String) {
        self.pages()
            .map(|p| (p.tracks().group_button_text(), p.tracks().status_text()))
            .unwrap_or_default()
    }
}
