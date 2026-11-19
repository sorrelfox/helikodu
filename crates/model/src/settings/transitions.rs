// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fades and crossfades: the `[playback.transitions]` table.

use std::ops::RangeInclusive;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{SettingsError, check_range};

/// How playback moves from one sound to the next: the crossfade between tracks and
/// the fades around pause, stop, seek and a skip. The `[playback.transitions]`
/// table.
///
/// ```toml
/// [playback.transitions]
/// crossfade = false            # between tracks that follow each other
/// crossfade_ms = 5000
/// skip_crossfade = false       # when the listener skips to another track
/// skip_crossfade_ms = 2000
/// keep_albums_gapless = true   # never crossfade consecutive tracks of one album
/// pause_fade_ms = 100          # pause and resume
/// stop_fade_ms = 200
/// seek_fade_ms = 50
/// skip_fade_ms = 100           # when the skip is not crossfaded
/// crossfade_from_analysis = true  # overlap chosen from the track analysis
/// trim_silence = false         # leading and trailing silence between tracks
/// skip_silence = false         # long silence inside a track
/// skip_silence_min_s = 10
/// ```
///
/// Nothing is crossfaded, trimmed or skipped on a bit-perfect path in exclusive
/// mode, whatever these say, and tracks that follow each other on one album are
/// never trimmed while albums are kept gapless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct TransitionSettings {
    /// Whether a track that ends is crossfaded into the next one.
    pub crossfade: bool,
    /// The length of that crossfade, in milliseconds (100 to 12 000).
    pub crossfade_ms: u32,
    /// Whether skipping to another track crossfades into it.
    pub skip_crossfade: bool,
    /// The length of the crossfade of a skip, in milliseconds (100 to 12 000).
    pub skip_crossfade_ms: u32,
    /// Whether tracks that follow each other on one album always join without a
    /// crossfade, so that gapless albums stay intact.
    pub keep_albums_gapless: bool,
    /// The fade out before a pause and the fade in after it, in milliseconds (0 to
    /// 2000; 0 is the shortest fade that does not click).
    pub pause_fade_ms: u32,
    /// The fade out before playback stops, in milliseconds (0 to 2000).
    pub stop_fade_ms: u32,
    /// The fade out before a seek and the fade in after it, in milliseconds (0 to
    /// 2000).
    pub seek_fade_ms: u32,
    /// The fade out when the listener skips to another track without a crossfade,
    /// in milliseconds (0 to 2000).
    pub skip_fade_ms: u32,
    /// Whether the crossfade between tracks takes its overlap from the track
    /// analysis when both tracks have one: it starts where the outgoing track
    /// begins to fade out (or where its sound ends), at most the crossfade's
    /// length before the end of its sound, and the incoming track starts with its
    /// sound. Only matters while the crossfade between tracks is on.
    pub crossfade_from_analysis: bool,
    /// Whether the silence before the sound of a track and after it is left out
    /// where tracks join and when a track starts from its beginning.
    pub trim_silence: bool,
    /// Whether long silent stretches inside a track (before a hidden track, for
    /// example) are skipped.
    pub skip_silence: bool,
    /// How long a silent stretch inside a track must be to be skipped, in seconds
    /// (5 to 600).
    pub skip_silence_min_s: u32,
}

impl Default for TransitionSettings {
    /// Crossfades off; fades of 100 ms (pause and resume), 200 ms (stop), 50 ms
    /// (seek) and 100 ms (skip).
    fn default() -> Self {
        Self {
            crossfade: false,
            crossfade_ms: 5000,
            skip_crossfade: false,
            skip_crossfade_ms: 2000,
            keep_albums_gapless: true,
            pause_fade_ms: 100,
            stop_fade_ms: 200,
            seek_fade_ms: 50,
            skip_fade_ms: 100,
            crossfade_from_analysis: true,
            trim_silence: false,
            skip_silence: false,
            skip_silence_min_s: 10,
        }
    }
}

impl TransitionSettings {
    /// The range of both crossfade lengths, in milliseconds.
    pub const CROSSFADE_RANGE_MS: RangeInclusive<u32> = 100..=12_000;
    /// The range of the fade lengths, in milliseconds.
    pub const FADE_RANGE_MS: RangeInclusive<u32> = 0..=2000;
    /// The range of the shortest silent stretch that is skipped, in seconds.
    pub const SKIP_SILENCE_RANGE_S: RangeInclusive<u32> = 5..=600;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        for (key, value) in [
            ("playback.transitions.crossfade_ms", self.crossfade_ms),
            (
                "playback.transitions.skip_crossfade_ms",
                self.skip_crossfade_ms,
            ),
        ] {
            check_range(key, value, Self::CROSSFADE_RANGE_MS)?;
        }
        for (key, value) in [
            ("playback.transitions.pause_fade_ms", self.pause_fade_ms),
            ("playback.transitions.stop_fade_ms", self.stop_fade_ms),
            ("playback.transitions.seek_fade_ms", self.seek_fade_ms),
            ("playback.transitions.skip_fade_ms", self.skip_fade_ms),
        ] {
            check_range(key, value, Self::FADE_RANGE_MS)?;
        }
        check_range(
            "playback.transitions.skip_silence_min_s",
            self.skip_silence_min_s,
            Self::SKIP_SILENCE_RANGE_S,
        )
    }

    /// The length of the crossfade between tracks that follow each other, when it
    /// is on.
    pub fn track_crossfade(&self) -> Option<Duration> {
        self.crossfade.then(|| millis(self.crossfade_ms))
    }

    /// The length of the crossfade of a skip, when it is on.
    pub fn skip_crossfade(&self) -> Option<Duration> {
        self.skip_crossfade.then(|| millis(self.skip_crossfade_ms))
    }

    /// The fade out before a pause and the fade in after it.
    pub fn pause_fade(&self) -> Duration {
        millis(self.pause_fade_ms)
    }

    /// The fade out before playback stops.
    pub fn stop_fade(&self) -> Duration {
        millis(self.stop_fade_ms)
    }

    /// The fade out before a seek and the fade in after it.
    pub fn seek_fade(&self) -> Duration {
        millis(self.seek_fade_ms)
    }

    /// The fade out of a skip that is not crossfaded.
    pub fn skip_fade(&self) -> Duration {
        millis(self.skip_fade_ms)
    }

    /// Whether the crossfade between tracks is on and takes its overlap from the
    /// track analysis.
    pub fn analysed_crossfade(&self) -> bool {
        self.crossfade && self.crossfade_from_analysis
    }

    /// The shortest silent stretch inside a track that is skipped, when skipping is
    /// on.
    pub fn silence_skip(&self) -> Option<Duration> {
        self.skip_silence
            .then(|| Duration::from_secs(u64::from(self.skip_silence_min_s)))
    }
}

fn millis(ms: u32) -> Duration {
    Duration::from_millis(u64::from(ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn the_design_defaults() {
        let t = TransitionSettings::default();
        assert_eq!(t.track_crossfade(), None);
        assert_eq!(t.skip_crossfade(), None);
        assert!(t.keep_albums_gapless);
        assert_eq!(t.pause_fade(), Duration::from_millis(100));
        assert_eq!(t.stop_fade(), Duration::from_millis(200));
        assert_eq!(t.seek_fade(), Duration::from_millis(50));
        assert_eq!(t.skip_fade(), Duration::from_millis(100));
        assert!(t.crossfade_from_analysis && !t.analysed_crossfade());
        assert!(!t.trim_silence);
        assert_eq!(t.silence_skip(), None);
        assert_eq!(t.skip_silence_min_s, 10);
        assert_eq!(t.validate(), Ok(()));
    }

    #[test]
    fn older_files_get_the_analysis_defaults() {
        let old: Settings = toml::from_str(
            "[playback.transitions]\ncrossfade = true\ncrossfade_ms = 4000\n\
             keep_albums_gapless = true\n",
        )
        .unwrap();
        let t = old.playback.transitions;
        assert!(t.analysed_crossfade());
        assert!(!t.trim_silence && !t.skip_silence);
        let new: Settings = toml::from_str(
            "[playback.transitions]\ntrim_silence = true\nskip_silence = true\n\
             skip_silence_min_s = 30\ncrossfade_from_analysis = false\n",
        )
        .unwrap();
        let t = new.playback.transitions;
        assert!(t.trim_silence && !t.crossfade_from_analysis);
        assert_eq!(t.silence_skip(), Some(Duration::from_secs(30)));
        let mut bad = t;
        bad.skip_silence_min_s = 4;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn a_table_of_the_playback_settings() {
        let mut settings = Settings::default();
        settings.playback.transitions.crossfade = true;
        settings.playback.transitions.crossfade_ms = 3000;
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains("[playback.transitions]"), "{text}");
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(
            back.playback.transitions.track_crossfade(),
            Some(Duration::from_secs(3))
        );
        let partial: Settings =
            toml::from_str("[playback.transitions]\nskip_crossfade = true\n").unwrap();
        assert_eq!(
            partial.playback.transitions.skip_crossfade(),
            Some(Duration::from_secs(2))
        );
        assert_eq!(partial.playback.transitions.stop_fade_ms, 200);
    }

    #[test]
    fn ranges_are_checked() {
        for change in [
            |t: &mut TransitionSettings| t.crossfade_ms = 50,
            |t: &mut TransitionSettings| t.crossfade_ms = 12_001,
            |t: &mut TransitionSettings| t.skip_crossfade_ms = 0,
            |t: &mut TransitionSettings| t.pause_fade_ms = 2001,
            |t: &mut TransitionSettings| t.seek_fade_ms = 5000,
        ] {
            let mut t = TransitionSettings::default();
            change(&mut t);
            assert!(t.validate().is_err(), "{t:?}");
        }
        let mut settings = Settings::default();
        settings.playback.transitions.stop_fade_ms = 3000;
        assert_eq!(
            settings.validate().unwrap_err().key,
            "playback.transitions.stop_fade_ms"
        );
    }
}
