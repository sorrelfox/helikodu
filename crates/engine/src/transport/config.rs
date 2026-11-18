// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine's configuration.

use std::time::Duration;

use helikodu_model::player::Volume;
use helikodu_model::settings::{DspSettings, OutputSettings, TransitionSettings};

use crate::output::{AudioTap, Pacing, SharedConnector, SharedReservation};
use crate::pipeline::{FadeDurations, ProcessingSettings};

/// What the engine starts with.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    /// The `[output]` settings: backend, device and mode (until
    /// [`SetOutput`](helikodu_model::player::EngineCommand::SetOutput) replaces them),
    /// sample rate and format policy, buffer length, volume control and the
    /// device-lost policy (until
    /// [`UpdateSettings`](helikodu_model::player::EngineCommand::UpdateSettings)
    /// replaces them).
    pub output: OutputSettings,
    /// Loudness (until `UpdateSettings` replaces it), resampler quality, channel
    /// mixing and fade lengths.
    pub processing: ProcessingSettings,
    /// The `[dsp]` settings: the DSP chain (until `UpdateSettings` replaces them).
    /// The resampler quality and the LFE option in them take effect through
    /// `processing`, which [`dsp::processing_settings`](crate::dsp::processing_settings)
    /// fills from them; `UpdateSettings` does that itself.
    pub dsp: DspSettings,
    /// The crossfades: between tracks, on a skip, and whether albums stay gapless
    /// (until [`SetTransitions`](helikodu_model::player::EngineCommand::SetTransitions)
    /// replaces them). Crossfades are off by default. The fade lengths in them take
    /// effect through `processing.fades`; [`with_transitions`](Self::with_transitions)
    /// and `SetTransitions` set both.
    pub transitions: TransitionSettings,
    /// The volume to start with.
    pub volume: Volume,
    /// How the null and WAV backends take samples. With [`Pacing::Freewheel`] they
    /// take them as fast as they are rendered; then there is no deadline, and at the
    /// end of an item the engine waits for the answer to
    /// [`NearEnd`](helikodu_model::player::EngineEvent::NearEnd) (up to
    /// [`next_wait`](Self::next_wait)) instead of draining at once.
    pub sink_pacing: Pacing,
    /// How long before the end of an item `NearEnd` is sent. With crossfades between
    /// tracks on, it is sent earlier by the crossfade's length.
    pub near_end: Duration,
    /// How often `Position` is sent while playing.
    pub position_interval: Duration,
    /// How long a freewheeling sink waits for the next item at the end of one.
    pub next_wait: Duration,
    /// How the ALSA backend takes a sound card that another program (usually the
    /// sound server) holds. The default, [`NoReservation`](crate::output::NoReservation),
    /// takes nothing: such a device is then reported busy.
    pub device_reservation: SharedReservation,
    /// Backends the program brings itself, in place of the built-in ones of the same
    /// kind. The default has none.
    pub backend_connector: SharedConnector,
    /// Where every stream copies what it gives its device, for the analyzer.
    /// Copying starts with [`AudioTap::set_wanted`].
    pub tap: AudioTap,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            output: OutputSettings::default(),
            processing: ProcessingSettings::default(),
            dsp: DspSettings::default(),
            transitions: TransitionSettings::default(),
            volume: Volume::FULL,
            sink_pacing: Pacing::RealTime,
            near_end: Duration::from_secs(5),
            position_interval: Duration::from_millis(100),
            next_wait: Duration::from_secs(5),
            device_reservation: SharedReservation::default(),
            backend_connector: SharedConnector::default(),
            tap: AudioTap::default(),
        }
    }
}

impl EngineConfig {
    /// The same configuration with the crossfades and fades of `transitions`.
    #[must_use]
    pub fn with_transitions(mut self, transitions: TransitionSettings) -> Self {
        self.transitions = transitions;
        self.processing.fades = FadeDurations::from_settings(&transitions);
        self
    }
}
