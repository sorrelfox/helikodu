// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lists of tracks in groups, for a track table grouped by a field, a label, a
//! computed field or an expression: the groups in the order of their values,
//! the tracks without a value first.

use helikodu_library::{GroupBy, TrackGroup};
use helikodu_model::library::TrackId;

use super::{LibraryError, MusicLibrary};

impl MusicLibrary {
    /// `tracks` (a list in the order its columns sort it) in groups by `by`:
    /// the groups in the order of their values, the tracks without a value
    /// first, and the tracks of each group in the order of `tracks`.
    pub fn group_tracks(
        &self,
        by: &GroupBy,
        tracks: &[TrackId],
    ) -> Result<Vec<TrackGroup>, LibraryError> {
        Ok(self.inner().group_tracks(by, tracks)?)
    }
}
