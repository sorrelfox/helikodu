// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The front end's types for labels: the words of their own that users give
//! tracks, albums and artists. Labels live in the library, never in the files.
//!
//! Every change returns a [`LabelUndo`]; the undos of changes made one after the
//! other are joined with [`LabelUndo::absorb`], and
//! [`LibrarySource::undo_labels`](super::LibrarySource::undo_labels) puts the
//! labels back as they were before the first of them.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use super::{AlbumKey, AlbumRow, ArtistKey, ArtistRow, BackendError, TrackKey, TrackRow};

/// A label of the library. It keeps its key through a rename and an undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct LabelKey(pub u64);

/// One label of the Labels view.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LabelRow {
    /// The label.
    pub key: LabelKey,
    /// Its name, as last written.
    pub name: String,
    /// The tracks that have it, given to them or to their albums or artists.
    pub tracks: usize,
    /// The tracks it was given to one by one.
    pub own_tracks: usize,
    /// The albums it was given to.
    pub albums: usize,
    /// The artists it was given to.
    pub artists: usize,
}

/// What labels are given to or taken from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelFor {
    /// These tracks.
    Tracks(Vec<TrackKey>),
    /// An album, and so each of its tracks.
    Album(AlbumKey),
    /// An artist, and so each track they play or are album artist of.
    Artist(ArtistKey),
}

/// How a label stands for what the Labels popover is open for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelState {
    /// Everything has it.
    All,
    /// Part of it has it.
    Some,
    /// Nothing has it.
    None,
    /// The tracks have it only from their album.
    FromAlbum,
    /// The tracks have it only from one of their artists.
    FromArtist,
}

impl LabelState {
    /// The state of a label that `own` of `items` have of their own, and
    /// `from_album` and `from_artist` others have from their album or an artist.
    pub fn of(items: usize, own: usize, from_album: usize, from_artist: usize) -> Self {
        if items > 0 && own >= items {
            Self::All
        } else if own == 0 && from_album + from_artist == 0 {
            Self::None
        } else if items > 0 && own == 0 && from_album + from_artist >= items {
            if from_album > 0 {
                Self::FromAlbum
            } else {
                Self::FromArtist
            }
        } else {
            Self::Some
        }
    }

    /// Whether it is given through an album or an artist, and changed there.
    pub fn inherited(self) -> bool {
        matches!(self, Self::FromAlbum | Self::FromArtist)
    }
}

/// One label in the Labels popover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelChoice {
    /// The label.
    pub key: LabelKey,
    /// Its name.
    pub name: String,
    /// The tracks of the library that have it.
    pub tracks: usize,
    /// How it stands for what the popover is open for.
    pub state: LabelState,
}

/// Every label, with how each stands for one selection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LabelChoices {
    /// The tracks of the selection, or 1 for an album or an artist.
    pub items: usize,
    /// Every label, in name order.
    pub labels: Vec<LabelChoice>,
}

impl LabelChoices {
    /// The labels everything of the selection has of its own, as the chips of an
    /// album or artist page show them.
    pub fn own(&self) -> impl Iterator<Item = &LabelChoice> {
        self.labels.iter().filter(|l| l.state == LabelState::All)
    }
}

/// A change of labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelEdit {
    /// Gives the label `name` to `target`, making it if there is none.
    Give {
        /// What gets it.
        target: LabelFor,
        /// The label.
        name: String,
    },
    /// Takes the label `name` from `target`.
    Take {
        /// What loses it.
        target: LabelFor,
        /// The label.
        name: String,
    },
    /// Renames the label `name`.
    Rename {
        /// The label.
        name: String,
        /// Its new name.
        new_name: String,
    },
    /// Merges `from` into `into`: what has `from` has `into` instead.
    Merge {
        /// The label that goes.
        from: String,
        /// The label that stays.
        into: String,
    },
    /// Removes the label `name` from everything.
    Delete {
        /// The label.
        name: String,
    },
}

/// What undoes label changes. Each source keeps its own kind of step in it.
#[derive(Clone, Default)]
pub struct LabelUndo(Vec<Arc<dyn Any + Send + Sync>>);

impl LabelUndo {
    /// The undo of one change, as a source keeps it.
    pub fn step(step: impl Any + Send + Sync) -> Self {
        Self(vec![Arc::new(step)])
    }

    /// Joins the undo of `later`, a change made after these.
    pub fn absorb(&mut self, later: LabelUndo) {
        self.0.extend(later.0);
    }

    /// Whether there is nothing to undo.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The steps of type `T`, oldest first.
    pub fn steps<T: Any>(&self) -> impl Iterator<Item = &T> {
        self.0.iter().filter_map(|s| s.downcast_ref::<T>())
    }
}

impl fmt::Debug for LabelUndo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("LabelUndo").field(&self.0.len()).finish()
    }
}

/// What a change did.
#[derive(Debug, Clone, Default)]
pub struct LabelDone {
    /// How many tracks, albums or artists got or lost the label (0 for a rename,
    /// a merge or a removal from everything).
    pub count: usize,
    /// What undoes it.
    pub undo: LabelUndo,
}

/// Why a change was not made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelEditError {
    /// Another label has that name already: the two can be merged.
    Exists(String),
    /// Anything else, said for the user.
    Failed(BackendError),
}

impl From<BackendError> for LabelEditError {
    fn from(error: BackendError) -> Self {
        Self::Failed(error)
    }
}

/// Everything the detail of the Labels view shows of one label.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LabelContents {
    /// The label.
    pub row: LabelRow,
    /// The artists it was given to.
    pub artists: Vec<ArtistRow>,
    /// The albums it was given to.
    pub albums: Vec<AlbumRow>,
    /// The tracks it was given to one by one.
    pub tracks: Vec<TrackRow>,
}
