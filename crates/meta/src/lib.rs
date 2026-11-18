// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Metadata: tags, covers, CUE sheets, lyrics, playlist formats and naming templates.
//!
//! See the crate's README for its place in the workspace.

#![forbid(unsafe_code)]

pub mod covers;
pub mod cue;
pub mod playlists;
pub mod tags;
pub mod worker;

pub use covers::{Cover, CoverHash, CoverLimits, CoverSource, find_cover};
pub use cue::{CueSheet, CueTracks, VirtualTrack, read_chapters, read_cue, read_embedded_cue};
pub use tags::{FileTags, ReadOptions, TagError, TrackTags, read_tags};
