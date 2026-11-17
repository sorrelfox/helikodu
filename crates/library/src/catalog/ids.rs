// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Identifiers of catalogue rows.

use std::fmt;

use helikodu_model::library::TrackId;

/// The identity of a track: what its statistics, rating and history belong to. It
/// outlives the track's path, so they survive moves and renames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdentityId(pub(crate) i64);

impl IdentityId {
    /// The number of the identity in the database.
    pub fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for IdentityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A library folder chosen by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootId(pub(crate) i64);

impl RootId {
    /// The number of the root in the database.
    pub fn get(self) -> i64 {
        self.0
    }
}

/// The database key of `track`. A number too large for SQLite cannot be in the
/// database; it becomes -1, which matches no row.
pub(crate) fn sql_track_id(track: TrackId) -> i64 {
    i64::try_from(track.get()).unwrap_or(-1)
}

/// The track with the database key `id`. Keys are positive.
pub(crate) fn track_id_from_sql(id: i64) -> TrackId {
    TrackId::new(u64::try_from(id).unwrap_or(0))
}

macro_rules! row_id {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub(crate) i64);

        impl $name {
            #[doc = concat!("The number of the ", $what, " in the database.")]
            pub fn get(self) -> i64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

row_id!(
    /// An album. Its id stays while its tracks keep forming it; an album that splits
    /// or merges with another may get a new one.
    AlbumId,
    "album"
);
row_id!(
    /// A person: an artist, album artist, composer, performer, conductor and so on.
    PersonId,
    "person"
);
row_id!(
    /// A genre.
    GenreId,
    "genre"
);
