// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The typed operations on the library: tracks, statistics and search.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use helikodu_model::library::TrackId;

use crate::catalog::{self, IdentityId, RootId, TrackFingerprint, TrackRecord, TrackWrite};
use crate::db::unix_time::{to_millis, to_unix_seconds};
use crate::db::{Database, DatabaseConfig, DbError, LocaleCollation};
use crate::search::{self, SearchRequest, SearchResults};
use crate::stats::{
    HistoryEntry, HistoryQuery, Listen, ListenOutcome, PlayRules, Rating, TrackStats, store,
};

/// The music library.
///
/// Reads run on a pool of read-only connections and see one snapshot each. Writes go to
/// the one writer thread, which batches them; a write method returns once its batch has
/// committed. The handle can be shared between threads.
#[derive(Debug)]
pub struct Library {
    db: Database,
}

impl Library {
    /// Opens the library, creating its database or bringing the schema up to date.
    ///
    /// Refuses a database written by a newer version ([`DbError::NewerSchema`]); such a
    /// database can still be opened with [`open_read_only`](Self::open_read_only).
    pub fn open(config: &DatabaseConfig) -> Result<Self, DbError> {
        Database::open(config).map(Self::from)
    }

    /// Opens an existing library for reading only, without migrating it. Writes fail
    /// with [`DbError::ReadOnly`].
    pub fn open_read_only(config: &DatabaseConfig) -> Result<Self, DbError> {
        Database::open_read_only(config).map(Self::from)
    }

    /// The schema version of the database.
    pub fn schema_version(&self) -> u32 {
        self.db.schema_version()
    }

    /// Switches the sort order to `collation` and rebuilds the indexes that use it.
    pub fn set_collation(&self, collation: LocaleCollation) -> Result<(), DbError> {
        self.db.set_collation(collation)
    }

    /// The database underneath, for direct SQL. Not a stable interface.
    #[doc(hidden)]
    pub fn database(&self) -> &Database {
        &self.db
    }

    fn write<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        self.db.writer()?.write(job)
    }

    // --- Catalogue -----------------------------------------------------------------

    /// Adds the library folder at `path`, or returns it if it is there already.
    pub fn add_root(&self, path: impl Into<PathBuf>) -> Result<RootId, DbError> {
        let path = path.into();
        self.write(move |c| catalog::write::add_root(c, &path))
    }

    /// Writes a track read from its file.
    ///
    /// - A track already at its path and sub-track number is updated.
    /// - A file that disappeared (see [`mark_file_missing`](Self::mark_file_missing))
    ///   and has the same size and quick hash is moved here, with its tracks.
    /// - A new track whose fingerprint matches an identity no track uses any more gets
    ///   that identity back, with its statistics.
    /// - Otherwise the track gets a new identity, rated from its tags if they have a
    ///   rating.
    ///
    /// The search indexes are updated in the same transaction.
    pub fn upsert_track(&self, record: TrackRecord) -> Result<TrackWrite, DbError> {
        check_file_path(&record.path)?;
        let now = to_unix_seconds(SystemTime::now());
        self.write(move |c| catalog::write::upsert_track(c, &record, now))
    }

    /// Writes several tracks in one transaction, like [`upsert_track`](Self::upsert_track).
    /// If one fails, none is written.
    pub fn upsert_tracks(&self, records: Vec<TrackRecord>) -> Result<Vec<TrackWrite>, DbError> {
        for record in &records {
            check_file_path(&record.path)?;
        }
        let now = to_unix_seconds(SystemTime::now());
        self.write(move |c| {
            records
                .iter()
                .map(|record| catalog::write::upsert_track(c, record, now))
                .collect()
        })
    }

    /// Moves the file at `from` to `to` inside the library folder `root`, as when a
    /// watched file is renamed. Its tracks keep their ids, identities and statistics.
    pub fn move_file(
        &self,
        root: RootId,
        from: impl Into<PathBuf>,
        to: impl Into<PathBuf>,
    ) -> Result<(), DbError> {
        let (from, to) = (from.into(), to.into());
        check_file_path(&to)?;
        let missing = from.clone();
        let moved = self.write(move |c| catalog::write::move_file(c, root, &from, &to))?;
        if moved {
            Ok(())
        } else {
            Err(DbError::UnknownPath(missing))
        }
    }

    /// Records that the file at `path` disappeared at `since`. Its tracks stay until the
    /// file is removed; if it reappears elsewhere first, it is moved there.
    pub fn mark_file_missing(
        &self,
        path: impl Into<PathBuf>,
        since: SystemTime,
    ) -> Result<(), DbError> {
        let path = path.into();
        let missing = path.clone();
        let since = to_unix_seconds(since);
        let found = self.write(move |c| catalog::write::mark_file_missing(c, &path, since))?;
        if found {
            Ok(())
        } else {
            Err(DbError::UnknownPath(missing))
        }
    }

    /// Removes the file at `path` and its tracks, and returns how many tracks it had.
    /// Their identities and statistics stay for when the tracks come back.
    pub fn remove_file(&self, path: impl Into<PathBuf>) -> Result<usize, DbError> {
        let path = path.into();
        self.write(move |c| catalog::write::remove_file(c, &path))
    }

    /// Removes `track`, and its file if no other track uses it. Its identity and
    /// statistics stay for when it comes back.
    pub fn remove_track(&self, track: TrackId) -> Result<(), DbError> {
        if self.write(move |c| catalog::write::remove_track(c, track))? {
            Ok(())
        } else {
            Err(DbError::UnknownTrack(track))
        }
    }

    /// The track at `path` with the sub-track number `sub_index` (0 for a whole file).
    pub fn track_at(&self, path: &Path, sub_index: u32) -> Result<Option<TrackId>, DbError> {
        self.db
            .read(|c| catalog::write::track_at(c, path, sub_index))
    }

    /// The identity of `track`.
    pub fn identity_of(&self, track: TrackId) -> Result<IdentityId, DbError> {
        self.db
            .read(|c| catalog::write::identity_of(c, track))?
            .ok_or(DbError::UnknownTrack(track))
    }

    /// The identities with `fingerprint`, newest first.
    pub fn identities_with(
        &self,
        fingerprint: &TrackFingerprint,
    ) -> Result<Vec<IdentityId>, DbError> {
        self.db
            .read(|c| catalog::write::identities_with(c, fingerprint))
    }

    // --- Search --------------------------------------------------------------------

    /// The tracks matching `request.text`, best first, one page of them.
    pub fn search(&self, request: &SearchRequest) -> Result<SearchResults, DbError> {
        self.db.read(|c| search::search(c, request))
    }

    /// Empties both search indexes and fills them again from the tracks.
    pub fn rebuild_search_index(&self) -> Result<(), DbError> {
        self.write(search::index::rebuild)
    }

    // --- Statistics ----------------------------------------------------------------

    /// Records a listen to `track` and returns what it counted as under `rules`.
    ///
    /// A play raises the play count and sets the first and last played times; a skip
    /// raises the skip count. Every listen that played some audio goes to the history.
    /// Tracks that remember where listening stopped store
    /// [`Listen::position`](crate::Listen::position), and forget it when they finish.
    pub fn record_listen(
        &self,
        track: TrackId,
        listen: Listen,
        rules: PlayRules,
    ) -> Result<ListenOutcome, DbError> {
        self.write(move |c| store::record_listen(c, track, &listen, &rules))?
            .ok_or(DbError::UnknownTrack(track))
    }

    /// The statistics of `track`'s identity.
    pub fn stats(&self, track: TrackId) -> Result<TrackStats, DbError> {
        self.db
            .read(|c| {
                catalog::write::identity_of(c, track)?
                    .map(|identity| store::stats_of(c, identity))
                    .transpose()
            })?
            .ok_or(DbError::UnknownTrack(track))
    }

    /// Rates `track`, or clears its rating with `None`.
    pub fn set_rating(&self, track: TrackId, rating: Option<Rating>) -> Result<(), DbError> {
        self.with_identity(track, move |c, identity| {
            store::set_rating(c, identity, rating)
        })
    }

    /// Stores where listening to `track` stopped, or forgets it with `None`.
    pub fn set_resume_position(
        &self,
        track: TrackId,
        position: Option<Duration>,
    ) -> Result<(), DbError> {
        self.with_identity(track, move |c, identity| {
            store::set_resume_ms(c, identity, position.map(to_millis))
        })
    }

    /// The listening history, newest first.
    pub fn history(&self, query: &HistoryQuery) -> Result<Vec<HistoryEntry>, DbError> {
        let identity = query
            .track
            .map(|track| self.identity_of(track))
            .transpose()?;
        self.db.read(|c| store::history(c, identity, query))
    }

    /// Runs `job` with the identity of `track` on the writer.
    fn with_identity<F>(&self, track: TrackId, job: F) -> Result<(), DbError>
    where
        F: FnOnce(&rusqlite::Connection, IdentityId) -> rusqlite::Result<()> + Send + 'static,
    {
        let found = self.write(move |c| {
            catalog::write::identity_of(c, track)?
                .map(|identity| job(c, identity))
                .transpose()
        })?;
        found.ok_or(DbError::UnknownTrack(track))
    }
}

impl From<Database> for Library {
    fn from(db: Database) -> Self {
        Self { db }
    }
}

/// Fails unless `path` names a file in a folder.
fn check_file_path(path: &Path) -> Result<(), DbError> {
    if path.parent().is_some() && path.file_name().is_some() {
        Ok(())
    } else {
        Err(DbError::InvalidPath(path.to_owned()))
    }
}
