// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine's commands, carried out on the engine thread.

use helikodu_model::audio::{MediaTime, OutputSelection};
use helikodu_model::player::{
    EngineError, EngineEvent, ItemId, PlaybackItem, TrackEndReason, TransportState,
};

use super::source::Source;
use super::worker::{Resume, Worker};
use crate::output::DeviceAction;
use crate::pipeline::Transition;

impl Worker {
    pub(super) fn load(&mut self, item: PlaybackItem, start: MediaTime, paused: bool) {
        self.interrupt(Transition::Stop);
        self.reset_items();
        self.set_state(TransportState::Loading);
        let id = item.id;
        let opened = Source::open(&self.registry, item).and_then(|mut source| {
            let frames = source.seek(start)?;
            Ok((source, frames))
        });
        let (source, frames) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                self.item_failed(id, error);
                return;
            }
        };
        let position = MediaTime::from_frames(frames, source.rate());
        let resume = Resume {
            segment: self.segment_of(&source, 0),
            follower: None,
            position,
        };
        self.emit(resume.segment.started_event());
        self.current = Some(source);
        if paused {
            self.close_output();
            self.pause_at(resume);
            return;
        }
        let transition = (!position.is_zero()).then_some(Transition::Start);
        match self.play_current(transition) {
            Ok(()) => {
                self.settle_playing();
                self.send_position_of(id, position);
            }
            Err(error) => self.output_failed(error, resume),
        }
    }

    pub(super) fn play(&mut self) {
        match self.state {
            TransportState::Paused => {
                let Some(resume) = self.resume.take() else {
                    self.error(None, EngineError::NothingLoaded);
                    return;
                };
                let transition = (!resume.position.is_zero()).then_some(Transition::Resume);
                self.continue_at(resume, transition);
            }
            TransportState::Stopped => self.error(None, EngineError::NothingLoaded),
            _ => {}
        }
    }

    pub(super) fn pause(&mut self) {
        if !matches!(
            self.state,
            TransportState::Playing | TransportState::Draining
        ) {
            return;
        }
        let resume = self.stop_where_heard(Transition::Pause);
        if let Some(output) = &mut self.output {
            output.pause();
        }
        match resume {
            Some(resume) => self.pause_at(resume),
            None => self.stop(),
        }
    }

    pub(super) fn stop(&mut self) {
        if self.state == TransportState::Stopped {
            return;
        }
        self.interrupt(Transition::Stop);
        self.close_output();
        self.reset_items();
        self.set_state(TransportState::Stopped);
    }

    pub(super) fn seek(&mut self, to: MediaTime) {
        match self.state {
            TransportState::Paused => {
                let Some(resume) = &mut self.resume else {
                    self.error(None, EngineError::NothingLoaded);
                    return;
                };
                resume.position = clamp(to, resume.segment.length);
                let (item, position) = (resume.segment.item.id, resume.position);
                self.set_state(TransportState::Seeking);
                self.set_state(TransportState::Paused);
                self.send_position_of(item, position);
            }
            TransportState::Playing | TransportState::Draining => {
                self.set_state(TransportState::Seeking);
                let Some(mut resume) = self.stop_where_heard(Transition::Seek) else {
                    self.stop();
                    return;
                };
                resume.position = clamp(to, resume.segment.length);
                self.continue_at(resume, Some(Transition::Seek));
            }
            TransportState::Stopped | TransportState::Loading => {
                self.error(None, EngineError::NothingLoaded);
            }
            TransportState::Seeking | TransportState::Reconfiguring => {}
        }
    }

    pub(super) fn set_output(&mut self, selection: OutputSelection) {
        self.selection = selection;
        match self.state {
            TransportState::Playing | TransportState::Draining => {
                self.set_state(TransportState::Reconfiguring);
                let Some(resume) = self.stop_where_heard(Transition::DeviceSwitch) else {
                    self.stop();
                    return;
                };
                self.close_output();
                self.continue_at(resume, Some(Transition::DeviceSwitch));
            }
            _ => {
                // The new device is opened when playback starts; connect to the
                // backend now, so that its device list is sent.
                self.close_output();
                let selection = self.selection.clone();
                let connected = self.backends.get(&selection).map(|(backend, fresh)| {
                    fresh.map(|devices| EngineEvent::DevicesChanged {
                        backend: backend.kind(),
                        devices,
                    })
                });
                match connected {
                    Ok(Some(event)) => self.emit(event),
                    Ok(None) => {}
                    Err(error) => self.error(None, error.to_engine_error()),
                }
            }
        }
    }

    /// Replaces the item that follows the heard one.
    pub(super) fn set_next(&mut self, item: Option<PlaybackItem>) {
        if self.state == TransportState::Paused
            && let Some(resume) = &mut self.resume
            && resume.follower.is_some()
        {
            // The render side had joined the follower; it is reopened on resume.
            resume.follower = item;
            self.next = None;
            self.next_answered = true;
            return;
        }
        if self.timeline.len() > 1 && self.state != TransportState::Paused {
            self.replace_follower(item);
            return;
        }
        if let (Some(next), Some(item)) = (&self.next, &item)
            && next.item() == item
        {
            self.next_answered = true;
            return;
        }
        self.next = None;
        self.next_answered = true;
        if let Some(item) = item {
            self.open_next(item);
        }
    }

    /// The render side has already written the item after the heard one: drops it
    /// from the buffer and renders `item` there instead.
    fn replace_follower(&mut self, item: Option<PlaybackItem>) {
        if let (Some(follower), Some(item)) = (self.timeline.follower(), &item)
            && follower.item == *item
        {
            return;
        }
        let Some(output) = &mut self.output else {
            return;
        };
        let until = output.writer.frames_written();
        let Some(from) = self.timeline.cut_after_heard() else {
            return;
        };
        // Joins are seamless, so the jump to the new item needs no fade.
        let _ = output
            .writer
            .schedule(from, DeviceAction::Discard { until, hold: false });
        self.timeline.skip(from, until);
        self.staged = 0..0;
        self.current = None;
        self.next = None;
        self.next_answered = true;
        self.drain = None;
        if let Some(item) = item {
            self.open_next(item);
        }
    }

    /// Opens `item` as the next one. A failure ends it at once and asks again with
    /// `NearEnd`.
    fn open_next(&mut self, item: PlaybackItem) {
        let id = item.id;
        match Source::open(&self.registry, item) {
            Ok(source) => self.next = Some(source),
            Err(error) => {
                self.error(Some(id), error);
                self.emit(EngineEvent::TrackEnded {
                    item: id,
                    reason: TrackEndReason::Failed,
                });
                self.next_answered = false;
                self.near_end_sent = None;
            }
        }
    }

    /// Continues playback at `resume`, fading in with `transition`.
    fn continue_at(&mut self, resume: Resume, transition: Option<Transition>) {
        if let Err(error) = self.restart_sources(&resume) {
            self.item_failed(resume.segment.item.id, error);
            return;
        }
        match self.play_current(transition) {
            Ok(()) => {
                self.settle_playing();
                self.send_position_of(resume.segment.item.id, resume.position);
            }
            Err(error) => self.output_failed(error, resume),
        }
    }

    /// Ends the heard item: a fade out if playing, then `TrackEnded` with
    /// `Interrupted`.
    fn interrupt(&mut self, transition: Transition) {
        let item = match self.state {
            TransportState::Playing
            | TransportState::Draining
            | TransportState::Seeking
            | TransportState::Reconfiguring => self
                .stop_where_heard(transition)
                .map(|resume| resume.segment.item.id),
            TransportState::Paused => self.resume.as_ref().map(|r| r.segment.item.id),
            TransportState::Stopped | TransportState::Loading => None,
        };
        if let Some(item) = item {
            self.emit(EngineEvent::TrackEnded {
                item,
                reason: TrackEndReason::Interrupted,
            });
        }
    }

    /// An item could not be opened or decoded where it was asked to play: reports
    /// it and stops.
    fn item_failed(&mut self, item: ItemId, error: EngineError) {
        self.error(Some(item), error);
        self.emit(EngineEvent::TrackEnded {
            item,
            reason: TrackEndReason::Failed,
        });
        self.close_output();
        self.reset_items();
        self.set_state(TransportState::Stopped);
    }
}

/// `to`, but not past `length`.
fn clamp(to: MediaTime, length: Option<MediaTime>) -> MediaTime {
    match length {
        Some(length) if to > length => length,
        _ => to,
    }
}
