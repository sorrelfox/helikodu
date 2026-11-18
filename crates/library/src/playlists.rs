// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Static playlists: creating, renaming, duplicating and deleting them, editing their
//! items, and importing and exporting playlist files.
//!
//! An item refers to a track's identity, so it follows the track through moves and
//! renames and comes back with it when the track returns to the library. An item whose
//! identity has no track, and an imported entry that matched nothing, is kept as a dead
//! item with the path or URL and the tags it was known by. URLs are kept as unsupported
//! items and never fetched.
//!
//! Every edit is one write job: one transaction, committed with `synchronous=FULL`, so
//! a crash leaves a playlist as it was before or after the edit, never in between.
//! Positions count from 0 without gaps.

mod export;
mod import;
mod items;
mod store;

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use helikodu_meta::playlists::{
    PathStyle, PlaylistKind, PlaylistLimits, PlaylistWarning, TextEncoding, WriteWarning,
};
use helikodu_model::library::TrackId;

use crate::catalog::{IdentityId, TrackSummary};
use crate::db::DbError;

pub(crate) use export::{export_rows, write_atomically};
pub(crate) use import::{imported_name, match_entries};
pub(crate) use items::{
    append_tracks, dead_items, insert_tracks, item_page, move_items, remove_items,
};
pub(crate) use store::{create, delete, duplicate, insert_imported, playlist, playlists, rename};

/// A playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlaylistId(pub(crate) i64);

impl PlaylistId {
    /// The number of the playlist in the database.
    pub fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for PlaylistId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// An item of a playlist. It keeps its id while its position changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlaylistItemId(pub(crate) i64);

impl PlaylistItemId {
    /// The number of the item in the database.
    pub fn get(self) -> i64 {
        self.0
    }
}

/// A playlist as the sidebar lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistSummary {
    /// The playlist.
    pub id: PlaylistId,
    /// Its name.
    pub name: String,
    /// The number of items.
    pub items: usize,
    /// The number of dead items: entries that name no track in the library.
    pub dead: usize,
    /// The total length of the items whose length is known.
    pub duration: Duration,
    /// When it was created.
    pub created: SystemTime,
    /// When it was last changed.
    pub modified: SystemTime,
}

/// One item of a playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistItem {
    /// The item.
    pub id: PlaylistItemId,
    /// Its position, from 0.
    pub position: usize,
    /// What it stands for.
    pub content: ItemContent,
}

/// What a playlist item stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemContent {
    /// A track in the library.
    Track(TrackSummary),
    /// An entry that names no track in the library.
    Dead(DeadItem),
}

/// What is known about a dead item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadItem {
    /// What it was added or imported with.
    pub reference: DeadReference,
    /// The identity of the track it was, when it was a track of the library that has
    /// left it. The item comes back to life when a track with this identity returns.
    pub identity: Option<IdentityId>,
    /// The title it was known by.
    pub title: Option<String>,
    /// The artist it was known by.
    pub artist: Option<String>,
    /// The album it was known by.
    pub album: Option<String>,
    /// The length it was known by.
    pub duration: Option<Duration>,
}

/// The path or URL of a dead item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DeadReference {
    /// Nothing is known.
    Unknown,
    /// A path on this system.
    Path(PathBuf),
    /// A path written for another system (a drive letter or a network share), with
    /// `/` as the separator.
    Foreign(Vec<u8>),
    /// A URL. It is not supported and never fetched.
    Url(String),
}

/// How to import a playlist file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportOptions {
    /// The name of the new playlist. Without one, the title the file gives, or else the
    /// file's name without its extension.
    pub name: Option<String>,
    /// The legacy charset the user chose for M3U and PLS files that are not UTF-8, as a
    /// WHATWG label such as `windows-1251`. When unset, it is guessed.
    pub legacy_charset: Option<String>,
    /// The bounds the parser keeps to.
    pub limits: PlaylistLimits,
}

/// What an import did.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    /// The new playlist.
    pub playlist: PlaylistId,
    /// Its name.
    pub name: String,
    /// The format the file was read as.
    pub kind: PlaylistKind,
    /// The charset its text was decoded with.
    pub encoding: TextEncoding,
    /// The entries kept, which are the playlist's items.
    pub entries: usize,
    /// The entries found in the library by their path.
    pub by_path: usize,
    /// The entries found by the end of their path (the file name and the folders above
    /// it), for a playlist made on another system or before a move.
    pub by_name: usize,
    /// The entries found by their title, artist and length.
    pub by_tags: usize,
    /// The entries kept as dead items, including the URLs.
    pub dead: usize,
    /// The URL entries, kept as unsupported dead items and never fetched.
    pub unsupported: usize,
    /// What the parser skipped or repaired.
    pub warnings: Vec<PlaylistWarning>,
}

/// How to export a playlist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// The format; M3U8 by default.
    pub kind: PlaylistKind,
    /// Relative paths (to the file's folder) or absolute ones.
    pub path_style: PathStyle,
    /// Whether dead items are left out, instead of written with the path or URL they
    /// were known by.
    pub skip_dead: bool,
}

/// What an export wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// The entries written.
    pub entries: usize,
    /// The dead items among the items: written with what they were known by, or left
    /// out with [`ExportOptions::skip_dead`] (or when nothing is known of them).
    pub dead: usize,
    /// The entries the format could not carry as they were.
    pub warnings: Vec<WriteWarning>,
}

/// Why a playlist operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlaylistError {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// There is no such playlist.
    #[error("there is no playlist {0}")]
    UnknownPlaylist(PlaylistId),
    /// The playlist is not a static playlist, so its items cannot be edited.
    #[error("playlist {0} is not a static playlist")]
    NotStatic(PlaylistId),
    /// The track is not in the library.
    #[error("there is no track {0} in the library")]
    UnknownTrack(TrackId),
    /// A position is past the end of the playlist.
    #[error("position {position} is past the end of a playlist of {len} items")]
    BadPosition {
        /// The position given.
        position: usize,
        /// The number of items.
        len: usize,
    },
    /// A playlist name must hold more than white space.
    #[error("a playlist needs a name")]
    EmptyName,
    /// The playlist file could not be read.
    #[error(transparent)]
    Read(#[from] helikodu_meta::playlists::PlaylistError),
    /// The playlist file could not be written.
    #[error("cannot write {}: {source}", path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        #[source]
        source: io::Error,
    },
}

/// A refusal raised inside a write job, so that the job's changes are rolled back.
#[derive(Debug)]
pub(crate) enum Refusal {
    UnknownPlaylist(PlaylistId),
    NotStatic(PlaylistId),
    UnknownTrack(TrackId),
    BadPosition { position: usize, len: usize },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Refusal {}

impl Refusal {
    /// The error that rolls the job back and carries the refusal out.
    pub(crate) fn raise<T>(self) -> rusqlite::Result<T> {
        Err(rusqlite::Error::UserFunctionError(Box::new(self)))
    }
}

impl PlaylistError {
    /// The error for `error` from a write or read job: a [`Refusal`] it carries, or the
    /// database error.
    pub(crate) fn from_job(error: DbError) -> Self {
        match error {
            DbError::Sqlite(crate::db::SqliteError(rusqlite::Error::UserFunctionError(inner))) => {
                match inner.downcast::<Refusal>() {
                    Ok(refusal) => match *refusal {
                        Refusal::UnknownPlaylist(id) => Self::UnknownPlaylist(id),
                        Refusal::NotStatic(id) => Self::NotStatic(id),
                        Refusal::UnknownTrack(id) => Self::UnknownTrack(id),
                        Refusal::BadPosition { position, len } => {
                            Self::BadPosition { position, len }
                        }
                    },
                    Err(other) => Self::Db(DbError::Sqlite(crate::db::SqliteError(
                        rusqlite::Error::UserFunctionError(other),
                    ))),
                }
            }
            other => Self::Db(other),
        }
    }
}

/// `name` trimmed, or [`PlaylistError::EmptyName`].
pub(crate) fn check_name(name: &str) -> Result<String, PlaylistError> {
    let name = name.trim();
    if name.is_empty() {
        Err(PlaylistError::EmptyName)
    } else {
        Ok(name.to_owned())
    }
}

/// `playlists.kind` of a static playlist.
pub(crate) const STATIC: i64 = 1;

/// `playlist_items.ref_kind`: a path on this system.
pub(crate) const REF_PATH: i64 = 0;
/// `playlist_items.ref_kind`: a path written for another system.
pub(crate) const REF_FOREIGN: i64 = 1;
/// `playlist_items.ref_kind`: a URL.
pub(crate) const REF_URL: i64 = 2;

/// An item to be written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NewItem {
    pub(crate) identity: Option<i64>,
    pub(crate) ref_kind: i64,
    pub(crate) reference: Option<Vec<u8>>,
    pub(crate) title: Option<String>,
    pub(crate) artist: Option<String>,
    pub(crate) album: Option<String>,
    pub(crate) duration_ms: Option<i64>,
}
