// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! References to something playable.

use serde::{Deserialize, Serialize};

use super::{MediaSource, TrackId};

/// A track to play: one from the library, or a file outside it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackRef {
    /// A track of the library; its source is looked up when it is played.
    Library(TrackId),
    /// A file, or part of one, that need not be in the library.
    File(MediaSource),
}

impl From<TrackId> for TrackRef {
    fn from(id: TrackId) -> Self {
        Self::Library(id)
    }
}

impl From<MediaSource> for TrackRef {
    fn from(source: MediaSource) -> Self {
        Self::File(source)
    }
}
