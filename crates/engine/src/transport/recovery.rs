// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Leaving a transition: settling in a state, and what happens when the output
//! fails, its device goes away, or the device it follows is unplugged.
//!
//! - A stream on a chosen device whose device goes away (every backend reports it
//!   as lost) applies `on_device_lost`: pause, continue on the default device, or
//!   stop.
//! - A stream that follows "System default" is not lost when its device goes: the
//!   sound server moves it to the next default device. When the device it played to
//!   is removed (headphones, a USB or Bluetooth device unplugged), playback pauses if
//!   `pause_on_unplug` is set, so that it does not go on through the loudspeakers.

use helikodu_model::audio::{DeviceId, DeviceSelection, OutputBackendKind};
use helikodu_model::player::{EngineError, EngineEvent, TrackEndReason, TransportState};
use helikodu_model::settings::DeviceLostPolicy;

use super::worker::{Resume, Worker};
use crate::pipeline::Transition;

impl Worker {
    /// Forgets the items: nothing is loaded.
    pub(super) fn reset_items(&mut self) {
        self.drop_crossfade();
        self.current = None;
        self.next = None;
        self.next_answered = false;
        self.timeline.reset(0);
        self.staged = 0..0;
        self.resume = None;
        self.drain = None;
        self.waiting_since = None;
        self.near_end_sent = None;
    }

    /// Moves to `Playing`, or on to `Draining` if the end was already rendered.
    pub(super) fn settle_playing(&mut self) {
        self.set_state(TransportState::Playing);
        if self.drain.is_some_and(|drain| !drain.reopen) {
            self.set_state(TransportState::Draining);
        }
    }

    /// Holds at `resume` until `Play`.
    pub(super) fn pause_at(&mut self, resume: Resume) {
        let (item, position) = (resume.segment.item.id, resume.position);
        self.drain = None;
        self.waiting_since = None;
        self.resume = Some(resume);
        self.set_state(TransportState::Paused);
        self.send_position_of(item, position);
    }

    /// The output could not be opened or started for `resume`: reports it and
    /// pauses there, or stops if the device-lost policy says so.
    pub(super) fn output_failed(&mut self, error: EngineError, resume: Resume) {
        self.error(Some(resume.segment.item.id), error);
        self.close_output();
        if self.config.output.on_device_lost == DeviceLostPolicy::Stop {
            self.emit(EngineEvent::TrackEnded {
                item: resume.segment.item.id,
                reason: TrackEndReason::Interrupted,
            });
            self.reset_items();
            self.set_state(TransportState::Stopped);
        } else {
            self.pause_at(resume);
        }
    }

    /// A device of `backend` went away. When it is the default device a stream that
    /// follows "System default" plays to, playback pauses if the settings say so.
    pub(super) fn device_removed(&mut self, backend: OutputBackendKind, id: &DeviceId) {
        let follows_it = self.output.as_ref().is_some_and(|output| {
            output.stream.backend() == backend && output.device == DeviceSelection::SystemDefault
        }) && self.backends.default_device.as_ref() == Some(id);
        if follows_it && self.config.output.pause_on_unplug {
            self.pause();
        }
    }

    /// The stream's device went away or the stream failed: applies the device-lost
    /// policy.
    pub(super) fn device_failed(&mut self, error: EngineError) {
        let paused = self.state == TransportState::Paused;
        let resume = if paused {
            self.resume.clone()
        } else {
            let at = self.heard_now();
            self.advance_to(at);
            self.resume_at(at)
        };
        self.close_output();
        let Some(resume) = resume else {
            self.error(None, error);
            self.reset_items();
            self.set_state(TransportState::Stopped);
            return;
        };
        let fallback = self.config.output.on_device_lost == DeviceLostPolicy::FallbackDefault
            && self.selection.device != DeviceSelection::SystemDefault;
        if paused || !fallback {
            self.output_failed(error, resume);
            return;
        }
        self.error(Some(resume.segment.item.id), error);
        self.set_state(TransportState::Reconfiguring);
        self.selection.device = DeviceSelection::SystemDefault;
        let result = self
            .restart_sources(&resume)
            .and_then(|()| self.play_current(Some(Transition::DeviceSwitch)));
        match result {
            Ok(()) => {
                self.settle_playing();
                self.send_position_of(resume.segment.item.id, resume.position);
            }
            Err(error) => self.output_failed(error, resume),
        }
    }
}
