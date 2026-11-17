// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What counts as a play and a skip, and which tracks remember where listening stopped.

use std::time::Duration;

use crate::catalog::TrackKind;

/// How a listen ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListenEnd {
    /// The track played to its end.
    Finished,
    /// The user moved to another track: next, previous, or a track chosen elsewhere.
    Skipped,
    /// Playback stopped without moving on: stop, quitting, an error, a device lost.
    Stopped,
}

/// What a listen counts as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListenOutcome {
    /// Enough was played: the play count goes up.
    Play,
    /// The user moved on before the threshold: the skip count goes up.
    Skip,
    /// Too little was played, and the user did not move on: nothing is counted.
    Partial,
}

/// The thresholds for plays and resume positions. The defaults are those of the
/// settings: half the track or four minutes, whichever comes first; resume positions
/// for tracks over 20 minutes and for audiobooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlayRules {
    /// The share of the track, in percent (1 to 100), that makes a play.
    pub play_percent: u8,
    /// The time played that makes a play however long the track is.
    pub play_time: Duration,
    /// Tracks longer than this remember where listening stopped.
    pub resume_after: Duration,
}

impl Default for PlayRules {
    fn default() -> Self {
        Self {
            play_percent: 50,
            play_time: Duration::from_secs(4 * 60),
            resume_after: Duration::from_secs(20 * 60),
        }
    }
}

impl PlayRules {
    /// The time that must be played for a listen of a track of `length` to count as a
    /// play. Without a known length it is [`play_time`](Self::play_time).
    pub fn play_threshold(&self, length: Option<Duration>) -> Duration {
        let percent = u32::from(self.play_percent.clamp(1, 100));
        match length {
            Some(length) if !length.is_zero() => (length * percent / 100).min(self.play_time),
            _ => self.play_time,
        }
    }

    /// What a listen counts as. `played` is the time actually played, seeks excluded.
    pub fn classify(
        &self,
        played: Duration,
        length: Option<Duration>,
        end: ListenEnd,
    ) -> ListenOutcome {
        if !played.is_zero() && played >= self.play_threshold(length) {
            ListenOutcome::Play
        } else if end == ListenEnd::Skipped {
            ListenOutcome::Skip
        } else {
            ListenOutcome::Partial
        }
    }

    /// Whether a track remembers where listening stopped (audiobooks, and tracks longer
    /// than [`resume_after`](Self::resume_after)).
    pub fn keeps_resume_position(&self, length: Option<Duration>, kind: TrackKind) -> bool {
        kind == TrackKind::Audiobook || length.is_some_and(|length| length > self.resume_after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn half_the_track_or_four_minutes() {
        let rules = PlayRules::default();
        assert_eq!(rules.play_threshold(Some(secs(180))), secs(90));
        assert_eq!(rules.play_threshold(Some(secs(480))), secs(240));
        assert_eq!(rules.play_threshold(Some(secs(3600))), secs(240));
        assert_eq!(rules.play_threshold(None), secs(240));
        assert_eq!(rules.play_threshold(Some(Duration::ZERO)), secs(240));
    }

    #[test]
    fn plays_skips_and_partial_listens() {
        let rules = PlayRules::default();
        let song = Some(secs(200));
        assert_eq!(
            rules.classify(secs(100), song, ListenEnd::Skipped),
            ListenOutcome::Play
        );
        assert_eq!(
            rules.classify(secs(99), song, ListenEnd::Skipped),
            ListenOutcome::Skip
        );
        assert_eq!(
            rules.classify(secs(99), song, ListenEnd::Stopped),
            ListenOutcome::Partial
        );
        // Seeking to the end does not make a play.
        assert_eq!(
            rules.classify(secs(10), song, ListenEnd::Finished),
            ListenOutcome::Partial
        );
        assert_eq!(
            rules.classify(secs(200), song, ListenEnd::Finished),
            ListenOutcome::Play
        );
        // Stopping after the threshold still counts.
        assert_eq!(
            rules.classify(secs(240), Some(secs(3600)), ListenEnd::Stopped),
            ListenOutcome::Play
        );
        assert_eq!(
            rules.classify(Duration::ZERO, Some(Duration::ZERO), ListenEnd::Finished),
            ListenOutcome::Partial
        );
        assert_eq!(
            rules.classify(Duration::ZERO, song, ListenEnd::Skipped),
            ListenOutcome::Skip
        );
    }

    #[test]
    fn configurable_threshold() {
        let rules = PlayRules {
            play_percent: 80,
            play_time: secs(600),
            ..PlayRules::default()
        };
        assert_eq!(rules.play_threshold(Some(secs(300))), secs(240));
        assert_eq!(
            rules.classify(secs(239), Some(secs(300)), ListenEnd::Skipped),
            ListenOutcome::Skip
        );
    }

    #[test]
    fn resume_positions_for_long_tracks_and_audiobooks() {
        let rules = PlayRules::default();
        assert!(!rules.keeps_resume_position(Some(secs(20 * 60)), TrackKind::Music));
        assert!(rules.keeps_resume_position(Some(secs(20 * 60 + 1)), TrackKind::Music));
        assert!(rules.keeps_resume_position(Some(secs(60)), TrackKind::Audiobook));
        assert!(!rules.keeps_resume_position(None, TrackKind::Podcast));
    }
}
