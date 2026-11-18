// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The A-B loop and the sleep timer as front ends see them.

use std::time::Duration;

use helikodu_model::audio::MediaTime;
use helikodu_model::player::LoopRange;

/// How long the sleep timer's "At once" fades the sound out before it pauses.
pub const SLEEP_FADE: Duration = Duration::from_secs(30);

/// The A-B loop of the playing track: its two points as far as they are set. The
/// section loops once both are set ([`range`](Self::range)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AbLoop {
    /// The start (A).
    pub start: Option<MediaTime>,
    /// The end (B).
    pub end: Option<MediaTime>,
}

impl AbLoop {
    /// The section that loops, once both points are set and the start comes first.
    pub fn range(&self) -> Option<LoopRange> {
        LoopRange::new(self.start?, self.end?)
    }

    /// The loop with its start at `at`; an end at or before it is dropped.
    #[must_use]
    pub fn with_start(self, at: MediaTime) -> Self {
        Self {
            start: Some(at),
            end: self.end.filter(|end| *end > at),
        }
    }

    /// The loop with its end at `at`; without a start before it, the start is the
    /// beginning of the track. `None` when `at` is the beginning.
    #[must_use]
    pub fn with_end(self, at: MediaTime) -> Option<Self> {
        if at.is_zero() {
            return None;
        }
        Some(Self {
            start: Some(
                self.start
                    .filter(|start| *start < at)
                    .unwrap_or(MediaTime::ZERO),
            ),
            end: Some(at),
        })
    }
}

/// When the sleep timer stops playback once its time is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum SleepWhen {
    /// At once, after fading the sound out over the last [`SLEEP_FADE`].
    #[default]
    AtOnce,
    /// When the track playing then ends.
    AfterTrack,
}

/// A running sleep timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SleepTimerStatus {
    /// The time left when this was reported; zero once the time is up and the timer
    /// waits for the track to end.
    pub remaining: Duration,
    /// The length chosen, with what was added since.
    pub length: Duration,
    /// What happens when the time is up.
    pub when: SleepWhen,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_points_keep_their_order() {
        let at = MediaTime::from_millis;
        let ab = AbLoop::default().with_start(at(2_000));
        assert_eq!(ab.range(), None);
        let ab = ab.with_end(at(5_000)).unwrap();
        assert_eq!(ab.range(), LoopRange::new(at(2_000), at(5_000)));
        // A start after the end drops the end; an end before the start drops it.
        assert_eq!(ab.with_start(at(6_000)).end, None);
        let ab = ab.with_end(at(1_000)).unwrap();
        assert_eq!(ab.start, Some(MediaTime::ZERO));
        assert_eq!(AbLoop::default().with_end(MediaTime::ZERO), None);
    }
}
