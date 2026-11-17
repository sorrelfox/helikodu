// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Play statistics, ratings and the listening history.
//!
//! Everything here belongs to a track's identity, not to its path, so it survives moves
//! and renames. Whether a listen counts as a play or a skip is decided by
//! [`PlayRules::classify`], a pure function of the time played, the track's length and
//! how the listen ended.

mod rating;
mod records;
mod rules;
pub(crate) mod store;

pub use rating::Rating;
pub use records::{HistoryEntry, HistoryQuery, Listen, TrackStats};
pub use rules::{ListenEnd, ListenOutcome, PlayRules};
