// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Release years, media kinds, resume positions and the sections of a file.

use std::time::Duration;

use helikodu_library::{FileSection, TrackKind, YearQuery, YearSummary};
use helikodu_model::library::TrackId;

use super::{LibraryError, MusicLibrary};

impl MusicLibrary {
    /// Every release year with albums or tracks, newest first, then the tracks
    /// without a year.
    pub fn years(&self) -> Result<Vec<YearSummary>, LibraryError> {
        Ok(self.library.years(&YearQuery::default())?)
    }

    /// Gives `tracks` the media kind `kind`. The choice survives rescans.
    pub fn set_kind(&self, tracks: &[TrackId], kind: TrackKind) -> Result<(), LibraryError> {
        Ok(self.library.set_kind(tracks, kind)?)
    }

    /// The chapters or CUE sheet tracks of the file of `track`, in file order, when
    /// it is one of them; empty otherwise.
    pub fn file_sections(&self, track: TrackId) -> Result<Vec<FileSection>, LibraryError> {
        Ok(self.library.file_sections(track)?)
    }

    /// Stores where listening to `track` stopped, or forgets it with `None` (to
    /// start an audiobook again).
    pub fn set_resume_position(
        &self,
        track: TrackId,
        position: Option<Duration>,
    ) -> Result<(), LibraryError> {
        Ok(self.library.set_resume_position(track, position)?)
    }
}
