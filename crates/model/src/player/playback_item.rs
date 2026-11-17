// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the engine is asked to play.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::library::MediaSource;

/// Identifies one request to play something. The sender chooses it, and the engine
/// repeats it in every event about that item, so that the same file queued twice can
/// be told apart.
///
/// Serialized as the plain number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(u64);

impl ItemId {
    /// The identifier with the number `id`.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A source for the engine to play, with the identifier its events will carry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaybackItem {
    /// The identifier chosen by the sender.
    pub id: ItemId,
    /// What to play.
    pub source: MediaSource,
}

impl PlaybackItem {
    /// An item from its two parts.
    pub const fn new(id: ItemId, source: MediaSource) -> Self {
        Self { id, source }
    }
}
