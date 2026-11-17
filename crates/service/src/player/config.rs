// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a player starts with.

use std::ops::RangeInclusive;

use helikodu_engine::output::Pacing;
use helikodu_engine::pipeline::{
    ClippingPrevention, LoudnessMode, LoudnessSettings, ProcessingSettings,
};
use helikodu_engine::transport::EngineConfig;
use helikodu_model::audio::{
    BackendSelection, DeviceSelection, OutputBackendKind, OutputSelection,
};
use helikodu_model::player::Volume;
use helikodu_model::settings::OutputSettings;

use super::PlayerError;

/// How the null and WAV backends, which have no device clock, take audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SinkPacing {
    /// At the speed it plays, like a device.
    #[default]
    RealTime,
    /// As fast as it is decoded: for tests and for writing files.
    Freewheel,
}

/// Which loudness gain the player applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReplayGainMode {
    /// No loudness normalization.
    #[default]
    Off,
    /// The track gain, or the album gain if a file has no track gain.
    Track,
    /// The album gain, or the track gain if a file has no album gain. For files
    /// queued in album order.
    Album,
    /// The album gain for a file whose neighbour in the queue (the file before or
    /// after it) has the same album and album artists in its tags, the track gain
    /// otherwise.
    Auto,
}

/// Loudness normalization by the ReplayGain and Opus R128 tags of the files.
///
/// Gains are relative to the ReplayGain 2.0 reference of −18 LUFS; Opus R128 gains
/// (relative to −23 LUFS) are converted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplayGain {
    /// Which gain is applied.
    pub mode: ReplayGainMode,
    /// Added to the gain of files that have one, in dB.
    pub preamp_db: f64,
    /// The gain of files without loudness tags, in dB.
    pub untagged_preamp_db: f64,
    /// Whether a gain is lowered so that the file's peak (from its tags) stays at or
    /// below full scale. On by default.
    pub prevent_clipping: bool,
}

impl ReplayGain {
    /// The range of both preamps, in dB.
    pub const PREAMP_RANGE: RangeInclusive<f64> = -24.0..=24.0;
}

impl Default for ReplayGain {
    fn default() -> Self {
        Self {
            mode: ReplayGainMode::Off,
            preamp_db: 0.0,
            untagged_preamp_db: 0.0,
            prevent_clipping: true,
        }
    }
}

/// The settings a player starts with. There is no settings file yet; the output
/// settings are built from the chosen backend, device and mode, and the rest keep
/// their defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerConfig {
    /// The `[output]` settings.
    pub output: OutputSettings,
    /// The volume to start with.
    pub volume: Volume,
    /// How the null and WAV backends take audio.
    pub pacing: SinkPacing,
    /// Loudness normalization; off by default.
    pub replay_gain: ReplayGain,
}

impl PlayerConfig {
    /// Plays to `selection`, with the default output settings otherwise, at full
    /// volume and in real time.
    pub fn new(selection: OutputSelection) -> Self {
        let OutputSelection {
            backend,
            device,
            mode,
        } = selection;
        Self {
            output: OutputSettings {
                backend,
                device,
                mode,
                ..OutputSettings::default()
            },
            volume: Volume::FULL,
            pacing: SinkPacing::RealTime,
            replay_gain: ReplayGain::default(),
        }
    }

    /// Checks the settings: the ranges of the output settings and the preamps, and
    /// that the WAV backend has a file name as its device.
    pub fn validate(&self) -> Result<(), PlayerError> {
        self.output
            .validate()
            .map_err(|error| PlayerError::InvalidConfig(error.to_string()))?;
        for (name, value) in [
            ("preamp", self.replay_gain.preamp_db),
            ("untagged preamp", self.replay_gain.untagged_preamp_db),
        ] {
            if !ReplayGain::PREAMP_RANGE.contains(&value) {
                return Err(PlayerError::InvalidConfig(format!(
                    "the {name} is {value} dB, but must be from {} to {} dB",
                    ReplayGain::PREAMP_RANGE.start(),
                    ReplayGain::PREAMP_RANGE.end()
                )));
            }
        }
        if self.output.backend == BackendSelection::Backend(OutputBackendKind::Wav)
            && self.output.device == DeviceSelection::SystemDefault
        {
            return Err(PlayerError::InvalidConfig(
                "the wav backend needs a file name as its device".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            output: self.output.clone(),
            volume: self.volume,
            sink_pacing: match self.pacing {
                SinkPacing::RealTime => Pacing::RealTime,
                SinkPacing::Freewheel => Pacing::Freewheel,
            },
            processing: ProcessingSettings {
                loudness: self.loudness_settings(),
                ..ProcessingSettings::default()
            },
            ..EngineConfig::default()
        }
    }
}

impl PlayerConfig {
    fn loudness_settings(&self) -> LoudnessSettings {
        let gain = &self.replay_gain;
        LoudnessSettings {
            mode: match gain.mode {
                ReplayGainMode::Off => LoudnessMode::Off,
                ReplayGainMode::Track => LoudnessMode::Track,
                ReplayGainMode::Album => LoudnessMode::Album,
                ReplayGainMode::Auto => LoudnessMode::Auto,
            },
            preamp_db: gain.preamp_db,
            untagged_preamp_db: gain.untagged_preamp_db,
            clipping: if gain.prevent_clipping {
                ClippingPrevention::Peak
            } else {
                ClippingPrevention::Limiter
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helikodu_model::audio::OutputMode;

    #[test]
    fn selection_becomes_output_settings() {
        let selection = OutputSelection {
            backend: BackendSelection::Backend(OutputBackendKind::Alsa),
            device: "hw:CARD=D10s,DEV=0".parse().unwrap(),
            mode: OutputMode::Exclusive,
        };
        let config = PlayerConfig::new(selection.clone());
        assert_eq!(config.output.selection(), selection);
        assert_eq!(config.output.buffer_ms, OutputSettings::default().buffer_ms);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn wav_needs_a_file() {
        let mut selection = OutputSelection {
            backend: BackendSelection::Backend(OutputBackendKind::Wav),
            ..OutputSelection::default()
        };
        assert!(matches!(
            PlayerConfig::new(selection.clone()).validate(),
            Err(PlayerError::InvalidConfig(_))
        ));
        selection.device = "out.wav".parse().unwrap();
        assert_eq!(PlayerConfig::new(selection).validate(), Ok(()));
    }

    #[test]
    fn replay_gain_reaches_the_engine() {
        let mut config = PlayerConfig::new(OutputSelection::default());
        assert_eq!(
            config.engine_config().processing.loudness.mode,
            LoudnessMode::Off
        );
        config.replay_gain = ReplayGain {
            mode: ReplayGainMode::Album,
            preamp_db: 3.0,
            untagged_preamp_db: -2.0,
            prevent_clipping: false,
        };
        let loudness = config.engine_config().processing.loudness;
        assert_eq!(loudness.mode, LoudnessMode::Album);
        assert_eq!(loudness.preamp_db, 3.0);
        assert_eq!(loudness.untagged_preamp_db, -2.0);
        assert_eq!(loudness.clipping, ClippingPrevention::Limiter);

        config.replay_gain.preamp_db = 30.0;
        assert!(matches!(
            config.validate(),
            Err(PlayerError::InvalidConfig(_))
        ));
        config.replay_gain.preamp_db = f64::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn invalid_buffer_is_refused() {
        let mut config = PlayerConfig::new(OutputSelection::default());
        config.output.buffer_ms = 1;
        assert!(config.validate().is_err());
    }
}
