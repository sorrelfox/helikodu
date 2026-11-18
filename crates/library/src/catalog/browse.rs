// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Browsing the catalogue: albums, artists by role, genres and their tracks, one page
//! at a time, in the alphabet order of the interface language.
//!
//! Every query leaves out the tracks of masked library folders unless asked not to,
//! and an album, artist or genre none of whose tracks is left.

mod albums;
mod people;
mod tracks;

use std::time::Duration;

use helikodu_meta::covers::CoverHash;
use helikodu_model::library::TrackId;
use rusqlite::types::Value;

pub(crate) use albums::{album, albums};
pub(crate) use people::{artists, genres};
pub(crate) use tracks::tracks;

use super::ids::{AlbumId, GenreId, PersonId};
use super::track_record::{ArtistRole, TrackKind, TrackSection};

/// The most items one page can hold.
pub const MAX_PAGE: usize = 1000;

/// Which items of a list to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Page {
    /// How many items to skip.
    pub offset: usize,
    /// How many items to return, at most [`MAX_PAGE`].
    pub limit: usize,
}

impl Page {
    /// The first `limit` items.
    pub fn first(limit: usize) -> Self {
        Self { offset: 0, limit }
    }

    /// `limit` items after the first `offset`.
    pub fn new(offset: usize, limit: usize) -> Self {
        Self { offset, limit }
    }

    /// The LIMIT and OFFSET values.
    pub(crate) fn sql(self) -> (i64, i64) {
        (
            i64::try_from(self.limit.min(MAX_PAGE)).unwrap_or(0),
            i64::try_from(self.offset).unwrap_or(i64::MAX),
        )
    }
}

impl Default for Page {
    /// The first 50 items.
    fn default() -> Self {
        Self::first(50)
    }
}

/// One page of a list, and the length of the whole list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paged<T> {
    /// The items of the page, in order.
    pub items: Vec<T>,
    /// The number of items on all pages.
    pub total: usize,
}

/// Which albums to list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AlbumFilter {
    /// Every album.
    #[default]
    All,
    /// The albums of which the person is an album artist (as the library groups
    /// them; see [`ArtistRole::AlbumArtist`]).
    ByArtist(PersonId),
    /// The albums on which the person is a track artist without being an album
    /// artist: "appears on".
    AppearsOn(PersonId),
    /// The albums with a track on which the person has the role.
    Credited(PersonId, ArtistRole),
    /// Compilations: albums with the compilation flag, or by various artists.
    Compilations,
    /// The albums with a track of the genre.
    Genre(GenreId),
}

/// The order of an album list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AlbumSort {
    /// By title.
    #[default]
    Title,
    /// By album artist, then year, then title. Albums without an album artist
    /// (compilations) come last.
    Artist,
    /// By year (the original year when known), then title. Albums without a year
    /// come last.
    Year,
    /// Newest first, by when the album's first track was added.
    Added,
}

/// Which albums to list by their audio format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AlbumQuality {
    /// Every album.
    #[default]
    Any,
    /// Albums whose tracks are all in a lossless format.
    Lossless,
    /// Lossless albums with a track above 48 kHz or 16 bits.
    HiRes,
}

/// A request for a list of albums.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct AlbumQuery {
    /// Which albums.
    pub filter: AlbumFilter,
    /// Which albums by their audio format; combined with `filter`.
    pub quality: AlbumQuality,
    /// In which order.
    pub sort: AlbumSort,
    /// Which page.
    pub page: Page,
    /// Whether the tracks of masked library folders count.
    pub include_masked: bool,
}

/// An album in a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumSummary {
    /// The album.
    pub id: AlbumId,
    /// The title.
    pub title: Option<String>,
    /// The album artist as credited. `None` for a compilation by various artists,
    /// which the interface names in its own language.
    pub artist: Option<String>,
    /// Whether it is a compilation.
    pub is_compilation: bool,
    /// The release year.
    pub year: Option<i32>,
    /// The year of the original release.
    pub original_year: Option<i32>,
    /// The release types, primary first, such as `album` and `live`.
    pub release_types: Vec<String>,
    /// The number of tracks.
    pub track_count: usize,
    /// The number of discs with tracks in the library.
    pub disc_count: u32,
    /// The number of discs the tags give, if larger.
    pub disc_total: Option<u32>,
    /// The length of all tracks, when known for some.
    pub duration: Option<Duration>,
    /// When its first track was added, in seconds since the Unix epoch.
    pub added_at: Option<i64>,
    /// The hash of its cover, the key of the thumbnail cache; `None` without one.
    pub cover: Option<CoverHash>,
    /// The audio format of its tracks; `None` when no track has one.
    pub format: Option<AlbumFormat>,
}

/// The audio format of an album, summed up from its tracks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlbumFormat {
    /// The codec most of its tracks have, such as `flac` or `mp3`.
    pub codec: Option<String>,
    /// The highest sample rate of its tracks, in Hz.
    pub sample_rate: Option<u32>,
    /// The highest bit depth of its tracks, for lossless formats.
    pub bit_depth: Option<u16>,
    /// The highest bit rate of its tracks, in bit/s.
    pub bitrate: Option<u32>,
    /// Whether every track is in a lossless format.
    pub lossless: bool,
    /// Whether it is lossless with a track above 48 kHz or 16 bits.
    pub hi_res: bool,
}

/// An album with its discs and tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumDetail {
    /// What a list shows of it.
    pub summary: AlbumSummary,
    /// The title for sorting.
    pub title_sort: Option<String>,
    /// The album artist for sorting.
    pub artist_sort: Option<String>,
    /// The album artists, as people.
    pub artists: Vec<ArtistRef>,
    /// The genres of its tracks, most frequent first.
    pub genres: Vec<GenreRef>,
    /// The MusicBrainz release ID.
    pub mbid: Option<String>,
    /// The MusicBrainz release group ID.
    pub release_group_mbid: Option<String>,
    /// The discs, in order, each with its tracks in order.
    pub discs: Vec<Disc>,
}

/// One disc of an album.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disc {
    /// The disc number; 1 for tracks without one.
    pub number: u32,
    /// The disc's title.
    pub subtitle: Option<String>,
    /// The tracks, by track number, then file name and position in the file.
    pub tracks: Vec<TrackSummary>,
}

/// A person, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistRef {
    /// The person.
    pub id: PersonId,
    /// The name.
    pub name: String,
}

/// A genre, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreRef {
    /// The genre.
    pub id: GenreId,
    /// The name.
    pub name: String,
}

/// A request for a list of people in one role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArtistQuery {
    /// The role. [`ArtistRole::AlbumArtist`] lists the album artists as the library
    /// groups albums, including artists taken from the track artists.
    pub role: ArtistRole,
    /// Which page.
    pub page: Page,
    /// Whether the tracks of masked library folders count.
    pub include_masked: bool,
}

impl ArtistQuery {
    /// The first page of the people in `role`.
    pub fn new(role: ArtistRole) -> Self {
        Self {
            role,
            page: Page::default(),
            include_masked: false,
        }
    }
}

/// A person in a list, in the order of their sort names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistSummary {
    /// The person.
    pub id: PersonId,
    /// The name.
    pub name: String,
    /// The name for sorting.
    pub sort_name: Option<String>,
    /// The albums on which they have the role.
    pub album_count: usize,
    /// The tracks on which they have the role.
    pub track_count: usize,
}

/// A request for the list of genres.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct GenreQuery {
    /// Which page.
    pub page: Page,
    /// Whether the tracks of masked library folders count.
    pub include_masked: bool,
}

/// A genre in a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreSummary {
    /// The genre.
    pub id: GenreId,
    /// The name.
    pub name: String,
    /// The albums with a track of the genre.
    pub album_count: usize,
    /// The tracks of the genre.
    pub track_count: usize,
}

/// Which tracks to list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TrackFilter {
    /// Every track, by album artist, year, album and track number.
    #[default]
    All,
    /// The tracks of an album, in album order.
    Album(AlbumId),
    /// The tracks of a person, in any role (`None`) or in one, by year, album and
    /// track number. With [`ArtistRole::AlbumArtist`], the tracks of their albums.
    Artist(PersonId, Option<ArtistRole>),
    /// The tracks of a genre, by album artist, year, album and track number.
    Genre(GenreId),
}

/// A request for a list of tracks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TrackQuery {
    /// Which tracks.
    pub filter: TrackFilter,
    /// Which page.
    pub page: Page,
    /// Whether the tracks of masked library folders count.
    pub include_masked: bool,
}

/// A track in a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackSummary {
    /// The track.
    pub id: TrackId,
    /// The title.
    pub title: Option<String>,
    /// The artist as credited.
    pub artist: Option<String>,
    /// The album.
    pub album: Option<AlbumId>,
    /// The album's title.
    pub album_title: Option<String>,
    /// The disc number.
    pub disc_no: Option<u32>,
    /// The track number.
    pub track_no: Option<u32>,
    /// The length.
    pub duration: Option<Duration>,
    /// The release year.
    pub year: Option<i32>,
    /// What kind of recording it is.
    pub kind: TrackKind,
    /// What part of its file it is.
    pub section: TrackSection,
    /// The CUE track or chapter number, or 0 for a whole file.
    pub sub_index: u32,
    /// Whether its file is missing (it disappeared and has not come back yet).
    pub missing: bool,
}

/// Parameters of a query, in order.
pub(crate) type Params = Vec<Value>;
