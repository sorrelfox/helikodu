// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Imports from other players and of exported user data, statistics carried to a
//! replaced file's replacement, and play counts and ratings written into the
//! files, for the front ends.

use std::path::{Path, PathBuf};

use helikodu_library::interchange::{
    CarryCandidate, ForeignLibrary, ImportPlan, ImportRun, ImportRunId, ImportSource, MergeOptions,
    StatsToFiles, StatsWriteReport, UserDataExport, UserDataFile,
};
use helikodu_library::{CancelToken, TagWriteSettings};
use helikodu_model::library::TrackId;
use helikodu_model::settings::{AdvancedSettings, LibrarySettings};

use super::{LibraryError, MusicLibrary, scan_options};

/// The files written in one batch of [`MusicLibrary::write_stats_to_files`].
pub const STATS_BATCH: usize = 200;

/// Which statistics the `[advanced]` settings send into the files.
pub fn stats_to_files(advanced: &AdvancedSettings) -> StatsToFiles {
    StatsToFiles {
        ratings: advanced.write_ratings_to_tags,
        play_counts: advanced.write_play_counts_to_tags,
    }
}

impl MusicLibrary {
    /// Reads another player's library (changing nothing there) and works out what
    /// importing it would do.
    pub fn plan_import(
        &self,
        source: &ImportSource,
        options: &MergeOptions,
    ) -> Result<ImportPlan, LibraryError> {
        Ok(self.inner().plan_import(source, options)?)
    }

    /// Works out what importing `library`, read from `source` once with
    /// `interchange::read_source`, would do.
    pub fn plan_source_import(
        &self,
        source: &ImportSource,
        library: &ForeignLibrary,
        options: &MergeOptions,
    ) -> Result<ImportPlan, LibraryError> {
        Ok(self.inner().plan_source_import(source, library, options)?)
    }

    /// Reads an export of the user data, changing nothing.
    pub fn read_user_data(path: &Path) -> Result<UserDataFile, LibraryError> {
        Ok(helikodu_library::Library::read_user_data(path)?)
    }

    /// Works out what importing an export of the user data, read with
    /// [`read_user_data`](Self::read_user_data), would do.
    pub fn plan_user_data(
        &self,
        file: &UserDataFile,
        options: &MergeOptions,
    ) -> Result<ImportPlan, LibraryError> {
        Ok(self.inner().plan_user_data(file, options)?)
    }

    /// Reads an export of the user data and works out what importing it would do.
    pub fn plan_user_data_import(
        &self,
        path: &Path,
        options: &MergeOptions,
    ) -> Result<ImportPlan, LibraryError> {
        Ok(self.inner().plan_user_data_import(path, options)?)
    }

    /// Carries out an import plan in one transaction, as a run that can be undone.
    pub fn apply_import(&self, plan: &ImportPlan) -> Result<ImportRunId, LibraryError> {
        Ok(self.inner().apply_import(plan)?)
    }

    /// Undoes the newest import run.
    pub fn undo_import(&self, run: ImportRunId) -> Result<(), LibraryError> {
        Ok(self.inner().undo_import(run)?)
    }

    /// Every import run, newest first.
    pub fn import_runs(&self) -> Result<Vec<ImportRun>, LibraryError> {
        Ok(self.inner().import_runs()?)
    }

    /// Writes the user data (ratings, play counts, history, playlists, Favourites,
    /// saved searches) to `path` as JSON.
    pub fn export_user_data_to(&self, path: &Path) -> Result<UserDataExport, LibraryError> {
        Ok(self.inner().export_user_data_to(path)?)
    }

    /// The missing tracks whose statistics a track of the same recording could
    /// take over.
    pub fn carry_candidates(&self) -> Result<Vec<CarryCandidate>, LibraryError> {
        Ok(self.inner().carry_candidates()?)
    }

    /// Moves the statistics, rating, history and playlist entries of `old` to
    /// `new`, as an import run that can be undone.
    pub fn carry_stats(&self, old: TrackId, new: TrackId) -> Result<ImportRunId, LibraryError> {
        Ok(self.inner().carry_stats(old, new)?)
    }

    /// Carries the statistics of each missing track in `pairs` to its
    /// replacement, as one import run that can be undone.
    pub fn carry_stats_of(
        &self,
        pairs: &[(TrackId, TrackId)],
    ) -> Result<ImportRunId, LibraryError> {
        Ok(self.inner().carry_stats_of(pairs)?)
    }

    /// How many files wait for their play count or rating to be written, under
    /// the `[advanced]` settings.
    pub fn pending_stats_writes(&self, advanced: &AdvancedSettings) -> Result<usize, LibraryError> {
        Ok(self
            .inner()
            .pending_stats_writes(stats_to_files(advanced))?)
    }

    /// Writes the play counts and ratings that changed into up to `limit` files,
    /// as the `[advanced]` settings ask, and reads them again; the files in `skip`
    /// (such as the one playing) wait for a later batch. Nothing is written when
    /// both settings are off. Blocks: call it when the application is idle or
    /// before it quits, off a window's thread.
    pub fn write_stats_to_files(
        &self,
        library: &LibrarySettings,
        advanced: &AdvancedSettings,
        worker_program: Option<&Path>,
        limit: usize,
        skip: &[PathBuf],
        cancel: &CancelToken,
    ) -> Result<StatsWriteReport, LibraryError> {
        let what = stats_to_files(advanced);
        if !what.any() {
            return Ok(StatsWriteReport::default());
        }
        let scan = scan_options(library, worker_program)?;
        let settings = TagWriteSettings::from_settings(advanced);
        Ok(self
            .inner()
            .write_stats_to_files(what, &settings, &scan, limit, skip, cancel)?)
    }
}
