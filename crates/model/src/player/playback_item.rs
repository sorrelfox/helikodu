// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the engine is asked to play.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::analysis::TrackAnalysis;
use crate::library::MediaSource;
use crate::settings::EqualizerSettings;

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
    /// The album the item belongs to, as a key the sender chooses (for example its
    /// title and album artists): items with the same key are tracks of one album.
    /// The engine joins consecutive items of one album without a crossfade when the
    /// settings keep albums gapless. `None` when the item has no album.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    /// The audio stream to play, by its index in the file, for files with several;
    /// `None` for the one the file marks as default, or the best one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_stream: Option<u32>,
    /// The volume adjustment of this track alone ("Volume for this track"), in dB,
    /// applied with the loudness gain. Zero by default.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub track_volume_db: f64,
    /// The track analysis of the item (where its sound starts and ends, its silent
    /// stretches and its loudness envelope), when the library has a current one.
    /// Transitions, silence trimming and the leveller use it. Not serialized: it
    /// is looked up again when the item is made.
    #[serde(skip)]
    pub analysis: Option<Arc<TrackAnalysis>>,
    /// The track's own equalizer, stored in the library, which replaces the
    /// equalizer of the settings while it plays (when `dsp.track_equalizer` is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equalizer: Option<Box<EqualizerSettings>>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

impl PlaybackItem {
    /// An item from its two parts, with no loudness values and outside an album.
    pub const fn new(id: ItemId, source: MediaSource) -> Self {
        Self {
            id,
            source,
            loudness: Loudness::UNKNOWN,
            album_context: false,
            album: None,
            audio_stream: None,
            track_volume_db: 0.0,
            analysis: None,
            equalizer: None,
        }
    }

    /// The same item with the track's own equalizer `equalizer` (see
    /// [`equalizer`](Self::equalizer)).
    #[must_use]
    pub fn with_equalizer(self, equalizer: Option<EqualizerSettings>) -> Self {
        Self {
            equalizer: equalizer.map(Box::new),
            ..self
        }
    }

    /// The same item with the track analysis `analysis` (see
    /// [`analysis`](Self::analysis)).
    #[must_use]
    pub fn with_analysis(self, analysis: Option<Arc<TrackAnalysis>>) -> Self {
        Self { analysis, ..self }
    }

    /// The same item with the volume adjustment `db` (see
    /// [`track_volume_db`](Self::track_volume_db)).
    #[must_use]
    pub fn with_track_volume(self, db: f64) -> Self {
        Self {
            track_volume_db: db,
            ..self
        }
    }

    /// The same item playing the audio stream with the index `stream` of the file
    /// (see [`audio_stream`](Self::audio_stream)).
    #[must_use]
    pub fn with_audio_stream(self, stream: Option<u32>) -> Self {
        Self {
            audio_stream: stream,
            ..self
        }
    }

    /// The same item on the album `album` (see [`album`](Self::album)).
    #[must_use]
    pub fn with_album(self, album: Option<String>) -> Self {
        Self { album, ..self }
    }

    /// Whether `self` and `other` are tracks of one album.
    pub fn same_album(&self, other: &Self) -> bool {
        self.album.is_some() && self.album == other.album
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
    /// Whether the peaks are true peaks (measured by the library's loudness
    /// scanner) rather than sample peaks (as ReplayGain tags carry them).
    pub true_peak: bool,
}

impl Loudness {
    /// No values known.
    pub const UNKNOWN: Self = Self {
        track_gain_db: None,
        track_peak: None,
        album_gain_db: None,
        album_peak: None,
        true_peak: false,
    };

    /// Whether no value is known.
    pub fn is_unknown(&self) -> bool {
        self.track_gain_db.is_none()
            && self.track_peak.is_none()
            && self.album_gain_db.is_none()
            && self.album_peak.is_none()
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

    #[test]
    fn only_items_with_the_same_album_share_one() {
        let a = PlaybackItem::new(ItemId::new(1), MediaSource::file("/a.flac"));
        let b = PlaybackItem::new(ItemId::new(2), MediaSource::file("/b.flac"));
        assert!(!a.same_album(&b));
        let a = a.with_album(Some("Album\u{1f}Artist".into()));
        assert!(!a.same_album(&b));
        let b = b.with_album(a.album.clone());
        assert!(a.same_album(&b));
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(serde_json::from_str::<PlaybackItem>(&json).unwrap(), b);
    }
}
