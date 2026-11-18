// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Changing the output and loudness settings while the engine runs.
//!
//! - A change of the sample rate policy, the sample format, the buffer length or the
//!   volume control changes the stream: while playing, the engine fades out, reopens
//!   the stream and continues at the heard position with a fade in, as for a device
//!   switch (`Reconfiguring`). While paused the stream is closed and opened anew on
//!   `Play`; while stopped there is none.
//! - A loudness change keeps the stream. The gain moves on the audio rendered from
//!   then on, over the gain stage's 20 ms ramp, so the change is heard after the
//!   audio already buffered (at most the buffer length). Items opened later, and the
//!   current one after a seek or resume, are planned with the new settings.
//! - The device-lost policy, pausing on unplug and the release time of a paused
//!   exclusive device apply from then on. The backend, device and mode stay as
//!   `SetOutput` chose them.
//! - Each of these is compared with the device's profile applied (see
//!   [`OutputSettings::for_device`]): a profile that sets its own value hides a
//!   change of the general one, and a changed profile (including its mode) reopens
//!   the stream.
//! - A DSP change keeps the stream too. The chain builds its new stages and
//!   crossfades to them on the audio rendered from then on; the plan of the current
//!   item follows (dither, the signal path). The resampler quality and the LFE
//!   option of the downmix apply to items planned later.

use helikodu_model::player::TransportState;
use helikodu_model::settings::{DspSettings, OutputSettings};

use super::device::plan_for;
use super::worker::Worker;
use crate::dsp::{DspChain, processing_settings};
use crate::output::DeviceAction;
use crate::pipeline::LoudnessSettings;

impl Worker {
    pub(super) fn update_settings(
        &mut self,
        mut output: OutputSettings,
        loudness: LoudnessSettings,
        dsp: DspSettings,
    ) {
        output.backend = self.selection.backend;
        output.device = self.selection.device.clone();
        output.mode = self.selection.mode;
        // Compared with the device's profile applied: a changed profile reopens the
        // stream as a changed general setting does.
        let before = self.output_settings_for(&self.selection.device);
        let regain = self.config.processing.loudness != loudness;
        self.config.output = output;
        self.config.processing.loudness = loudness;
        let redsp = self.config.dsp != dsp;
        self.config.processing = processing_settings(self.config.processing, &dsp);
        if let Some(chain) = &mut self.dsp {
            chain.set_settings(&dsp);
        }
        self.config.dsp = dsp;
        let reopen = changes_stream(&before, &self.output_settings_for(&self.selection.device));
        // What applies without another stream (the release time) applies from now on.
        if let Some(device) = self.output.as_ref().map(|output| output.device.clone()) {
            let settings = self.output_settings_for(&device);
            if let Some(output) = &mut self.output {
                output.settings = settings;
            }
        }
        let playing = matches!(
            self.state,
            TransportState::Playing | TransportState::Draining
        );
        if reopen {
            if playing {
                self.reopen_where_heard();
            } else {
                self.close_output();
            }
        } else if (regain || redsp) && playing {
            self.retune_gain();
        }
    }

    /// Moves the current item's gain to what the settings now give it, from the
    /// first frame rendered after this, and its plan to the DSP chain's stages.
    fn retune_gain(&mut self) {
        let (Some(output), Some(source)) = (&self.output, &self.current) else {
            return;
        };
        if source.plan().is_none() {
            return;
        }
        let dsp = self.dsp.as_ref().map(DspChain::plan).unwrap_or_default();
        let plan = plan_for(&self.config.processing, output, source).with_dsp(dsp);
        let channels = output.writer.channels().max(1);
        let at = output.writer.frames_written() + (self.staged.len() / channels) as u64;
        let dither = plan.always_dither;
        if let Some(source) = &mut self.current {
            source.retune(plan);
        }
        if let Some(output) = &mut self.output {
            // A full action queue means the device side has stopped; the stream's
            // state reports that.
            let _ = output
                .writer
                .schedule(at, DeviceAction::AlwaysDither(dither));
        }
    }
}

/// Whether going from the output settings `old` to `new` needs another stream.
fn changes_stream(old: &OutputSettings, new: &OutputSettings) -> bool {
    old.mode != new.mode
        || old.sample_rate != new.sample_rate
        || old.format != new.format
        || old.buffer_ms != new.buffer_ms
        || old.volume != new.volume
}
