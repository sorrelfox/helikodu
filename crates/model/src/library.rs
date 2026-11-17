// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library records: what a track is and where its audio comes from.

mod media_source;
mod track_id;
mod track_ref;

pub use media_source::{MediaSource, SourceSection};
pub use track_id::TrackId;
pub use track_ref::TrackRef;
