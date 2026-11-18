// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What plays for a library track: its file, and for a CUE sheet track or a chapter
//! the span of the file, for a subsong the subsong.

use std::path::PathBuf;

use helikodu_model::audio::{MediaTime, SampleRate};
use helikodu_model::library::{MediaSource, SourceSection, TrackId};
use rusqlite::{Connection, OptionalExtension};

use crate::Library;
use crate::catalog::{TrackSection, sql_track_id};
use crate::db::{DbError, path_from_blob};

/// Where a library track's audio is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackSource {
    /// The file.
    pub path: PathBuf,
    /// The span of the file for a CUE sheet track or a chapter, the subsong for a
    /// subsong; `None` for a whole file.
    pub section: Option<SourceSection>,
}

impl TrackSource {
    /// The source as the engine plays it.
    pub fn media_source(&self) -> MediaSource {
        self.clone().into()
    }
}

impl From<TrackSource> for MediaSource {
    fn from(source: TrackSource) -> Self {
        let file = Self::file(source.path);
        match source.section {
            Some(section) => file.with_section(section),
            None => file,
        }
    }
}

impl Library {
    /// The file of `track` and, for a CUE sheet track or a chapter, its span of the
    /// file; `None` if there is no such track. The span's times are exact: the
    /// library stores it in frames at the file's sample rate.
    pub fn track_source(&self, track: TrackId) -> Result<Option<TrackSource>, DbError> {
        self.database().read(|c| track_source(c, track))
    }
}

/// A stored span: the first frame, the end frame and the sample rate they count at.
type StoredSpan = (Option<i64>, Option<i64>, Option<i64>);

fn track_source(c: &Connection, track: TrackId) -> rusqlite::Result<Option<TrackSource>> {
    let found = c
        .prepare_cached(
            "SELECT d.path, f.name, t.section, t.start_frame, t.end_frame, t.sample_rate,
                    t.sub_index
             FROM tracks t
             JOIN files f ON f.id = t.file_id
             JOIN dirs d ON d.id = f.dir_id
             WHERE t.id = ?1",
        )?
        .query_row([sql_track_id(track)], |row| {
            let dir = path_from_blob(row.get(0)?);
            let name = path_from_blob(row.get(1)?);
            let section = TrackSection::from_sql(row.get(2)?);
            let span: StoredSpan = (row.get(3)?, row.get(4)?, row.get(5)?);
            let sub_index: i64 = row.get(6)?;
            Ok((dir.join(name), section, span, sub_index))
        })
        .optional()?;
    Ok(found.map(|(path, section, span, sub_index)| TrackSource {
        path,
        section: section_of(section, span, sub_index),
    }))
}

/// The part of the file a track of `section` with the stored `span` and
/// `sub_index` plays.
pub(crate) fn section_of(
    section: TrackSection,
    span: StoredSpan,
    sub_index: i64,
) -> Option<SourceSection> {
    if section == TrackSection::Subsong {
        let index = u32::try_from(sub_index).ok()?.checked_sub(1)?;
        return Some(SourceSection::Subsong(index));
    }
    span_of(section, span)
}

/// The span a track of `section` with the stored `span` plays.
fn span_of(section: TrackSection, span: StoredSpan) -> Option<SourceSection> {
    if matches!(section, TrackSection::Whole | TrackSection::Subsong) {
        return None;
    }
    let (Some(start), end, Some(rate)) = span else {
        return None;
    };
    let rate = u32::try_from(rate).ok().and_then(SampleRate::new)?;
    let time = |frame: i64| MediaTime::from_frames(u64::try_from(frame).unwrap_or(0), rate);
    Some(SourceSection::Span {
        start: time(start),
        end: end.map(time),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_are_only_for_parts_of_files() {
        let rate = SampleRate::new(44_100).unwrap();
        assert_eq!(
            span_of(TrackSection::Whole, (Some(0), Some(10), Some(44_100))),
            None
        );
        assert_eq!(
            span_of(TrackSection::CueSheet, (Some(588), None, Some(44_100))),
            Some(SourceSection::Span {
                start: MediaTime::from_frames(588, rate),
                end: None,
            })
        );
        assert_eq!(
            span_of(TrackSection::Chapter, (Some(0), Some(44_100), Some(44_100))),
            Some(SourceSection::Span {
                start: MediaTime::from_frames(0, rate),
                end: Some(MediaTime::from_frames(44_100, rate)),
            })
        );
        assert_eq!(span_of(TrackSection::CueSheet, (Some(5), None, None)), None);
        assert_eq!(
            section_of(TrackSection::Subsong, (None, None, Some(44_100)), 3),
            Some(SourceSection::Subsong(2))
        );
        assert_eq!(
            section_of(TrackSection::Subsong, (None, None, None), 0),
            None
        );
        assert_eq!(
            section_of(TrackSection::Chapter, (Some(0), None, Some(44_100)), 4),
            span_of(TrackSection::Chapter, (Some(0), None, Some(44_100)))
        );
        assert_eq!(
            span_of(TrackSection::Chapter, (None, None, Some(48_000))),
            None
        );
    }
}
