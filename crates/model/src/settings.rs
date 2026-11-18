// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings schema, one file per group, as stored in `settings.toml`; and the
//! state of the window, stored in `ui-state.toml`.
//!
//! Every field has a default, so a missing file, table or key reads as the default.
//! Keys this version does not know are ignored. The file carries a `version`, so that
//! a later version can move or reinterpret keys when it reads an older file.

mod advanced;
mod appearance;
mod decoders;
mod dsp;
mod library;
mod loudness;
mod output;
mod playback;
mod shortcuts;
mod transitions;
mod ui_state;

use std::fmt;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use advanced::{AdvancedSettings, Id3Version, LogDetail};
pub use appearance::{Accent, AppearanceSettings, ColorScheme, WindowStyle};
pub use decoders::{DecoderSettings, MEMORY_BUFFER_LIMIT, TrackerInterpolation};
pub use dsp::{
    BuiltInPreset, ChannelMixSettings, ChannelMode, CrossfeedPreset, CrossfeedSettings,
    DspSettings, EqMode, EqualizerSettings, FilterKind, GRAPHIC_10_BANDS_HZ, GRAPHIC_31_BANDS_HZ,
    GraphicPreset, LimiterSettings, ParametricFilter, ResamplerQuality,
};
pub use library::{DuplicateScan, DuplicateSettings, LibrarySettings, StorageLimits};
pub use loudness::{ClippingPrevention, LoudnessMode, LoudnessSettings};
pub use output::{
    DeviceLostPolicy, DeviceProfile, DsdOutput, OutputFormat, OutputSettings, SampleRatePolicy,
    VolumeControl,
};
pub use playback::{ContextAction, PlaybackSettings, StartAction};
pub use shortcuts::{MAX_KEYS_PER_ACTION, ShortcutSettings};
pub use transitions::TransitionSettings;
pub use ui_state::{MiniPlayerState, PaneState, UiState, WindowState};

/// All settings: the contents of `settings.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The version of the schema the file was written with. The settings store treats
    /// a file without one as version 0, written before versions were recorded, and
    /// migrates it before it is read into this type.
    pub version: u32,
    /// The `[library]` table: folders and scanning.
    pub library: LibrarySettings,
    /// The `[playback]` table.
    pub playback: PlaybackSettings,
    /// The `[output]` table: the audio interface.
    pub output: OutputSettings,
    /// The `[loudness]` table: loudness normalization.
    pub loudness: LoudnessSettings,
    /// The `[dsp]` table: the processing chain and the resampler quality.
    pub dsp: DspSettings,
    /// The `[appearance]` table.
    pub appearance: AppearanceSettings,
    /// The `[advanced]` table.
    pub advanced: AdvancedSettings,
    /// The `[shortcuts]` table: keyboard shortcuts changed by the user.
    pub shortcuts: ShortcutSettings,
}

impl Settings {
    /// The version of the schema this build writes.
    pub const VERSION: u32 = 1;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        self.library.validate()?;
        self.playback.validate()?;
        self.output.validate()?;
        self.loudness.validate()?;
        self.dsp.validate()?;
        self.advanced.validate()?;
        self.shortcuts.validate()
    }

    /// The groups whose values differ between `self` and `other`.
    pub fn changed_groups(&self, other: &Self) -> Vec<SettingsGroup> {
        let mut groups = Vec::new();
        if self.library != other.library {
            groups.push(SettingsGroup::Library);
        }
        if self.playback != other.playback {
            groups.push(SettingsGroup::Playback);
        }
        if self.output != other.output {
            groups.push(SettingsGroup::Output);
        }
        if self.loudness != other.loudness {
            groups.push(SettingsGroup::Loudness);
        }
        if self.dsp != other.dsp {
            groups.push(SettingsGroup::Dsp);
        }
        if self.appearance != other.appearance {
            groups.push(SettingsGroup::Appearance);
        }
        if self.advanced != other.advanced {
            groups.push(SettingsGroup::Advanced);
        }
        if self.shortcuts != other.shortcuts {
            groups.push(SettingsGroup::Shortcuts);
        }
        groups
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            library: LibrarySettings::default(),
            playback: PlaybackSettings::default(),
            output: OutputSettings::default(),
            loudness: LoudnessSettings::default(),
            dsp: DspSettings::default(),
            appearance: AppearanceSettings::default(),
            advanced: AdvancedSettings::default(),
            shortcuts: ShortcutSettings::default(),
        }
    }
}

/// A table of `settings.toml`, one page of the settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsGroup {
    /// `[library]`.
    Library,
    /// `[playback]`.
    Playback,
    /// `[output]`.
    Output,
    /// `[loudness]`.
    Loudness,
    /// `[dsp]`.
    Dsp,
    /// `[appearance]`.
    Appearance,
    /// `[advanced]`.
    Advanced,
    /// `[shortcuts]`.
    Shortcuts,
}

/// When a changed setting takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    /// At once.
    Live,
    /// When the next track starts.
    NextTrack,
    /// When the program starts again.
    Restart,
}

/// A setting with a value it may not have.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{key} {problem}")]
pub struct SettingsError {
    /// The setting's key in `settings.toml`, for example `output.buffer_ms`.
    pub key: String,
    /// What is wrong with it, as the rest of a sentence that starts with the key, for
    /// example "is 10, but must be from 50 to 2000".
    pub problem: String,
}

impl SettingsError {
    /// A value outside the range from `min` to `max`.
    pub fn out_of_range(
        key: impl Into<String>,
        value: impl fmt::Display,
        min: impl fmt::Display,
        max: impl fmt::Display,
    ) -> Self {
        Self {
            key: key.into(),
            problem: format!("is {value}, but must be from {min} to {max}"),
        }
    }
}

/// Checks that `value` of the setting `key` is in `range`.
pub(crate) fn check_range<T>(
    key: &str,
    value: T,
    range: RangeInclusive<T>,
) -> Result<(), SettingsError>
where
    T: PartialOrd + fmt::Display + Copy,
{
    if range.contains(&value) {
        Ok(())
    } else {
        Err(SettingsError::out_of_range(
            key,
            value,
            range.start(),
            range.end(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let settings: Settings = toml::from_str("").unwrap();
        assert_eq!(settings.version, Settings::VERSION);
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.validate(), Ok(()));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let settings: Settings =
            toml::from_str("[output]\nfuture_key = 1\n[later]\nx = 2\n").unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn defaults_round_trip() {
        let text = toml::to_string(&Settings::default()).unwrap();
        assert!(text.starts_with("version = 1\n"), "{text}");
        for table in [
            "[library]",
            "[playback]",
            "[output]",
            "[loudness]",
            "[appearance]",
            "[advanced]",
        ] {
            assert!(text.contains(table), "{text}");
        }
        assert_eq!(
            toml::from_str::<Settings>(&text).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn changed_groups_are_named() {
        let old = Settings::default();
        let mut new = old.clone();
        assert!(old.changed_groups(&new).is_empty());
        new.playback.seek_step_s = 10;
        new.appearance.accent = Accent::Violet;
        assert_eq!(
            old.changed_groups(&new),
            vec![SettingsGroup::Playback, SettingsGroup::Appearance]
        );
    }

    #[test]
    fn errors_read_as_sentences() {
        let error = SettingsError::out_of_range("loudness.preamp_db", -13.0, -12.0, 12.0);
        assert_eq!(
            error.to_string(),
            "loudness.preamp_db is -13, but must be from -12 to 12"
        );
    }
}
