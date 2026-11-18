// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the engine plays for a library track: its file, and for a CUE track or a
//! chapter the span of the file.

use std::time::Duration;

use helikodu_library::{DbError, Library, TrackKind};
use helikodu_model::library::{MediaSource, TrackId};

/// A library track as the player needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackSource {
    /// The track.
    pub track: TrackId,
    /// What the engine plays.
    pub source: MediaSource,
    /// What kind of recording it is.
    pub kind: TrackKind,
    /// Its length, as the library knows it.
    pub duration: Option<Duration>,
}

pub(super) fn track_source(
    library: &Library,
    track: TrackId,
) -> Result<Option<TrackSource>, DbError> {
    let Some(details) = library.track_details(track)? else {
        return Ok(None);
    };
    let Some(found) = library.track_source(track)? else {
        return Ok(None);
    };
    Ok(Some(TrackSource {
        track,
        source: found.into(),
        kind: details.kind,
        duration: details.audio.duration,
    }))
}
