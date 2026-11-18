// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the engine plays for a library track: its file, and for a CUE track or a
//! chapter the span of the file.

use std::time::Duration;

use helikodu_library::raw::rusqlite::OptionalExtension;
use helikodu_library::{DbError, Library, TrackKind};
use helikodu_model::audio::{MediaTime, SampleRate};
use helikodu_model::library::{MediaSource, SourceSection, TrackId};

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

/// The span stored for a track: its section kind (0 for a whole file), the first
/// frame and the end frame, at the file's sample rate.
type StoredSpan = (i64, Option<i64>, Option<i64>, Option<i64>);

pub(super) fn track_source(
    library: &Library,
    track: TrackId,
) -> Result<Option<TrackSource>, DbError> {
    let Some(details) = library.track_details(track)? else {
        return Ok(None);
    };
    // The library's typed interface does not give a track's frame range yet; it is
    // read from the `tracks` table, where the scanner stores it with the sample rate
    // it is counted at.
    let id = i64::try_from(track.get()).unwrap_or(i64::MAX);
    let span: Option<StoredSpan> = library.database().read(|c| {
        c.query_row(
            "SELECT section, start_frame, end_frame, sample_rate FROM tracks WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
    })?;
    let mut source = MediaSource::file(details.path);
    if let Some((section, Some(start), end, Some(rate))) = span
        && section != 0
        && let Some(rate) = u32::try_from(rate).ok().and_then(SampleRate::new)
    {
        let frames = |frame: i64| MediaTime::from_frames(u64::try_from(frame).unwrap_or(0), rate);
        source = source.with_section(SourceSection::Span {
            start: frames(start),
            end: end.map(frames),
        });
    }
    Ok(Some(TrackSource {
        track,
        source,
        kind: details.kind,
        duration: details.audio.duration,
    }))
}
