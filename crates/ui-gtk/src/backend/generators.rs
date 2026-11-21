// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The front end's types for generator playlists (playlists made by steps) and
//! for the list of tracks that sound alike.
//!
//! A generator's steps are the library's own [`GeneratorSteps`]; the library
//! keeps them in their text form. A generator is listed among the playlists
//! with a [`PlaylistKey`] of its own and the kind
//! [`PlaylistType::Generator`](super::PlaylistType::Generator).

use std::time::SystemTime;

use super::{AlbumKey, ArtistKey, PlaylistKey, TrackKey, TrackRow};

pub use helikodu_service::library::{
    GeneratorSort, GeneratorSource, GeneratorStep, GeneratorSteps,
};

/// A generator playlist, as its page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratorInfo {
    /// The generator.
    pub key: PlaylistKey,
    /// Its name.
    pub name: String,
    /// Its steps.
    pub steps: GeneratorSteps,
    /// Whether it is made again each time it is opened.
    pub on_open: bool,
}

/// One track of a generator's result, with the values its steps may sort by
/// that [`TrackRow`] does not hold.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GeneratorTrack {
    /// The track.
    pub row: TrackRow,
    /// When it last played.
    pub last_played: Option<SystemTime>,
    /// How often it was skipped.
    pub skips: u32,
}

/// Why a step cannot run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepProblem {
    /// The playlist it names is not there.
    MissingPlaylist(String),
    /// Its query cannot be read or cannot run: the message.
    Query(String),
    /// Something else: the message.
    Other(String),
}

/// The tracks of a generator and how they came about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GeneratorRun {
    /// The tracks shown: the new result, or the last one when a step failed.
    pub tracks: Vec<GeneratorTrack>,
    /// How many tracks were left after the source (first) and after each step
    /// that ran; empty when nothing ran.
    pub counts: Vec<usize>,
    /// The step that could not run (the source counted as 0) and why.
    pub problem: Option<(usize, StepProblem)>,
    /// When the tracks shown were made; `None` for steps tried in the editor.
    pub made: Option<SystemTime>,
    /// Whether the tracks were made just now.
    pub fresh: bool,
}

/// What the tracks that sound alike are compared with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimilarSeed {
    /// A track.
    Track(TrackKey),
    /// The tracks of an album.
    Album(AlbumKey),
    /// The tracks of an artist.
    Artist(ArtistKey),
}

/// A track that sounds like the seed.
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarRow {
    /// The track.
    pub row: TrackRow,
    /// How alike it sounds, from 0.5 to 1.
    pub score: f32,
}

/// Why there is no list of tracks that sound alike yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotYet {
    /// The analysis has not reached the seed.
    NotAnalysed,
    /// Too few tracks are analysed for a useful list: how many are.
    TooFew(usize),
    /// The seed's file could not be read.
    Unreadable,
}

/// The tracks that sound alike, or why there are none yet.
#[derive(Debug, Clone, PartialEq)]
pub enum SimilarFound {
    /// The closest tracks, most alike first, none below 0.5; `more` when
    /// another one at 0.5 or above is left.
    Tracks {
        /// The tracks.
        rows: Vec<SimilarRow>,
        /// Whether more are left.
        more: bool,
    },
    /// No list yet.
    NotYet(NotYet),
}

/// The lowest score listed.
pub const SIMILAR_FLOOR: f32 = 0.5;

/// Fewer analysed tracks than this make too short a list.
pub const SIMILAR_LEAST_ANALYSED: usize = 200;

/// Whether `analysed` of `total` tracks are too few to compare with: fewer
/// than 200, or under a tenth of the library.
pub fn too_few_analysed(analysed: usize, total: usize) -> bool {
    analysed < SIMILAR_LEAST_ANALYSED || analysed.saturating_mul(10) < total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_few_follows_the_drawn_rule() {
        assert!(too_few_analysed(199, 300));
        assert!(!too_few_analysed(200, 300));
        assert!(too_few_analysed(212, 18_402));
        assert!(!too_few_analysed(14_950, 18_402));
    }
}
