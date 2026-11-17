// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a player starts with.

use helikodu_engine::output::Pacing;
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
        }
    }

    /// Checks the settings: the ranges of the output settings, and that the WAV
    /// backend has a file name as its device.
    pub fn validate(&self) -> Result<(), PlayerError> {
        self.output
            .validate()
            .map_err(|error| PlayerError::InvalidConfig(error.to_string()))?;
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
            ..EngineConfig::default()
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
    fn invalid_buffer_is_refused() {
        let mut config = PlayerConfig::new(OutputSelection::default());
        config.output.buffer_ms = 1;
        assert!(config.validate().is_err());
    }
}
