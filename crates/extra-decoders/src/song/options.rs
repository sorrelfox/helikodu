// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The options of the synthesizing decoders.

use std::path::PathBuf;
use std::time::Duration;

use helikodu_model::settings::{DecoderSettings, TrackerInterpolation};

/// How tunes are rendered and how long they play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SongOptions {
    /// The sample rate the decoders render at.
    pub rate: u32,
    /// How many times a looping tune plays its loop before the fade (at least 1).
    pub loops: u32,
    /// The fade at the end of a looping tune, or one played for the default length.
    pub fade: Duration,
    /// How long a tune whose length nothing states plays before the fade.
    pub default_length: Duration,
    /// The stereo separation of tracker modules in percent (0 to 200).
    pub stereo_separation: u32,
    /// The interpolation of tracker module samples.
    pub interpolation: Interpolation,
    /// The SoundFont for MIDI files; `None` looks for one installed on the system.
    pub soundfont: Option<PathBuf>,
    /// The HVSC song-length database for SID tunes.
    pub sid_song_lengths: Option<PathBuf>,
    /// A folder with C64 ROM images.
    pub sid_roms: Option<PathBuf>,
}

/// How tracker module samples are interpolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Interpolation {
    /// The library's default.
    #[default]
    Default,
    /// None.
    None,
    /// Linear.
    Linear,
    /// Cubic.
    Cubic,
    /// An 8-tap windowed sinc.
    Sinc,
}

impl Interpolation {
    /// libopenmpt's filter length for it.
    pub fn taps(self) -> i32 {
        match self {
            Self::Default => 0,
            Self::None => 1,
            Self::Linear => 2,
            Self::Cubic => 4,
            Self::Sinc => 8,
        }
    }
}

impl SongOptions {
    /// The rate tunes are rendered at by default: 44.1 kHz.
    pub const DEFAULT_RATE: u32 = 44_100;

    /// The options of `settings`.
    pub fn from_settings(settings: &DecoderSettings) -> Self {
        Self {
            rate: Self::DEFAULT_RATE,
            loops: settings.loops.max(1),
            fade: settings.fade(),
            default_length: settings.default_length(),
            stereo_separation: settings.tracker_stereo_separation.min(200),
            interpolation: match settings.tracker_interpolation {
                TrackerInterpolation::Default => Interpolation::Default,
                TrackerInterpolation::None => Interpolation::None,
                TrackerInterpolation::Linear => Interpolation::Linear,
                TrackerInterpolation::Cubic => Interpolation::Cubic,
                TrackerInterpolation::Sinc => Interpolation::Sinc,
            },
            soundfont: settings.soundfont.clone(),
            sid_song_lengths: settings.sid_song_lengths.clone(),
            sid_roms: settings.sid_roms.clone(),
        }
    }

    /// Frames at the rate in `duration`, rounded to the nearest.
    pub fn frames(&self, duration: Duration) -> u64 {
        let frames = duration.as_nanos() * u128::from(self.rate) / 1_000_000_000;
        u64::try_from(frames).unwrap_or(u64::MAX)
    }

    /// The duration of `frames` frames at the rate.
    pub fn duration(&self, frames: u64) -> Duration {
        Duration::from_nanos(
            u64::try_from(u128::from(frames) * 1_000_000_000 / u128::from(self.rate.max(1)))
                .unwrap_or(u64::MAX),
        )
    }
}

impl Default for SongOptions {
    fn default() -> Self {
        Self::from_settings(&DecoderSettings::default())
    }
}
