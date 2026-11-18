// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! How one track takes part in playback: the per-track flags of the tag editor's
//! Playback group.

use serde::{Deserialize, Serialize};

/// The per-track playback settings. They are kept in the library, not in the file,
/// and apply at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackFlags {
    /// "Leave out when shuffling": while any shuffle is on, automatic playback and
    /// "Next" pass over the track.
    pub skip_in_shuffle: bool,
    /// "Play only when I start it myself": automatic playback, "Next" and
    /// "Previous" pass over the track; it plays when it is started directly.
    pub manual_only: bool,
    /// "Volume for this track", in half decibels from −24 (−12 dB) to +24 (+12 dB).
    pub volume_half_db: i8,
}

impl TrackFlags {
    /// The quietest volume adjustment, in half decibels.
    pub const MIN_VOLUME_HALF_DB: i8 = -24;
    /// The loudest volume adjustment, in half decibels.
    pub const MAX_VOLUME_HALF_DB: i8 = 24;

    const SKIP_IN_SHUFFLE: i64 = 1;
    const MANUAL_ONLY: i64 = 2;
    const VOLUME_SHIFT: u32 = 8;

    /// Whether every flag is off and the volume unchanged.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The volume adjustment in dB.
    pub fn volume_db(&self) -> f64 {
        f64::from(
            self.volume_half_db
                .clamp(Self::MIN_VOLUME_HALF_DB, Self::MAX_VOLUME_HALF_DB),
        ) / 2.0
    }

    /// The same flags with the volume adjustment nearest to `db`, within the range.
    #[must_use]
    pub fn with_volume_db(self, db: f64) -> Self {
        let half = (db * 2.0).round().clamp(
            f64::from(Self::MIN_VOLUME_HALF_DB),
            f64::from(Self::MAX_VOLUME_HALF_DB),
        );
        Self {
            // In range by the clamp above.
            #[allow(clippy::cast_possible_truncation)]
            volume_half_db: half as i8,
            ..self
        }
    }

    /// The flags packed into one integer, as the library stores them: bit 0 skip in
    /// shuffle, bit 1 manual only, bits 8 to 15 the volume as a signed byte.
    pub fn to_bits(self) -> i64 {
        let mut bits = 0;
        if self.skip_in_shuffle {
            bits |= Self::SKIP_IN_SHUFFLE;
        }
        if self.manual_only {
            bits |= Self::MANUAL_ONLY;
        }
        bits | (i64::from(self.volume_half_db.to_ne_bytes()[0]) << Self::VOLUME_SHIFT)
    }

    /// The flags of [`to_bits`](Self::to_bits); unknown bits are ignored.
    pub fn from_bits(bits: i64) -> Self {
        let byte = ((bits >> Self::VOLUME_SHIFT) & 0xff) as u8;
        Self {
            skip_in_shuffle: bits & Self::SKIP_IN_SHUFFLE != 0,
            manual_only: bits & Self::MANUAL_ONLY != 0,
            volume_half_db: i8::from_ne_bytes([byte])
                .clamp(Self::MIN_VOLUME_HALF_DB, Self::MAX_VOLUME_HALF_DB),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_round_trip_through_bits() {
        for skip in [false, true] {
            for manual in [false, true] {
                for half in TrackFlags::MIN_VOLUME_HALF_DB..=TrackFlags::MAX_VOLUME_HALF_DB {
                    let flags = TrackFlags {
                        skip_in_shuffle: skip,
                        manual_only: manual,
                        volume_half_db: half,
                    };
                    assert_eq!(TrackFlags::from_bits(flags.to_bits()), flags);
                }
            }
        }
        assert_eq!(TrackFlags::from_bits(0), TrackFlags::default());
        assert!(TrackFlags::default().is_default());
    }

    #[test]
    fn volume_is_kept_in_half_decibels_within_the_range() {
        let flags = TrackFlags::default().with_volume_db(-3.4);
        assert_eq!(flags.volume_half_db, -7);
        assert_eq!(flags.volume_db(), -3.5);
        assert_eq!(TrackFlags::default().with_volume_db(40.0).volume_db(), 12.0);
    }
}
