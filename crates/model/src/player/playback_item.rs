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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybackItem {
    /// The identifier chosen by the sender.
    pub id: ItemId,
    /// What to play.
    pub source: MediaSource,
    /// The source's loudness values, for loudness normalization. None are known by
    /// default.
    #[serde(default, skip_serializing_if = "Loudness::is_unknown")]
    pub loudness: Loudness,
    /// Whether the item plays as part of its album: its neighbours in the queue come
    /// from the same album. The automatic loudness mode then applies the album gain.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub album_context: bool,
}

impl PlaybackItem {
    /// An item from its two parts, with no loudness values and outside an album.
    pub const fn new(id: ItemId, source: MediaSource) -> Self {
        Self {
            id,
            source,
            loudness: Loudness::UNKNOWN,
            album_context: false,
        }
    }

    /// The same item with the loudness values `loudness`.
    #[must_use]
    pub fn with_loudness(self, loudness: Loudness, album_context: bool) -> Self {
        Self {
            loudness,
            album_context,
            ..self
        }
    }
}

/// The loudness values of a track, from its tags or a scan.
///
/// Gains are in dB relative to the ReplayGain 2.0 reference of −18 LUFS; Opus R128
/// gains (relative to −23 LUFS) are converted before they get here. Peaks are linear,
/// 1.0 being full scale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Loudness {
    /// The track gain in dB.
    pub track_gain_db: Option<f64>,
    /// The track's peak.
    pub track_peak: Option<f64>,
    /// The album gain in dB.
    pub album_gain_db: Option<f64>,
    /// The album's peak.
    pub album_peak: Option<f64>,
}

impl Loudness {
    /// No values known.
    pub const UNKNOWN: Self = Self {
        track_gain_db: None,
        track_peak: None,
        album_gain_db: None,
        album_peak: None,
    };

    /// Whether no value is known.
    pub fn is_unknown(&self) -> bool {
        *self == Self::UNKNOWN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudness_is_optional_in_json() {
        let item = PlaybackItem::new(ItemId::new(3), MediaSource::file("/a.flac"));
        let json = serde_json::to_string(&item).unwrap();
        assert!(!json.contains("loudness"), "{json}");
        assert_eq!(serde_json::from_str::<PlaybackItem>(&json).unwrap(), item);

        let loudness = Loudness {
            track_gain_db: Some(-6.5),
            album_peak: Some(0.98),
            ..Loudness::UNKNOWN
        };
        let item = item.with_loudness(loudness, true);
        let json = serde_json::to_string(&item).unwrap();
        let back: PlaybackItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back, item);
        assert!(back.album_context);
        assert!(!back.loudness.is_unknown());
    }
}
