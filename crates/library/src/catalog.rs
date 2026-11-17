// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The catalogue: tracks, the files they come from, their people, albums and genres, and
//! the identity that keeps a track's statistics across moves.
//!
//! A track's identity is found by its fingerprint: the quick hash of its file and its
//! audio properties. A file that disappears and comes back elsewhere with the same size
//! and quick hash is the same file; its row moves, so the track and its identity stay.
//! A track that left the library and comes back with the same fingerprint gets its old
//! identity back, with its statistics, rating and history.

mod fingerprint;
mod ids;
mod track_record;
pub(crate) mod write;

pub use fingerprint::TrackFingerprint;
pub use ids::{IdentityId, RootId};
pub(crate) use ids::{sql_track_id, track_id_from_sql};
pub use track_record::{AudioProperties, FileStamp, TrackKind, TrackRecord, TrackTags};
pub use write::{TrackChange, TrackWrite};
