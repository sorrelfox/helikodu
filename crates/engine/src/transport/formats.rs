// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine's part in the long tail of formats: the decoder settings, the choice
//! of an audio stream in files with several, and DSD over PCM (DoP).
//!
//! **DoP.** With [`DsdOutput::Dop`] in exclusive mode the registry opens DSD files as
//! DoP. A DoP source plays only on a path that leaves its samples untouched
//! ([`keeps_dop`]): no gain, channel mixing, resampling, crossfade or DSP, a fixed or
//! hardware volume, and a device format of 24 bits or more. On any other path it is
//! replaced by the same file converted to PCM, at the same position, before the
//! first sample is rendered.

use helikodu_model::audio::{OutputMode, SampleFormat};
use helikodu_model::player::{EngineError, EngineEvent, ItemId, TransportState};
use helikodu_model::settings::{DecoderSettings, DsdOutput};

use super::source::Source;
use super::worker::Worker;
use crate::pipeline::{PathPlan, Transition, VolumePlan};

impl Worker {
    /// Replaces the decoder settings; they apply to the items opened from now on.
    pub(super) fn set_decoder_settings(&mut self, settings: &DecoderSettings) {
        self.registry.configure(settings);
    }

    /// Tells the registry whether to open DSD files as DoP: when the settings ask for
    /// it and the device is used exclusively. Whether the path is untouched is known
    /// only with the plan, which [`keeps_dop`] checks.
    pub(super) fn sync_dop(&self) {
        let settings = self.output_settings_for(&self.selection.device);
        self.registry
            .set_dop(settings.dsd == DsdOutput::Dop && settings.mode == OutputMode::Exclusive);
    }

    /// Plays the audio stream `stream` of the loaded item's file from the heard
    /// position, as a seek does, and sends the item's streams again. While paused
    /// the item is opened with the stream at once, so that a stream that does not
    /// open is reported now.
    pub(super) fn select_audio_stream(&mut self, stream: Option<u32>) {
        match self.state {
            TransportState::Paused => {
                let Some(resume) = &mut self.resume else {
                    self.error(None, EngineError::NothingLoaded);
                    return;
                };
                let mut item = resume.segment.item.clone();
                item.audio_stream = stream;
                let position = resume.position;
                let opened = Source::open(&self.registry, item.clone()).and_then(|mut source| {
                    source.seek(position)?;
                    Ok(source)
                });
                match opened {
                    Ok(source) => {
                        let id = item.id;
                        resume.segment.item = item;
                        self.current = Some(source);
                        if let Some(event) = self.audio_streams_event(id) {
                            self.emit(event);
                        }
                    }
                    Err(error) => self.error(Some(item.id), error),
                }
            }
            TransportState::Playing | TransportState::Draining => {
                self.set_state(TransportState::Seeking);
                let Some(mut resume) = self.stop_where_heard(Transition::Seek) else {
                    self.stop();
                    return;
                };
                resume.segment.item.audio_stream = stream;
                let id = resume.segment.item.id;
                if self.current.as_ref().is_some_and(|s| s.item().id == id) {
                    self.current = None;
                }
                self.continue_at(resume, Some(Transition::Seek));
                if let Some(event) = self.audio_streams_event(id) {
                    self.emit(event);
                }
            }
            TransportState::Stopped | TransportState::Loading => {
                self.error(None, EngineError::NothingLoaded);
            }
            TransportState::Seeking | TransportState::Reconfiguring => {}
        }
    }

    /// The audio streams of `item`'s file, when it has several, for the event that
    /// follows its `TrackStarted`.
    pub(super) fn audio_streams_event(&self, item: ItemId) -> Option<EngineEvent> {
        let source = [&self.current, &self.next]
            .into_iter()
            .flatten()
            .find(|s| s.item().id == item)?;
        let info = source.decoder_info();
        if info.streams.len() < 2 {
            return None;
        }
        Some(EngineEvent::AudioStreams {
            item,
            streams: info.streams.clone(),
            playing: info.stream?,
        })
    }
}

/// Whether `plan` leaves DoP samples untouched, so that the DAC sees the markers.
pub(super) fn keeps_dop(plan: &PathPlan) -> bool {
    let wide = matches!(
        plan.device.format,
        SampleFormat::S24(_) | SampleFormat::S24Packed(_) | SampleFormat::S32(_)
    );
    plan.is_bit_perfect()
        && plan.gain.is_none()
        && plan.channel_mix.is_none()
        && plan.resample.is_none()
        && plan.speed.is_none()
        && !plan.crossfade
        && !plan.dsp
        && plan.dsp_stages.is_empty()
        && matches!(plan.volume, VolumePlan::Fixed | VolumePlan::Hardware)
        && wide
}
