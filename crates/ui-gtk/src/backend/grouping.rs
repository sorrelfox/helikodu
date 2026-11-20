// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the track table is grouped by, and its groups. The groups are made by
//! the library ([`LibrarySource::group_tracks`](super::LibrarySource::group_tracks)),
//! never by the table's model: the table asks for the tracks of its query in the
//! order its columns sort them, and the library puts them in groups.

use super::TrackKey;

/// What the track table is grouped by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TrackGrouping {
    /// The album artist.
    AlbumArtist,
    /// The album.
    Album,
    /// The first genre.
    Genre,
    /// The year.
    Year,
    /// The format (the codec).
    Format,
    /// The first label by name.
    Label,
    /// One of the user's fields, by its name: computed or a tag.
    Field(String),
    /// An expression of the expression language.
    Expression(String),
}

impl TrackGrouping {
    /// The built-in choices, in the order the menu offers them.
    pub const BUILT_IN: [TrackGrouping; 6] = [
        TrackGrouping::AlbumArtist,
        TrackGrouping::Album,
        TrackGrouping::Genre,
        TrackGrouping::Year,
        TrackGrouping::Format,
        TrackGrouping::Label,
    ];

    /// The name of the field in the query language (`albumartist`, `decade`),
    /// or the expression.
    pub fn field_name(&self) -> &str {
        match self {
            Self::AlbumArtist => "albumartist",
            Self::Album => "album",
            Self::Genre => "genre",
            Self::Year => "year",
            Self::Format => "codec",
            Self::Label => "label",
            Self::Field(name) | Self::Expression(name) => name,
        }
    }
}

/// One group of the track table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackGroup {
    /// The value its tracks share; empty for the tracks without one.
    pub value: String,
    /// Its tracks, in the table's order.
    pub keys: Vec<TrackKey>,
}
