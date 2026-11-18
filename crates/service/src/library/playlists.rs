// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Playlist folders, smart playlists, undo and redo, the lock, tidying, pins and the
//! playlist files found in the library folders, for the front ends.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use helikodu_library::{ExcludeRule, QueryRequest};
use helikodu_model::query::Query;
use helikodu_model::settings::LibrarySettings;

pub use helikodu_library::{
    CleanupCounts, FoundReport, Pin, PinTarget, PlaylistNode, PlaylistType,
};

use super::{
    LibraryError, LibraryEvent, LibraryService, MAX_PAGE, MusicLibrary, Page, PlaylistId,
    TrackSummary,
};
use crate::Event;

impl MusicLibrary {
    /// The built-in Favourites playlist, first among the playlists.
    pub fn favourites(&self) -> Result<PlaylistId, LibraryError> {
        Ok(self.library.favourites()?)
    }

    /// Whether `track` is in Favourites.
    pub fn is_favourite(
        &self,
        track: helikodu_model::library::TrackId,
    ) -> Result<bool, LibraryError> {
        Ok(self.library.is_favourite(track)?)
    }

    /// Adds `track` to Favourites, or takes it out; returns whether it is a
    /// favourite now.
    pub fn set_favourite(
        &self,
        track: helikodu_model::library::TrackId,
        favourite: bool,
    ) -> Result<bool, LibraryError> {
        Ok(self.library.set_favourite(track, favourite)?)
    }

    /// Every playlist and folder, each folder followed by what is in it.
    pub fn playlist_tree(&self) -> Result<Vec<PlaylistNode>, LibraryError> {
        Ok(self.library.playlist_tree()?)
    }

    /// Creates an empty folder in `parent` (the top for `None`).
    pub fn create_playlist_folder(
        &self,
        name: &str,
        parent: Option<PlaylistId>,
    ) -> Result<PlaylistId, LibraryError> {
        Ok(self.library.create_playlist_folder(name, parent)?)
    }

    /// Moves a playlist or folder into `parent` at `position`, or after what is there.
    pub fn move_playlist(
        &self,
        playlist: PlaylistId,
        parent: Option<PlaylistId>,
        position: Option<usize>,
    ) -> Result<(), LibraryError> {
        Ok(self.library.move_playlist(playlist, parent, position)?)
    }

    /// Copies a playlist into a new static one named `name`; a smart playlist's copy
    /// holds the tracks it selects now.
    pub fn duplicate_playlist(
        &self,
        playlist: PlaylistId,
        name: &str,
    ) -> Result<PlaylistId, LibraryError> {
        Ok(self.library.duplicate_playlist(playlist, name)?)
    }

    /// Locks a playlist against changes of its items and query, or unlocks it.
    pub fn set_playlist_locked(
        &self,
        playlist: PlaylistId,
        locked: bool,
    ) -> Result<(), LibraryError> {
        Ok(self.library.set_playlist_locked(playlist, locked)?)
    }

    /// Shows a playlist in groups by album, or not.
    pub fn set_playlist_grouped(
        &self,
        playlist: PlaylistId,
        grouped: bool,
    ) -> Result<(), LibraryError> {
        Ok(self.library.set_playlist_grouped(playlist, grouped)?)
    }

    /// Undoes the newest edit of a playlist; whether there was one.
    pub fn undo_playlist(&self, playlist: PlaylistId) -> Result<bool, LibraryError> {
        Ok(self.library.undo_playlist(playlist)?)
    }

    /// Does the newest undone edit of a playlist again; whether there was one.
    pub fn redo_playlist(&self, playlist: PlaylistId) -> Result<bool, LibraryError> {
        Ok(self.library.redo_playlist(playlist)?)
    }

    /// What the tidying commands would remove from a static playlist.
    pub fn playlist_cleanup_counts(
        &self,
        playlist: PlaylistId,
    ) -> Result<CleanupCounts, LibraryError> {
        Ok(self.library.playlist_cleanup_counts(playlist)?)
    }

    /// Removes the items that repeat an earlier one; how many went.
    pub fn remove_playlist_duplicates(&self, playlist: PlaylistId) -> Result<usize, LibraryError> {
        Ok(self.library.remove_playlist_duplicates(playlist)?)
    }

    /// Removes the items that cannot play; how many went.
    pub fn remove_missing_from_playlist(
        &self,
        playlist: PlaylistId,
    ) -> Result<usize, LibraryError> {
        Ok(self.library.remove_missing_from_playlist(playlist)?)
    }

    /// Shuffles the order of a static playlist for good.
    pub fn shuffle_playlist(&self, playlist: PlaylistId, seed: u64) -> Result<(), LibraryError> {
        Ok(self.library.shuffle_playlist(playlist, seed)?)
    }

    /// Creates a smart playlist of the tracks `query` selects.
    pub fn create_smart_playlist(
        &self,
        name: &str,
        query: &Query,
        parent: Option<PlaylistId>,
    ) -> Result<PlaylistId, LibraryError> {
        Ok(self.library.create_smart_playlist(name, query, parent)?)
    }

    /// Replaces the query of a smart playlist.
    pub fn set_smart_playlist_query(
        &self,
        playlist: PlaylistId,
        query: &Query,
    ) -> Result<(), LibraryError> {
        Ok(self.library.set_smart_playlist_query(playlist, query)?)
    }

    /// The query of a smart playlist.
    pub fn smart_playlist_query(&self, playlist: PlaylistId) -> Result<Query, LibraryError> {
        Ok(self.library.smart_playlist_query(playlist)?)
    }

    /// Every track a smart playlist selects now, in its order.
    pub fn smart_playlist_tracks(
        &self,
        playlist: PlaylistId,
    ) -> Result<Vec<TrackSummary>, LibraryError> {
        let mut all = Vec::new();
        loop {
            let page = self
                .library
                .smart_playlist_tracks(playlist, Page::new(all.len(), MAX_PAGE))?;
            let done = page.items.is_empty() || all.len() + page.items.len() >= page.total;
            all.extend(page.items);
            if done {
                return Ok(all);
            }
        }
    }

    /// When each of `tracks` came into the library, by track.
    pub fn added_times(
        &self,
        tracks: &[helikodu_model::library::TrackId],
    ) -> Result<
        std::collections::HashMap<helikodu_model::library::TrackId, std::time::SystemTime>,
        LibraryError,
    > {
        Ok(self.library.added_times(tracks)?)
    }

    /// How many tracks `query` selects among all, and how many tracks there are: the
    /// "N tracks match of total" line of the rule editor.
    pub fn query_match_count(&self, query: &Query) -> Result<(usize, usize), LibraryError> {
        let matched = self
            .library
            .query_count(&QueryRequest::new(query.clone()))?;
        let total = self
            .library
            .query_count(&QueryRequest::new(Query::default()))?;
        Ok((matched, total))
    }

    /// Pins an album, an artist or a playlist to the sidebar.
    pub fn pin(&self, target: PinTarget) -> Result<(), LibraryError> {
        Ok(self.library.pin(target)?)
    }

    /// Unpins it; whether it was pinned.
    pub fn unpin(&self, target: PinTarget) -> Result<bool, LibraryError> {
        Ok(self.library.unpin(target)?)
    }

    /// The pins, in their order.
    pub fn pins(&self) -> Result<Vec<Pin>, LibraryError> {
        Ok(self.library.pins()?)
    }

    /// Picks up the playlist files in the library folders, leaving out what the
    /// `skip` patterns of `settings` name (see [`ScanRequest::exclude`](super::ScanRequest)).
    pub fn import_found_playlists(
        &self,
        settings: &LibrarySettings,
    ) -> Result<FoundReport, LibraryError> {
        let exclude: Vec<ExcludeRule> = settings
            .skip
            .iter()
            .map(|p| {
                if p.contains('/') {
                    ExcludeRule::Path(p.into())
                } else {
                    ExcludeRule::Name(p.clone())
                }
            })
            .collect();
        Ok(self.library.import_found_playlists(&exclude)?)
    }
}

/// Picks up the playlist files of the library folders after scans, while the
/// setting asks for it. Clones share the setting.
#[derive(Debug, Clone)]
pub(super) struct PlaylistFinder {
    library: MusicLibrary,
    on: Arc<AtomicBool>,
    settings: Arc<Mutex<LibrarySettings>>,
}

impl PlaylistFinder {
    pub(super) fn new(library: MusicLibrary) -> Self {
        Self {
            library,
            on: Arc::new(AtomicBool::new(false)),
            settings: Arc::new(Mutex::new(LibrarySettings::default())),
        }
    }

    /// Picks up the files if the setting is on (after a scan, before the library
    /// announces its change).
    pub(super) fn after_scan(&self) {
        if !self.on.load(Ordering::SeqCst) {
            return;
        }
        let settings = self
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Err(error) = self.library.import_found_playlists(&settings) {
            eprintln!("helikodu: cannot pick up the playlist files: {error}");
        }
    }
}

impl LibraryService {
    /// Whether the playlist files in the library folders are picked up after every
    /// scan and rescan (the `[library] import_playlists` setting), leaving out what
    /// its `skip` patterns name. Switching it on picks them up at once, announcing
    /// [`LibraryEvent::Changed`] if any was.
    pub fn set_import_playlists(&self, settings: &LibrarySettings) {
        let on = settings.import_playlists;
        *self
            .finder
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = settings.clone();
        let was = self.finder.on.swap(on, Ordering::SeqCst);
        if on && !was {
            let bus = self.bus.clone();
            let settings = settings.clone();
            let _ = self.run(
                move |l| l.import_found_playlists(&settings),
                move |report| {
                    if report.is_ok_and(|r| r.imported + r.refreshed > 0) {
                        bus.publish(&Event::Library(LibraryEvent::Changed));
                    }
                },
            );
        }
    }
}
