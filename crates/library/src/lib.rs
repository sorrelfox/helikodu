// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The music library: database, scanner, watcher, search, query language and statistics.
//!
//! [`Library`] is the entry point: it opens the database and offers typed operations on
//! tracks, statistics and search. See the crate's README for its place in the
//! workspace.

#![forbid(unsafe_code)]

mod catalog;
mod copies;
mod covers;
mod db;
mod details;
mod library;
mod loudness;
mod maintenance;
mod media_kinds;
mod play_keys;
mod playlists;
mod query;
mod scan;
mod search;
mod statistics;
mod stats;
mod tag_journal;
pub mod tag_tools;
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
pub use catalog::{YearQuery, YearSpan, YearSummary};
pub use copies::{
    BACKUP_SUFFIX, CopyFile, CopyGroup, DuplicateCounts, DuplicateEntry, DuplicateGroup, FileId,
    KeepRule, MainCopyError, MatchBy, TrashOutcome, TrashRefusal, backup_path,
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
pub use loudness::{
    AlbumMeasurement, LoudnessCoverage, LoudnessSelection, LoudnessTarget, MeasuredLoudness,
    REFERENCE_LUFS, StoredLoudness,
};
pub use maintenance::{Trash, TrashError, Trashed};
pub use media_kinds::FileSection;
pub use play_keys::PlayKeys;
pub use playlists::{CleanupCounts, FoundReport, Pin, PinTarget, PlaylistNode, PlaylistType};
pub use playlists::{
    DeadItem, DeadReference, ExportOptions, ExportReport, ImportOptions, ImportReport, ItemContent,
    PlaylistError, PlaylistId, PlaylistItem, PlaylistItemId, PlaylistSummary,
};
pub use query::{FacetChoice, FacetField, FacetKey, FacetList, FacetValue};
pub use query::{
    ParsedQuery, QueryError, QueryRequest, SavedSearch, SavedSearchId, SyntaxError,
    SyntaxErrorKind, parse_query, parse_query_lenient, print_query,
};
pub use scan::{
    CancelToken, ExcludeRule, FailureKind, FileFailure, ScanError, ScanEvent, ScanHandle,
    ScanOptions, ScanSummary, SymlinkPolicy, WORKER_PROGRAM, WorkerOptions, find_worker_program,
};
pub use search::{MatchRank, SearchHit, SearchRequest, SearchResults, fold};
pub use statistics::{LibraryStatistics, PlayedAlbum, PlayedArtist, StatisticsRequest};
pub use stats::{
    HistoryEntry, HistoryQuery, HistoryTotals, Listen, ListenEnd, ListenOutcome, PlayRules, Rating,
    TrackStats,
};
pub use tag_journal::{
    KEPT_DAYS, KEPT_STEPS, StepProblem, TagEditReport, TagHistory, TagStep, TagStepId,
    TagStepReport,
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
    /// The pictures inside a file, to save its front cover as a file.
    pub use helikodu_meta::covers::{CoverLimits, read_embedded_covers};
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

/// Lyrics from a track's tags or from a file beside it: the types of `meta`'s
/// lyrics reader, so that front ends need not depend on `meta`.
pub use helikodu_meta::lyrics;

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
