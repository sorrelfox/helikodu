// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The catalogue: tracks, the files they come from, their people, albums and genres, and
//! the identity that keeps a track's statistics across moves.
//!
//! A track's identity is found by its fingerprint: the quick hash of its file and its
//! audio properties. A file that disappears and comes back elsewhere with the same size
//! and quick hash is the same file; its row moves, so the track and its identity stay.
//! A track that left the library and comes back with the same fingerprint gets its old
//! identity back, with its statistics, rating and history.

pub(crate) mod albums;
mod browse;
pub(crate) mod covers;
mod fingerprint;
mod folders;
mod ids;
mod people;
pub(crate) mod roots;
mod track_record;
pub(crate) mod write;

pub use browse::{
    AlbumDetail, AlbumFilter, AlbumFormat, AlbumQuality, AlbumQuery, AlbumSort, AlbumSummary,
    ArtistQuery, ArtistRef, ArtistSummary, Disc, GenreQuery, GenreRef, GenreSummary, MAX_PAGE,
    Page, Paged, TrackFilter, TrackOrder, TrackQuery, TrackSortKey, TrackSummary,
};
pub(crate) use browse::{album, albums, artists, genres, tracks};
pub use covers::{CoverInfo, CoverLocation};
pub use fingerprint::TrackFingerprint;
pub use ids::{AlbumId, GenreId, IdentityId, PersonId, RootId};
pub(crate) use ids::{sql_track_id, track_id_from_sql};
pub use roots::RootState;
pub use track_record::{
    ArtistRole, AudioProperties, CatalogTags, Credit, FileStamp, ReplayGain, TrackCovers,
    TrackKind, TrackRecord, TrackSection, TrackTags,
};
pub use write::{TrackChange, TrackWrite};
