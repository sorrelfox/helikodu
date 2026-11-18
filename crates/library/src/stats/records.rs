// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What is recorded about listening, and what can be read back.

use std::time::{Duration, SystemTime};

use helikodu_model::library::TrackId;

use super::rating::Rating;
use super::rules::ListenEnd;
use crate::catalog::IdentityId;

/// One listen to a track, reported when it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listen {
    /// When playback of the track started.
    pub started_at: SystemTime,
    /// The time actually played, seeks excluded.
    pub played: Duration,
    /// How the listen ended.
    pub end: ListenEnd,
    /// Where in the track playback was when it ended, for tracks that remember it.
    pub position: Option<Duration>,
    /// Where the track was played from (an album, a playlist, a search), for the
    /// history.
    pub context: Option<String>,
}

/// The statistics of a track's identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackStats {
    /// How often it was played.
    pub play_count: u64,
    /// How often it was skipped.
    pub skip_count: u64,
    /// When it was first played: the start of the first listen that counted.
    pub first_played: Option<SystemTime>,
    /// When it was last played: the start of the last listen that counted.
    pub last_played: Option<SystemTime>,
    /// When it was last skipped.
    pub last_skipped: Option<SystemTime>,
    /// Its rating; `None` if it is not rated.
    pub rating: Option<Rating>,
    /// Where listening stopped, for tracks that remember it.
    pub resume_position: Option<Duration>,
}

/// Which history entries to read. The newest come first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryQuery {
    /// Only the listens of this track's identity.
    pub track: Option<TrackId>,
    /// Only listens that started at or after this time.
    pub since: Option<SystemTime>,
    /// Only listens that started before this time.
    pub until: Option<SystemTime>,
    /// Only the listens that counted as plays.
    pub plays_only: bool,
    /// The most entries to return.
    pub limit: usize,
}

impl Default for HistoryQuery {
    fn default() -> Self {
        Self {
            track: None,
            since: None,
            until: None,
            plays_only: false,
            limit: 100,
        }
    }
}

/// Totals of the whole history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HistoryTotals {
    /// The listens that counted as plays.
    pub plays: u64,
    /// The listens that did not.
    pub skips: u64,
    /// When the oldest listen started.
    pub first: Option<SystemTime>,
}

/// One listen in the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The identity that was listened to.
    pub identity: IdentityId,
    /// A track of the library with that identity, if it is still there.
    pub track: Option<TrackId>,
    /// When playback started.
    pub started_at: SystemTime,
    /// The time actually played.
    pub played: Duration,
    /// Whether the listen counted as a play.
    pub counted: bool,
    /// Where the track was played from.
    pub context: Option<String>,
}
