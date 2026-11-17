// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine's configuration.

use std::time::Duration;

use helikodu_model::player::Volume;
use helikodu_model::settings::OutputSettings;

use crate::output::Pacing;
use crate::pipeline::ProcessingSettings;

/// What the engine starts with.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    /// The `[output]` settings: backend, device and mode (until
    /// [`SetOutput`](helikodu_model::player::EngineCommand::SetOutput) replaces them),
    /// sample rate and format policy, buffer length, volume control and the
    /// device-lost policy.
    pub output: OutputSettings,
    /// Loudness, resampler quality, channel mixing and fade lengths.
    pub processing: ProcessingSettings,
    /// The volume to start with.
    pub volume: Volume,
    /// How the null and WAV backends take samples. With [`Pacing::Freewheel`] they
    /// take them as fast as they are rendered; then there is no deadline, and at the
    /// end of an item the engine waits for the answer to
    /// [`NearEnd`](helikodu_model::player::EngineEvent::NearEnd) (up to
    /// [`next_wait`](Self::next_wait)) instead of draining at once.
    pub sink_pacing: Pacing,
    /// How long before the end of an item `NearEnd` is sent.
    pub near_end: Duration,
    /// How often `Position` is sent while playing.
    pub position_interval: Duration,
    /// How long a freewheeling sink waits for the next item at the end of one.
    pub next_wait: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            output: OutputSettings::default(),
            processing: ProcessingSettings::default(),
            volume: Volume::FULL,
            sink_pacing: Pacing::RealTime,
            near_end: Duration::from_secs(5),
            position_interval: Duration::from_millis(100),
            next_wait: Duration::from_secs(5),
        }
    }
}
