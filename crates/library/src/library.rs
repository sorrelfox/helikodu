// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The typed operations on the library: tracks, statistics and search.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime};

use helikodu_model::library::TrackId;

use std::collections::HashSet;

use helikodu_meta::covers::CoverHash;

use crate::catalog::{self, IdentityId, RootId, TrackFingerprint, TrackRecord, TrackWrite};
use crate::catalog::{
    AlbumDetail, AlbumId, AlbumQuery, AlbumSummary, ArtistQuery, ArtistSummary, GenreQuery,
    GenreSummary, Paged, RootState, TrackQuery, TrackSummary,
};
use crate::catalog::{CoverInfo, CoverLocation, Page};
use crate::db::unix_time::{to_millis, to_unix_seconds};
use crate::db::{Database, DatabaseConfig, DbError, LocaleCollation};
use crate::playlists::{
    self, CleanupCounts, ExportOptions, ExportReport, FoundReport, ImportOptions, ImportReport,
    Pin, PinTarget, PlaylistError, PlaylistId, PlaylistItem, PlaylistNode, PlaylistSummary,
};
use crate::scan::{self, CancelToken, ScanError, ScanEvent, ScanOptions, ScanSummary};
use crate::search::{self, SearchRequest, SearchResults};
use crate::stats::{
    HistoryEntry, HistoryQuery, HistoryTotals, Listen, ListenOutcome, PlayRules, Rating,
    TrackStats, store,
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
    /// The albums they affect are regrouped once, after all of them are written. If one
    /// fails, none is written.
    pub fn upsert_tracks(&self, records: Vec<TrackRecord>) -> Result<Vec<TrackWrite>, DbError> {
        for record in &records {
            check_file_path(&record.path)?;
        }
        let now = to_unix_seconds(SystemTime::now());
        self.write(move |c| catalog::write::upsert_tracks(c, &records, now))
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

    /// Moves every file in the folder `from` and below it to the same place below `to`
    /// inside the library folder `root`, as when a watched folder is renamed. Their
    /// tracks keep their ids, identities and statistics. Returns the number of files
    /// moved.
    pub fn move_folder(
        &self,
        root: RootId,
        from: impl Into<PathBuf>,
        to: impl Into<PathBuf>,
    ) -> Result<usize, DbError> {
        let (from, to) = (from.into(), to.into());
        self.write(move |c| catalog::write::move_folder(c, root, &from, &to))
    }

    /// Removes the files in the library folder `root` that have been missing since
    /// `missing_before` or longer, with their tracks: the end of the grace period for
    /// missing files. Their identities and statistics stay for when the tracks come
    /// back. Returns the number of files removed.
    pub fn remove_missing_files(
        &self,
        root: RootId,
        missing_before: SystemTime,
    ) -> Result<usize, DbError> {
        let cutoff = to_unix_seconds(missing_before);
        self.write(move |c| scan::store::remove_missing_files(c, root, cutoff))
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

    // --- Browsing ------------------------------------------------------------------

    /// One page of albums: all of them, an artist's, the ones an artist appears on,
    /// compilations or a genre's, in the order of `query.sort`. Titles and names sort
    /// in the alphabet order of the interface language.
    pub fn albums(&self, query: &AlbumQuery) -> Result<Paged<AlbumSummary>, DbError> {
        self.db.read(|c| catalog::albums(c, query))
    }

    /// The album `album` with its artists, genres, discs and tracks in order; `None`
    /// if there is no such album or all its tracks are in masked library folders.
    pub fn album(&self, album: AlbumId) -> Result<Option<AlbumDetail>, DbError> {
        self.db.read(|c| catalog::album(c, album, false))
    }

    /// Like [`album`](Self::album), with the tracks in masked library folders.
    pub fn album_including_masked(&self, album: AlbumId) -> Result<Option<AlbumDetail>, DbError> {
        self.db.read(|c| catalog::album(c, album, true))
    }

    /// One page of the people in a role (artists, album artists, composers,
    /// conductors, performers, …), by sort name.
    pub fn artists(&self, query: &ArtistQuery) -> Result<Paged<ArtistSummary>, DbError> {
        self.db.read(|c| catalog::artists(c, query))
    }

    /// One page of the genres, by name.
    pub fn genres(&self, query: &GenreQuery) -> Result<Paged<GenreSummary>, DbError> {
        self.db.read(|c| catalog::genres(c, query))
    }

    /// One page of the tracks of an album, a person or a genre.
    pub fn tracks(&self, query: &TrackQuery) -> Result<Paged<TrackSummary>, DbError> {
        self.db.read(|c| catalog::tracks(c, query))
    }

    /// Every track of `query` in its order (the page is not used), as its id and
    /// album: one sorted query, for long lists that fetch their rows later.
    pub fn track_order(
        &self,
        query: &TrackQuery,
    ) -> Result<Vec<(TrackId, Option<AlbumId>)>, DbError> {
        self.db.read(|c| catalog::track_order(c, query))
    }

    // --- Covers ----------------------------------------------------------------------

    /// The cover of `album`: the embedded cover most of its tracks have, or else the
    /// folder image most of them have.
    pub fn album_cover(&self, album: AlbumId) -> Result<Option<CoverInfo>, DbError> {
        self.db.read(|c| catalog::covers::album_cover(c, album))
    }

    /// The cover of `track`: its embedded picture, else its folder's image, else its
    /// album's cover.
    pub fn track_cover(&self, track: TrackId) -> Result<Option<CoverInfo>, DbError> {
        self.db.read(|c| catalog::covers::track_cover(c, track))
    }

    /// Where the bytes of the cover with `hash` can be read, best first; empty when
    /// the library has no such cover.
    pub fn cover_locations(&self, hash: &CoverHash) -> Result<Vec<CoverLocation>, DbError> {
        self.db.read(|c| {
            let mut out = catalog::covers::cover_locations(c, hash)?;
            crate::artist_photos::locations(c, hash, &mut out)?;
            Ok(out)
        })
    }

    /// The hashes of every cover and artist photo in the library, for removing the
    /// thumbnails of the others ([`ThumbnailCache::remove_unreferenced`](crate::ThumbnailCache::remove_unreferenced)).
    pub fn cover_hashes(&self) -> Result<HashSet<CoverHash>, DbError> {
        self.db.read(|c| {
            let mut out = catalog::covers::cover_hashes(c)?;
            crate::artist_photos::hashes(c, &mut out)?;
            Ok(out)
        })
    }

    // --- Library folders -------------------------------------------------------------

    /// Every library folder with its state, in path order.
    pub fn root_states(&self) -> Result<Vec<RootState>, DbError> {
        self.db.read(catalog::roots::root_states)
    }

    /// Masks the library folder `root`, as the watcher does when it is gone, or
    /// unmasks it. While it is masked (and [`RootState::mask_when_missing`] is set),
    /// the browsing queries leave out its tracks.
    pub fn set_root_masked(&self, root: RootId, masked: bool) -> Result<(), DbError> {
        let now = to_unix_seconds(SystemTime::now());
        if self.write(move |c| catalog::roots::set_masked(c, root, masked, now))? {
            Ok(())
        } else {
            Err(DbError::UnknownRoot(root))
        }
    }

    /// Sets whether the tracks of `root` are hidden while it is masked (the default)
    /// or still shown.
    pub fn set_mask_when_missing(&self, root: RootId, hide: bool) -> Result<(), DbError> {
        if self.write(move |c| catalog::roots::set_mask_when_missing(c, root, hide))? {
            Ok(())
        } else {
            Err(DbError::UnknownRoot(root))
        }
    }

    // --- Playlists --------------------------------------------------------------------

    /// Creates an empty static playlist named `name`, after the others.
    pub fn create_playlist(&self, name: &str) -> Result<PlaylistId, PlaylistError> {
        let name = playlists::check_name(name)?;
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::create(c, &name, now))
    }

    /// Renames `playlist`.
    pub fn rename_playlist(&self, playlist: PlaylistId, name: &str) -> Result<(), PlaylistError> {
        let name = playlists::check_name(name)?;
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::rename(c, playlist, &name, now))
    }

    /// Deletes `playlist` with its items.
    pub fn delete_playlist(&self, playlist: PlaylistId) -> Result<(), PlaylistError> {
        self.playlist_write(move |c| playlists::delete(c, playlist))
    }

    /// Copies the static playlist `playlist` into a new one named `name`.
    pub fn duplicate_playlist(
        &self,
        playlist: PlaylistId,
        name: &str,
    ) -> Result<PlaylistId, PlaylistError> {
        let name = playlists::check_name(name)?;
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::duplicate(c, playlist, &name, now))
    }

    /// Adds `tracks` at the end of the static playlist `playlist`, and returns the
    /// position of the first. Fails, adding nothing, if a track is not in the library.
    pub fn add_to_playlist(
        &self,
        playlist: PlaylistId,
        tracks: Vec<TrackId>,
    ) -> Result<usize, PlaylistError> {
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::append_tracks(c, playlist, &tracks, now))
    }

    /// Inserts `tracks` at `position` (at most the playlist's length) of the static
    /// playlist `playlist`. Fails, adding nothing, if a track is not in the library.
    pub fn insert_into_playlist(
        &self,
        playlist: PlaylistId,
        position: usize,
        tracks: Vec<TrackId>,
    ) -> Result<(), PlaylistError> {
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::insert_tracks(c, playlist, position, &tracks, now))
    }

    /// Moves the items at `positions` of the static playlist `playlist`, keeping their
    /// order, so that they stand together before the item now at `to` (or at the end
    /// when `to` is the playlist's length), as when dragging a selection. Returns the
    /// new position of the first.
    pub fn move_playlist_items(
        &self,
        playlist: PlaylistId,
        positions: Vec<usize>,
        to: usize,
    ) -> Result<usize, PlaylistError> {
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::move_items(c, playlist, &positions, to, now))
    }

    /// Removes the items at `positions` from the static playlist `playlist`. Returns how
    /// many were removed.
    pub fn remove_from_playlist(
        &self,
        playlist: PlaylistId,
        positions: Vec<usize>,
    ) -> Result<usize, PlaylistError> {
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::remove_items(c, playlist, &positions, now))
    }

    /// One page of the static playlists that are not in a folder, in their order.
    pub fn playlists(&self, page: Page) -> Result<crate::Paged<PlaylistSummary>, DbError> {
        let now = to_unix_seconds(SystemTime::now());
        self.db.read(|c| playlists::playlists(c, page, now))
    }

    /// The playlist or folder `playlist`, if there is one. The count and length of a
    /// smart playlist are those of the tracks its query selects now.
    pub fn playlist(&self, playlist: PlaylistId) -> Result<Option<PlaylistSummary>, DbError> {
        let now = to_unix_seconds(SystemTime::now());
        self.db.read(|c| playlists::playlist(c, playlist, now))
    }

    /// Every playlist and folder, each folder followed by what is in it, in the order
    /// the sidebar shows them.
    pub fn playlist_tree(&self) -> Result<Vec<PlaylistNode>, DbError> {
        let now = to_unix_seconds(SystemTime::now());
        self.db.read(|c| playlists::playlist_tree(c, now))
    }

    /// Creates an empty folder named `name` in `parent` (at the top for `None`),
    /// after what is there.
    pub fn create_playlist_folder(
        &self,
        name: &str,
        parent: Option<PlaylistId>,
    ) -> Result<PlaylistId, PlaylistError> {
        let name = playlists::check_name(name)?;
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::create_folder(c, &name, parent, now))
    }

    /// Moves the playlist or folder `playlist` into the folder `parent` (the top for
    /// `None`), at `position` among what is there, or after it. A folder cannot go
    /// into itself or a folder inside it.
    pub fn move_playlist(
        &self,
        playlist: PlaylistId,
        parent: Option<PlaylistId>,
        position: Option<usize>,
    ) -> Result<(), PlaylistError> {
        self.playlist_write(move |c| playlists::move_to(c, playlist, parent, position))
    }

    /// Locks the playlist against changes of its items and query, or unlocks it.
    pub fn set_playlist_locked(
        &self,
        playlist: PlaylistId,
        locked: bool,
    ) -> Result<(), PlaylistError> {
        self.playlist_write(move |c| playlists::set_locked(c, playlist, locked))
    }

    /// Shows the playlist in groups by album, or not.
    pub fn set_playlist_grouped(
        &self,
        playlist: PlaylistId,
        grouped: bool,
    ) -> Result<(), PlaylistError> {
        self.playlist_write(move |c| playlists::set_grouped(c, playlist, grouped))
    }

    /// Undoes the newest edit of the playlist's items, name or query. Returns whether
    /// there was one. A locked playlist refuses.
    pub fn undo_playlist(&self, playlist: PlaylistId) -> Result<bool, PlaylistError> {
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::history_step(c, playlist, false, now))
    }

    /// Does the newest undone edit of the playlist again. Returns whether there was
    /// one. A locked playlist refuses.
    pub fn redo_playlist(&self, playlist: PlaylistId) -> Result<bool, PlaylistError> {
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::history_step(c, playlist, true, now))
    }

    /// What removing the repeated items and the items that cannot play would remove
    /// from the static playlist `playlist`.
    pub fn playlist_cleanup_counts(
        &self,
        playlist: PlaylistId,
    ) -> Result<CleanupCounts, PlaylistError> {
        self.db
            .read(|c| playlists::cleanup_counts(c, playlist))
            .map_err(PlaylistError::from_job)
    }

    /// Removes the items of the static playlist `playlist` that repeat an earlier
    /// one (the same track, or the same path or URL). Returns how many went.
    pub fn remove_playlist_duplicates(&self, playlist: PlaylistId) -> Result<usize, PlaylistError> {
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::remove_duplicates(c, playlist, now))
    }

    /// Removes the items of the static playlist `playlist` that cannot play: entries
    /// that name no track, and tracks whose file is missing. Returns how many went.
    pub fn remove_missing_from_playlist(
        &self,
        playlist: PlaylistId,
    ) -> Result<usize, PlaylistError> {
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::remove_missing(c, playlist, now))
    }

    /// Puts the items of the static playlist `playlist` in a random order that `seed`
    /// chooses, for good.
    pub fn shuffle_playlist(&self, playlist: PlaylistId, seed: u64) -> Result<(), PlaylistError> {
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::shuffle(c, playlist, seed, now))
    }

    /// Creates a smart playlist named `name` whose tracks `query` selects (its sort
    /// keys and limit included), in the folder `parent` or at the top.
    pub fn create_smart_playlist(
        &self,
        name: &str,
        query: &helikodu_model::query::Query,
        parent: Option<PlaylistId>,
    ) -> Result<PlaylistId, PlaylistError> {
        let name = playlists::check_name(name)?;
        if let Some((_, problem)) = query.problem() {
            return Err(PlaylistError::BadQuery(problem));
        }
        let text = crate::query::print_query(query);
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::create_smart(c, &name, &text, parent, now))
    }

    /// Replaces the query of the smart playlist `playlist`. The change can be undone;
    /// a locked playlist refuses.
    pub fn set_smart_playlist_query(
        &self,
        playlist: PlaylistId,
        query: &helikodu_model::query::Query,
    ) -> Result<(), PlaylistError> {
        if let Some((_, problem)) = query.problem() {
            return Err(PlaylistError::BadQuery(problem));
        }
        let text = crate::query::print_query(query);
        let now = to_unix_seconds(SystemTime::now());
        self.playlist_write(move |c| playlists::set_query(c, playlist, &text, now))
    }

    /// The query of the smart playlist `playlist`.
    pub fn smart_playlist_query(
        &self,
        playlist: PlaylistId,
    ) -> Result<helikodu_model::query::Query, PlaylistError> {
        self.db
            .read(|c| playlists::smart_query(c, playlist))
            .map_err(PlaylistError::from_job)
    }

    /// One page of the tracks the smart playlist `playlist` selects now, in its
    /// order, with how many it selects in all.
    pub fn smart_playlist_tracks(
        &self,
        playlist: PlaylistId,
        page: Page,
    ) -> Result<crate::Paged<crate::TrackSummary>, PlaylistError> {
        let now = to_unix_seconds(SystemTime::now());
        self.db
            .read(|c| playlists::smart_tracks(c, playlist, page, now))
            .map_err(PlaylistError::from_job)
    }

    /// When each of `tracks` came into the library, by track; a track that is not in
    /// the library is left out.
    pub fn added_times(
        &self,
        tracks: &[TrackId],
    ) -> Result<std::collections::HashMap<TrackId, SystemTime>, DbError> {
        self.db.read(|c| playlists::added_times(c, tracks))
    }

    /// Pins an album, an artist or a playlist to the sidebar, after the other pins.
    pub fn pin(&self, target: PinTarget) -> Result<(), DbError> {
        self.write(move |c| playlists::pin(c, target))
    }

    /// Unpins it. Returns whether it was pinned.
    pub fn unpin(&self, target: PinTarget) -> Result<bool, DbError> {
        self.write(move |c| playlists::unpin(c, target))
    }

    /// The pins, in their order. A pinned album, artist or playlist that has gone is
    /// left out; an album that split or merged with another during a scan may have
    /// a new id and so lose its pin.
    pub fn pins(&self) -> Result<Vec<Pin>, DbError> {
        self.db.read(playlists::pins)
    }

    /// Picks up the playlist files (M3U, M3U8, PLS, XSPF) in the library folders that
    /// are connected, leaving out hidden folders and what `exclude` names: a file
    /// not seen before becomes a static playlist that remembers it; a file that
    /// changed since replaces the items of its playlist (an edit that can be undone)
    /// unless the playlist is locked.
    pub fn import_found_playlists(
        &self,
        exclude: &[crate::ExcludeRule],
    ) -> Result<FoundReport, DbError> {
        use helikodu_meta::playlists::{ParseOptions, read_playlist};
        let roots: Vec<PathBuf> = self
            .root_states()?
            .into_iter()
            .filter(|root| !root.masked)
            .map(|root| root.path)
            .collect();
        let files = playlists::find_playlist_files(&roots, exclude);
        let known = self.db.read(playlists::imported_sources)?;
        let mut report = FoundReport::default();
        for file in files {
            let existing = known.get(&file.path);
            match existing {
                Some((_, Some(mtime), _)) if *mtime == file.mtime_ns => continue,
                Some((_, _, true)) => continue,
                _ => {}
            }
            let parsed = match read_playlist(&file.path, &ParseOptions::default()) {
                Ok(parsed) => parsed,
                Err(error) => {
                    report.failed.push((file.path.clone(), error.to_string()));
                    continue;
                }
            };
            let name = playlists::imported_name(None, parsed.playlist.title.as_deref(), &file.path);
            let (items, _) = self
                .db
                .read(|c| playlists::match_entries(c, &parsed, name.clone()))?;
            let existing = existing.map(|(id, _, _)| *id);
            let now = to_unix_seconds(SystemTime::now());
            let written =
                self.write(move |c| playlists::store_found(c, &file, &name, &items, existing, now));
            match written {
                Ok(_) if existing.is_some() => report.refreshed += 1,
                Ok(_) => report.imported += 1,
                Err(error) => return Err(error),
            }
        }
        Ok(report)
    }

    /// One page of the items of the static playlist `playlist`, in order. An item whose
    /// track is not in the library is [`ItemContent::Dead`](crate::ItemContent::Dead).
    pub fn playlist_items(
        &self,
        playlist: PlaylistId,
        page: Page,
    ) -> Result<crate::Paged<PlaylistItem>, PlaylistError> {
        self.db
            .read(|c| playlists::item_page(c, playlist, page))
            .map_err(PlaylistError::from_job)
    }

    /// The dead items of the static playlist `playlist`, in order.
    pub fn dead_playlist_items(
        &self,
        playlist: PlaylistId,
    ) -> Result<Vec<PlaylistItem>, PlaylistError> {
        self.db
            .read(|c| playlists::dead_items(c, playlist))
            .map_err(PlaylistError::from_job)
    }

    /// Imports the playlist file at `path` (M3U, M3U8, PLS or XSPF, by its extension or
    /// its contents) as a new static playlist, after the others.
    ///
    /// Each entry is matched to a track of the library by its path, then by the end of
    /// its path, then by its title, artist and length. An entry that matches nothing is
    /// kept as a dead item; a URL is kept as an unsupported dead item and never
    /// fetched; an entry naming another playlist is skipped, not followed. The
    /// playlist is written in one transaction.
    pub fn import_playlist(
        &self,
        path: &Path,
        options: &ImportOptions,
    ) -> Result<ImportReport, PlaylistError> {
        use helikodu_meta::playlists::{ParseOptions, read_playlist};
        let parsed = read_playlist(
            path,
            &ParseOptions {
                base_dir: None,
                legacy_charset: options.legacy_charset.clone(),
                limits: options.limits,
            },
        )?;
        let name = playlists::imported_name(
            options.name.as_deref(),
            parsed.playlist.title.as_deref(),
            path,
        );
        // Matching reads the library on a reader, so the writer is not held up.
        let (items, mut report) = self
            .db
            .read(|c| playlists::match_entries(c, &parsed, name.clone()))?;
        let now = to_unix_seconds(SystemTime::now());
        report.playlist =
            self.playlist_write(move |c| playlists::insert_imported(c, &name, &items, now))?;
        Ok(report)
    }

    /// Exports the static playlist `playlist` into the file at `path`, replacing it
    /// atomically: a crash leaves the old file or the new one.
    pub fn export_playlist(
        &self,
        playlist: PlaylistId,
        path: &Path,
        options: ExportOptions,
    ) -> Result<ExportReport, PlaylistError> {
        use helikodu_meta::playlists::{WriteOptions, write_playlist};
        let (contents, dead) = self
            .db
            .read(|c| playlists::export_rows(c, playlist, options.skip_dead))
            .map_err(PlaylistError::from_job)?;
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        let written = write_playlist(
            &contents,
            options.kind,
            &WriteOptions {
                path_style: options.path_style,
                base_dir: absolute.parent().map(Path::to_owned),
            },
        );
        playlists::write_atomically(path, &written.bytes).map_err(|source| {
            PlaylistError::Write {
                path: path.to_owned(),
                source,
            }
        })?;
        let left_out = written
            .warnings
            .iter()
            .filter(|w| w.kind == helikodu_meta::playlists::WriteWarningKind::Unrepresentable)
            .count();
        Ok(ExportReport {
            entries: contents.entries.len().saturating_sub(left_out),
            dead,
            warnings: written.warnings,
        })
    }

    /// Runs a playlist edit on the writer; a refusal rolls it back.
    fn playlist_write<T, F>(&self, job: F) -> Result<T, PlaylistError>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        self.write(job).map_err(PlaylistError::from_job)
    }

    // --- Scanning ------------------------------------------------------------------

    /// Scans the library folders of `options` and waits until the scan ends.
    ///
    /// Each folder is added to the library if needed, walked, and compared with the
    /// database; new and changed files are read by a pool of `helikodu-scan-worker`
    /// processes and written in batches, and files that disappeared are marked
    /// missing. Events go to `events` while the scan runs. A file that cannot be read
    /// (including one that crashes or hangs a worker) is reported and recorded, and
    /// the scan goes on; only a database failure or a worker that cannot be started
    /// at all ends it with an error. [`ScanHandle`](crate::ScanHandle) runs a scan on
    /// its own thread.
    pub fn scan(
        &self,
        options: &ScanOptions,
        events: Option<&Sender<ScanEvent>>,
        cancel: &CancelToken,
    ) -> Result<ScanSummary, ScanError> {
        scan::run(&self.db, options, events, cancel)
    }

    /// The files that could not be read, with the reason, in path order.
    pub fn unreadable_files(&self) -> Result<Vec<(PathBuf, String)>, DbError> {
        self.db.read(scan::store::unreadable_files)
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

    /// The totals of the whole listening history.
    pub fn history_totals(&self) -> Result<HistoryTotals, DbError> {
        self.db.read(store::history_totals)
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
