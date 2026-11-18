// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The music library: database, scanner, watcher, search, query language and statistics.
//!
//! [`Library`] is the entry point: it opens the database and offers typed operations on
//! tracks, statistics and search. See the crate's README for its place in the
//! workspace.

#![forbid(unsafe_code)]

mod catalog;
mod covers;
mod db;
mod details;
mod library;
mod playlists;
mod scan;
mod search;
mod stats;
mod tag_writes;
mod track_source;
mod watch;

pub use catalog::{
    AlbumDetail, AlbumFilter, AlbumFormat, AlbumId, AlbumQuality, AlbumQuery, AlbumSort,
    AlbumSummary, ArtistQuery, ArtistRef, ArtistRole, ArtistSummary, CatalogTags, Credit, Disc,
    GenreId, GenreQuery, GenreRef, GenreSummary, MAX_PAGE, Page, Paged, PersonId, RootState,
    TrackFilter, TrackOrder, TrackQuery, TrackSection, TrackSortKey, TrackSummary,
};
pub use catalog::{
    AudioProperties, CoverInfo, CoverLocation, FileStamp, IdentityId, ReplayGain, RootId,
    TrackChange, TrackCovers, TrackFingerprint, TrackKind, TrackRecord, TrackTags, TrackWrite,
};
pub use covers::{
    ArtworkOptions, ArtworkPool, CoverLocator, CoverReadError, DEFAULT_CACHE_BYTES, Removed,
    Thumbnail, ThumbnailCache, ThumbnailCacheOptions, ThumbnailEvent, ThumbnailSize, read_cover,
    render_thumbnail,
};
pub use db::{
    DatabaseConfig, DbError, LocaleCollation, Location, MemoryName, SCHEMA_VERSION, SqliteError,
    display_text, path_from_blob, path_to_blob,
};
pub use details::{
    FileDetails, LibraryTotals, PlayedTrack, ReadFileError, TrackDetails, read_file_details,
};
/// The content hash of a cover, the key of the thumbnail cache.
pub use helikodu_meta::covers::CoverHash;
/// The playlist file formats an export can write and an import reads.
pub use helikodu_meta::playlists::{PathStyle, PlaylistKind};
/// A cover found by a scan worker, as a [`TrackCovers`] holds it.
pub use helikodu_meta::worker::{CoverOrigin, CoverReport};
pub use library::Library;
pub use playlists::{
    DeadItem, DeadReference, ExportOptions, ExportReport, ImportOptions, ImportReport, ItemContent,
    PlaylistError, PlaylistId, PlaylistItem, PlaylistItemId, PlaylistSummary,
};
pub use scan::{
    CancelToken, ExcludeRule, FailureKind, FileFailure, ScanError, ScanEvent, ScanHandle,
    ScanOptions, ScanSummary, SymlinkPolicy, WORKER_PROGRAM, WorkerOptions, find_worker_program,
};
pub use search::{MatchRank, SearchHit, SearchRequest, SearchResults, fold};
pub use stats::{
    HistoryEntry, HistoryQuery, Listen, ListenEnd, ListenOutcome, PlayRules, Rating, TrackStats,
};
pub use tag_writes::{RateError, RatingOutcome, TagWriteReport, TagWriteSettings};
pub use track_source::TrackSource;
pub use watch::{PollReason, RescanReason, WatchEvent, WatchMode, WatchOptions, Watcher};

/// What a tag editor needs to open files and write their tags: the types of
/// `meta`'s tag writer, so that front ends need not depend on `meta`.
///
/// Open each file with [`load_for_editing`](tag_editing::load_for_editing), show
/// the values a selection shares with [`SelectionView`](tag_editing::SelectionView),
/// collect the edits in [`TagChanges`](tag_editing::TagChanges) and write them with
/// [`Library::write_tags`] as one [`BatchItem`](tag_editing::BatchItem) per file.
pub mod tag_editing {
    /// A rating as tags hold it: 0 to 10 half stars.
    pub use helikodu_meta::tags::Rating as TagRating;
    pub use helikodu_meta::tags::writing::{
        BatchItem, BatchProgress, BatchReport, CoverChange, EditStamp, EditableFile, FieldChange,
        FieldKey, Id3Version, InPlaceReason, SelectionView, Shared, SkippedChange, TagChanges,
        TagField, WriteError, WriteMode, WriteOptions, WriteOutcome, field_values, is_writable,
        load_for_editing, read_lyrics,
    };
    pub use helikodu_meta::tags::{ReadOptions, TagError};
}

/// Direct SQL access to the database, below [`Library`].
///
/// For this crate's tests and benchmarks, and for diagnostics. It is not a stable
/// interface: the schema and these types change without notice. Code in other crates
/// uses the typed operations of [`Library`].
pub mod raw {
    pub use crate::db::{Database, WriteTicket, Writer};
    /// The SQLite bindings, whose types appear in the read and write callbacks.
    pub use rusqlite;

    /// The error from the SQLite bindings underneath `error`.
    pub fn sqlite_error(error: &crate::SqliteError) -> &rusqlite::Error {
        error.inner()
    }
}
