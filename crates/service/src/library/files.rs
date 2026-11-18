// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! File maintenance for the front ends: the organizer with its journal, renaming
//! one file, and missing files. These calls block while files move, so the front
//! ends make them on threads of their own and announce the change with
//! [`LibraryService::announce_change`] when they are done.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use helikodu_library::{
    CancelToken, DbError, FileId, FileRunId, FileRunSummary, FoundFile, MissingFile, MoveRequest,
    OrganizeOptions, PlannedMove, Progress, RelinkOutcome, RunReport, SavedPattern,
};
use helikodu_model::library::TrackId;

use super::service::{LibraryEvent, LibraryService};
use super::{LibraryError, MusicLibrary};
use crate::Event;

impl MusicLibrary {
    /// Previews organizing the files of `tracks` (every file when `None`).
    pub fn plan_organize(
        &self,
        tracks: Option<&[TrackId]>,
        options: &OrganizeOptions,
    ) -> Result<Vec<PlannedMove>, LibraryError> {
        Ok(self.inner().plan_organize(tracks, options)?)
    }

    /// Moves `moves` as one run of the journal (see
    /// [`Library::organize`](helikodu_library::Library::organize)).
    pub fn organize(
        &self,
        moves: &[MoveRequest],
        pattern: Option<&str>,
        companions: bool,
        progress: Progress<'_>,
        cancel: &CancelToken,
    ) -> Result<RunReport, LibraryError> {
        Ok(self
            .inner()
            .organize(moves, pattern, companions, progress, cancel)?)
    }

    /// Renames the file of `track` to `name` in its folder, as a run of the
    /// journal that can be undone.
    pub fn rename_track_file(
        &self,
        track: TrackId,
        name: &OsStr,
    ) -> Result<RunReport, LibraryError> {
        let details = self
            .inner()
            .track_details(track)?
            .ok_or(LibraryError::Database(DbError::UnknownTrack(track)))?;
        let path = details.path.clone();
        let file = self
            .inner()
            .file_of_path(&path)?
            .ok_or_else(|| LibraryError::Database(DbError::UnknownPath(path.clone())))?;
        Ok(self.inner().rename_file(file, path, name)?)
    }

    /// The path of the file of `track`, if the library has it.
    pub fn track_file_path(&self, track: TrackId) -> Result<Option<PathBuf>, LibraryError> {
        Ok(self.inner().track_details(track)?.map(|d| d.path))
    }

    /// The library file at `path`, if there is one.
    pub fn file_of_path(&self, path: &Path) -> Result<Option<FileId>, LibraryError> {
        Ok(self.inner().file_of_path(path)?)
    }

    /// The newest run of the journal.
    pub fn last_file_run(&self) -> Result<Option<FileRunSummary>, LibraryError> {
        Ok(self.inner().last_file_run()?)
    }

    /// Undoes the newest run.
    pub fn undo_file_run(
        &self,
        run: FileRunId,
        progress: Progress<'_>,
    ) -> Result<RunReport, LibraryError> {
        Ok(self.inner().undo_file_run(run, progress)?)
    }

    /// Does the newest run again after an undo.
    pub fn redo_file_run(
        &self,
        run: FileRunId,
        progress: Progress<'_>,
    ) -> Result<RunReport, LibraryError> {
        Ok(self.inner().redo_file_run(run, progress)?)
    }

    /// Removes the runs older than `days` days from the journal.
    pub fn prune_file_runs(&self, days: u32) -> Result<usize, LibraryError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        Ok(self
            .inner()
            .prune_file_runs(now - i64::from(days) * 86_400)?)
    }

    /// The organizer's saved patterns.
    pub fn naming_patterns(&self) -> Result<Vec<SavedPattern>, LibraryError> {
        Ok(self.inner().naming_patterns()?)
    }

    /// Saves a pattern (a new one without `id`); returns its id.
    pub fn save_naming_pattern(
        &self,
        id: Option<i64>,
        name: &str,
        pattern: &str,
    ) -> Result<i64, LibraryError> {
        Ok(self.inner().save_naming_pattern(id, name, pattern)?)
    }

    /// Removes a saved pattern.
    pub fn delete_naming_pattern(&self, id: i64) -> Result<(), LibraryError> {
        Ok(self.inner().delete_naming_pattern(id)?)
    }

    /// The missing files, leaving out drives that are not connected.
    pub fn missing_files(&self) -> Result<Vec<MissingFile>, LibraryError> {
        Ok(self.inner().missing_files()?)
    }

    /// Looks for the missing `files` in `folder` (every library folder when
    /// `None`) and at their own places.
    pub fn find_missing(
        &self,
        files: &[FileId],
        folder: Option<&Path>,
        cancel: &CancelToken,
    ) -> Result<Vec<FoundFile>, LibraryError> {
        Ok(self.inner().find_missing(files, folder, cancel)?)
    }

    /// Relinks the missing files that were found.
    pub fn relink_missing(&self, found: &[FoundFile]) -> Result<Vec<RelinkOutcome>, LibraryError> {
        Ok(self.inner().relink_missing(found)?)
    }

    /// Removes missing files from the library.
    pub fn remove_missing(&self, files: &[FileId]) -> Result<usize, LibraryError> {
        Ok(self.inner().remove_missing(files)?)
    }

    /// The files of `tracks` (every library file when `None`) that are present,
    /// for a check of the files.
    pub fn present_files(&self, tracks: Option<&[TrackId]>) -> Result<Vec<PathBuf>, LibraryError> {
        Ok(self.inner().present_files(tracks)?)
    }
}

impl LibraryService {
    /// Tells every list that the library changed: after files were moved, renamed,
    /// relinked or removed by the calls above.
    pub fn announce_change(&self) {
        self.bus.publish(&Event::Library(LibraryEvent::Changed));
    }
}
