// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings of the long-tail decoders: the `[playback.decoders]` table.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{SettingsError, check_range};

/// The largest file that is ever loaded into memory whole, in bytes (512 MiB).
pub const MEMORY_BUFFER_LIMIT: u64 = 512 << 20;

/// How synthesized and looping formats (tracker modules, game music, SID tunes,
/// game audio streams, MIDI files) are played, which SoundFont MIDI files use, and
/// whether files are read into memory before they play: the `[playback.decoders]`
/// table.
///
/// ```toml
/// [playback.decoders]
/// soundfont = "/usr/share/soundfonts/FluidR3_GM.sf2"   # not set: look for one
/// sid_song_lengths = "/home/me/C64Music/DOCUMENTS/Songlengths.md5"
/// sid_roms = "/home/me/c64-roms"   # a folder with kernal, basic and chargen
/// loops = 2                        # how often a looping tune plays its loop
/// fade_ms = 8000                   # the fade after the last loop
/// default_length_s = 150           # tunes whose length nothing states
/// tracker_stereo_separation = 100  # percent, 0 to 200
/// tracker_interpolation = "default"   # default | none | linear | cubic | sinc
/// load_into_memory = false         # read each file whole before it plays
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DecoderSettings {
    /// The SoundFont 2 file MIDI files are played with. Without one the player uses a
    /// SoundFont installed on the system, if it finds one; otherwise MIDI files need
    /// one to be chosen.
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::path_serde::opt_text"
    )]
    pub soundfont: Option<PathBuf>,
    /// The song-length database of the High Voltage SID Collection
    /// (`Songlengths.md5`), for the lengths of SID tunes.
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::path_serde::opt_text"
    )]
    pub sid_song_lengths: Option<PathBuf>,
    /// A folder with C64 ROM images (KERNAL, BASIC, character ROM), which some SID
    /// tunes need. Without it a built-in replacement KERNAL is used.
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::path_serde::opt_text"
    )]
    pub sid_roms: Option<PathBuf>,
    /// How many times a tune with a loop plays the looped part before the fade (1 to
    /// 10).
    pub loops: u32,
    /// The fade at the end of a looping tune or of one played for the default length,
    /// in milliseconds (0 to 30 000).
    pub fade_ms: u32,
    /// How long a tune plays when nothing states its length, before the fade, in
    /// seconds (10 to 3600).
    pub default_length_s: u32,
    /// The stereo separation of tracker modules, in percent (0 to 200; 100 is the
    /// module's own).
    pub tracker_stereo_separation: u32,
    /// The interpolation of tracker module samples.
    pub tracker_interpolation: TrackerInterpolation,
    /// Whether each file is read into memory whole before it plays (for slow or
    /// sleeping drives). Files over [`MEMORY_BUFFER_LIMIT`] are read as usual.
    pub load_into_memory: bool,
}

impl Default for DecoderSettings {
    fn default() -> Self {
        Self {
            soundfont: None,
            sid_song_lengths: None,
            sid_roms: None,
            loops: 2,
            fade_ms: 8000,
            default_length_s: 150,
            tracker_stereo_separation: 100,
            tracker_interpolation: TrackerInterpolation::Default,
            load_into_memory: false,
        }
    }
}

impl DecoderSettings {
    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_range("playback.decoders.loops", self.loops, 1..=10)?;
        check_range("playback.decoders.fade_ms", self.fade_ms, 0..=30_000)?;
        check_range(
            "playback.decoders.default_length_s",
            self.default_length_s,
            10..=3600,
        )?;
        check_range(
            "playback.decoders.tracker_stereo_separation",
            self.tracker_stereo_separation,
            0..=200,
        )
    }

    /// The fade.
    pub fn fade(&self) -> Duration {
        Duration::from_millis(self.fade_ms.into())
    }

    /// The length of tunes whose length nothing states, before the fade.
    pub fn default_length(&self) -> Duration {
        Duration::from_secs(self.default_length_s.into())
    }
}

/// How the samples of tracker modules are interpolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrackerInterpolation {
    /// What the module format suggests (libopenmpt's default).
    #[default]
    Default,
    /// None: the raw samples, as early trackers played them.
    None,
    /// Linear.
    Linear,
    /// Cubic.
    Cubic,
    /// An 8-tap windowed sinc.
    Sinc,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::settings::Settings;

    #[test]
    fn defaults_and_ranges() {
        let decoders = DecoderSettings::default();
        assert_eq!(decoders.validate(), Ok(()));
        assert!(!decoders.load_into_memory);
        assert_eq!(MEMORY_BUFFER_LIMIT, 512 * 1024 * 1024);
        for change in [
            |d: &mut DecoderSettings| d.loops = 0,
            |d: &mut DecoderSettings| d.fade_ms = 30_001,
            |d: &mut DecoderSettings| d.default_length_s = 5,
            |d: &mut DecoderSettings| d.tracker_stereo_separation = 201,
        ] {
            let mut decoders = DecoderSettings::default();
            change(&mut decoders);
            assert!(decoders.validate().is_err(), "{decoders:?}");
        }
    }

    #[test]
    fn paths_round_trip_through_toml() {
        let mut settings = Settings::default();
        let text = toml::to_string(&settings).unwrap();
        assert!(!text.contains("soundfont"), "{text}");
        settings.playback.decoders.soundfont = Some("/usr/share/soundfonts/a.sf2".into());
        settings.playback.decoders.tracker_interpolation = TrackerInterpolation::Sinc;
        let text = toml::to_string(&settings).unwrap();
        assert!(
            text.contains("soundfont = \"/usr/share/soundfonts/a.sf2\""),
            "{text}"
        );
        assert!(text.contains("tracker_interpolation = \"sinc\""), "{text}");
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back, settings);
        assert_eq!(
            back.playback.decoders.soundfont.as_deref(),
            Some(Path::new("/usr/share/soundfonts/a.sf2"))
        );
    }
}
