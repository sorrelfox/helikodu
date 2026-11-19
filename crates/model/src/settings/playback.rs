// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The playback settings: the `[playback]` table.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{DecoderSettings, SettingsError, TransitionSettings, check_range};

/// How the player behaves: the `[playback]` table.
///
/// ```toml
/// [playback]
/// previous_restarts_after_s = 3   # 0: "Previous" always goes to the track before
/// seek_step_s = 5
/// play_count_percent = 50         # a play counts after half the track …
/// play_count_minutes = 4          # … or four minutes, whichever comes first
/// play_context = "replace"        # replace | append
/// on_start = "restore"            # restore | empty
/// resume_long_tracks = true
/// long_track_minutes = 20
/// keep_awake = true
/// pause_on_lock = false
/// pause_before_sleep = true
///
/// [playback.transitions]          # crossfades and fades, see `TransitionSettings`
/// crossfade = false
///
/// [playback.decoders]             # tracker, game music, SID, MIDI, see `DecoderSettings`
/// loops = 2
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackSettings {
    /// After how many seconds of a track "Previous" restarts it instead of going to
    /// the track before (0 to 30); 0 always goes to the track before.
    pub previous_restarts_after_s: u32,
    /// How far the seek shortcuts move, in seconds (1 to 60; the settings page offers
    /// 2, 5, 10, 15 and 30).
    pub seek_step_s: u32,
    /// The share of a track, in percent (1 to 100), after which a listen counts as a
    /// play.
    pub play_count_percent: u8,
    /// The minutes (1 to 60) after which a listen counts as a play however long the
    /// track is.
    pub play_count_minutes: u32,
    /// Whether playing an album, playlist or other context replaces the queue or is
    /// added to its end.
    pub play_context: ContextAction,
    /// What the queue holds when the program starts.
    pub on_start: StartAction,
    /// Whether long tracks and audiobooks continue where listening stopped.
    pub resume_long_tracks: bool,
    /// The length in minutes (1 to 600) above which a track counts as long.
    pub long_track_minutes: u32,
    /// Whether the computer is kept from going to sleep while music plays.
    pub keep_awake: bool,
    /// Whether playback pauses when the screen locks (applied from a later version).
    pub pause_on_lock: bool,
    /// Whether playback pauses before the computer goes to sleep (applied from a later
    /// version).
    pub pause_before_sleep: bool,
    /// The crossfades and fades: the `[playback.transitions]` table.
    pub transitions: TransitionSettings,
    /// The long-tail decoders: the `[playback.decoders]` table.
    pub decoders: DecoderSettings,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            previous_restarts_after_s: 3,
            seek_step_s: 5,
            play_count_percent: 50,
            play_count_minutes: 4,
            play_context: ContextAction::Replace,
            on_start: StartAction::Restore,
            resume_long_tracks: true,
            long_track_minutes: 20,
            keep_awake: true,
            pause_on_lock: false,
            pause_before_sleep: true,
            transitions: TransitionSettings::default(),
            decoders: DecoderSettings::default(),
        }
    }
}

impl PlaybackSettings {
    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_range(
            "playback.previous_restarts_after_s",
            self.previous_restarts_after_s,
            0..=30,
        )?;
        check_range("playback.seek_step_s", self.seek_step_s, 1..=60)?;
        check_range(
            "playback.play_count_percent",
            self.play_count_percent,
            1..=100,
        )?;
        check_range(
            "playback.play_count_minutes",
            self.play_count_minutes,
            1..=60,
        )?;
        check_range(
            "playback.long_track_minutes",
            self.long_track_minutes,
            1..=600,
        )?;
        self.transitions.validate()?;
        self.decoders.validate()
    }

    /// How much of a track must have played for "Previous" to restart it, or `None`
    /// when it always goes to the track before.
    pub fn previous_restart(&self) -> Option<Duration> {
        (self.previous_restarts_after_s > 0)
            .then(|| Duration::from_secs(self.previous_restarts_after_s.into()))
    }

    /// The seek step.
    pub fn seek_step(&self) -> Duration {
        Duration::from_secs(self.seek_step_s.into())
    }

    /// The time played that makes a play however long the track is.
    pub fn play_count_time(&self) -> Duration {
        Duration::from_secs(u64::from(self.play_count_minutes) * 60)
    }

    /// The length above which a track counts as long.
    pub fn long_track(&self) -> Duration {
        Duration::from_secs(u64::from(self.long_track_minutes) * 60)
    }
}

/// What playing a context does to the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextAction {
    /// The queue is replaced.
    #[default]
    Replace,
    /// The tracks are added to the end of the queue, and the first of them plays.
    Append,
}

/// What the queue holds when the program starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartAction {
    /// The last session: queue, position, volume and modes. Playback continues if
    /// it was playing when the program ended, and stays paused otherwise.
    #[default]
    Restore,
    /// An empty queue.
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn the_design_defaults() {
        let playback = PlaybackSettings::default();
        assert_eq!(playback.previous_restart(), Some(Duration::from_secs(3)));
        assert_eq!(playback.seek_step(), Duration::from_secs(5));
        assert_eq!(playback.play_count_time(), Duration::from_secs(240));
        assert_eq!(playback.long_track(), Duration::from_secs(1200));
        assert_eq!(playback.on_start, StartAction::Restore);
        assert_eq!(playback.validate(), Ok(()));
    }

    #[test]
    fn written_in_kebab_case() {
        let mut settings = Settings::default();
        settings.playback.play_context = ContextAction::Append;
        settings.playback.on_start = StartAction::Empty;
        settings.playback.previous_restarts_after_s = 0;
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains("play_context = \"append\""), "{text}");
        assert!(text.contains("on_start = \"empty\""), "{text}");
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back.playback.previous_restart(), None);
    }

    #[test]
    fn ranges_are_checked() {
        for change in [
            |p: &mut PlaybackSettings| p.seek_step_s = 0,
            |p: &mut PlaybackSettings| p.play_count_percent = 0,
            |p: &mut PlaybackSettings| p.play_count_percent = 101,
            |p: &mut PlaybackSettings| p.play_count_minutes = 61,
            |p: &mut PlaybackSettings| p.previous_restarts_after_s = 31,
            |p: &mut PlaybackSettings| p.long_track_minutes = 0,
        ] {
            let mut playback = PlaybackSettings::default();
            change(&mut playback);
            assert!(playback.validate().is_err(), "{playback:?}");
        }
    }
}
