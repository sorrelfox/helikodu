// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The front end's types for browsing by composer and year, media kinds and the
//! chapters of a file.

use std::time::Duration;

use super::TrackKey;

/// A span of release years, or the albums without a year.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum YearRange {
    /// The years from `first` to `last`, both included.
    Span {
        /// The first year.
        first: i32,
        /// The last year.
        last: i32,
    },
    /// No year.
    Unknown,
}

impl YearRange {
    /// One year, or no year.
    pub fn of(year: Option<i32>) -> Self {
        match year {
            Some(year) => Self::Span {
                first: year,
                last: year,
            },
            None => Self::Unknown,
        }
    }

    /// The decade of `year`, such as 2010 to 2019 for 2014.
    pub fn decade_of(year: i32) -> Self {
        let first = year.div_euclid(10) * 10;
        Self::Span {
            first,
            last: first + 9,
        }
    }

    /// Whether `year` is in the span.
    pub fn contains(self, year: Option<i32>) -> bool {
        match (self, year) {
            (Self::Span { first, last }, Some(year)) => (first..=last).contains(&year),
            (Self::Unknown, None) => true,
            _ => false,
        }
    }
}

/// One release year of the Years list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct YearRow {
    /// The year; `None` for the albums without one.
    pub year: Option<i32>,
    /// The number of its albums.
    pub album_count: usize,
    /// The number of its tracks.
    pub track_count: usize,
}

/// What kind of recording an album is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MediaKind {
    /// Music, the default.
    #[default]
    Music,
    /// An audiobook: it continues where listening stopped and stays out of shuffle.
    Audiobook,
    /// The audio of video files.
    Video,
}

/// One chapter (or CUE sheet track) of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterRow {
    /// The chapter's track.
    pub key: TrackKey,
    /// Its number in the file.
    pub number: u32,
    /// Its title.
    pub title: String,
    /// Where it starts in the file.
    pub start: Duration,
    /// Its length, when known.
    pub length: Option<Duration>,
}
