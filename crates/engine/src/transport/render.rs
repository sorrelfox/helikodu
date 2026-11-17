// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The render side of the engine thread: filling the ring buffer, joining items,
//! draining, and the fades and refills around pauses, seeks and device switches.

use std::thread;
use std::time::{Duration, Instant};

use helikodu_model::audio::MediaTime;
use helikodu_model::player::{EngineError, EngineEvent, TrackEndReason, TransportState};

use super::source::Source;
use super::timeline::Segment;
use super::worker::{Drain, Resume, Worker};
use crate::output::DeviceAction;
use crate::pipeline::Transition;

/// Digital silence, written to a freewheeling sink while a fade out finishes after
/// the last rendered frame.
static SILENCE: [f32; 1024] = [0.0; 1024];
/// How much audio is buffered after a seek or resume before it fades in.
const REFILL: Duration = Duration::from_millis(50);
/// The longest wait for the device side during a transition.
const DEVICE_WAIT: Duration = Duration::from_millis(500);
/// The interval at which transitions look at the device side.
const POLL: Duration = Duration::from_millis(1);

impl Worker {
    /// Tops up the ring buffer: renders the current item, joins the next one when it
    /// ends, and starts draining when nothing follows. Writes at most one buffer
    /// length per call.
    pub(super) fn fill(&mut self) {
        let mut budget = match &self.output {
            Some(output) => output.writer.capacity_frames(),
            None => return,
        };
        loop {
            let Some(output) = self.output.as_mut() else {
                return;
            };
            let channels = output.writer.channels();
            if !self.staged.is_empty() {
                let frames = output.writer.write(&self.staging[self.staged.clone()]);
                self.staged.start += frames * channels;
                budget = budget.saturating_sub(frames);
                if !self.staged.is_empty() {
                    break;
                }
                continue;
            }
            if budget == 0 {
                break;
            }
            match &mut self.current {
                Some(source) if !source.is_finished() => {
                    let frames = source.render(&mut self.staging);
                    self.staged = 0..frames * channels;
                    if frames == 0 && !source.is_finished() {
                        break;
                    }
                }
                _ => {
                    if !self.end_of_current() {
                        break;
                    }
                }
            }
        }
    }

    /// The current item has been rendered to its end, or there is none. Joins the
    /// next item, waits for one, or starts draining. Returns whether rendering goes
    /// on.
    fn end_of_current(&mut self) -> bool {
        if let Some(source) = &mut self.current
            && let Some(error) = source.take_error()
            && let Some(segment) = self.timeline.last_mut()
            && segment.item.id == source.item().id
        {
            segment.error = Some(error);
        }
        if self.next.is_some() && !self.drain.is_some_and(|drain| drain.reopen) {
            return self.join_next();
        }
        if self.next.is_none() && self.waits_for_next() {
            return false;
        }
        self.start_drain(self.next.is_some());
        false
    }

    /// Whether to hold at the end of an item for the answer to `NearEnd`: only a
    /// freewheeling sink, which has no deadline, waits.
    fn waits_for_next(&mut self) -> bool {
        let freewheel = self.output.as_ref().is_some_and(|output| output.freewheel);
        if !freewheel || self.next_answered {
            return false;
        }
        let since = *self.waiting_since.get_or_insert_with(Instant::now);
        since.elapsed() < self.config.next_wait
    }

    /// Makes the next item current, joined sample-exactly to the audio written so
    /// far if it takes the same stream format; otherwise drains first. Returns
    /// whether rendering goes on.
    fn join_next(&mut self) -> bool {
        let Some(source) = self.next.take() else {
            return false;
        };
        let Some(output) = &self.output else {
            self.next = Some(source);
            return false;
        };
        if self.stream_config(&source).spec != output.requested {
            self.next = Some(source);
            self.start_drain(true);
            return false;
        }
        let at = output.writer.frames_written();
        let id = source.item().id;
        self.current = Some(source);
        self.next_answered = false;
        self.waiting_since = None;
        if let Err(error) = self.attach_current() {
            self.current = None;
            self.error(Some(id), error);
            self.emit(EngineEvent::TrackEnded {
                item: id,
                reason: TrackEndReason::Failed,
            });
            return true;
        }
        self.push_segment(at);
        if self.drain.take().is_some() && self.state == TransportState::Draining {
            self.set_state(TransportState::Playing);
        }
        true
    }

    /// Nothing more is rendered for now; the buffer plays out. With `reopen`, a new
    /// stream is opened for the next item afterwards.
    fn start_drain(&mut self, reopen: bool) {
        if self.drain.is_some_and(|drain| drain.reopen == reopen) {
            return;
        }
        self.drain = Some(Drain {
            reopen,
            empty_since: None,
        });
        if let Some(output) = &self.output {
            output.writer.end_of_audio();
        }
        if !reopen && self.state == TransportState::Playing {
            self.set_state(TransportState::Draining);
        }
    }

    /// Ends draining once the buffer and the device have played everything out.
    pub(super) fn check_drain(&mut self) {
        let (Some(mut drain), Some(output)) = (self.drain, &self.output) else {
            return;
        };
        if !self.staged.is_empty() {
            return;
        }
        let timing = output.stream.timing();
        if timing.queued_frames() > 0 {
            return;
        }
        let since = *drain.empty_since.get_or_insert_with(Instant::now);
        self.drain = Some(drain);
        let delay =
            Duration::from_secs_f64(timing.delay_frames as f64 / f64::from(timing.rate.hz()));
        if since.elapsed() < delay {
            return;
        }
        self.drain = None;
        let mut events = std::mem::take(&mut self.pending);
        self.timeline.finish(&mut events);
        for event in events.drain(..) {
            self.emit(event);
        }
        self.pending = events;
        if drain.reopen {
            self.reopen_for_next();
        } else {
            self.close_output();
            self.reset_items();
            self.set_state(TransportState::Stopped);
        }
    }

    /// After draining for a change of format: opens a stream for the next item and
    /// plays it.
    fn reopen_for_next(&mut self) {
        self.set_state(TransportState::Reconfiguring);
        self.close_output();
        let Some(source) = self.next.take() else {
            self.reset_items();
            self.set_state(TransportState::Stopped);
            return;
        };
        let resume = Resume {
            segment: self.segment_of(&source, 0),
            follower: None,
            position: MediaTime::ZERO,
        };
        let started = resume.segment.started_event();
        self.current = Some(source);
        self.next_answered = false;
        self.near_end_sent = None;
        match self.play_current(None) {
            Ok(()) => {
                self.emit(started);
                self.settle_playing();
                self.send_position_of(resume.segment.item.id, MediaTime::ZERO);
            }
            Err(error) => {
                self.emit(started);
                self.output_failed(error, resume);
            }
        }
    }

    /// Fades out on the device side and waits until it is silent, rendering on
    /// meanwhile. Returns the ring position where silence began, or `None` when the
    /// stream is not playing or the device did not answer in time.
    pub(super) fn fade_out(&mut self, transition: Transition) -> Option<u64> {
        let duration = self.config.processing.fades.of(transition);
        let output = self.output.as_mut()?;
        if !output.running
            || !output.writer.schedule(
                0,
                DeviceAction::Fade {
                    open: false,
                    duration,
                },
            )
        {
            return None;
        }
        let deadline = Instant::now() + duration + DEVICE_WAIT;
        loop {
            let output = self.output.as_mut()?;
            if output.writer.is_silent() {
                return Some(output.writer.silent_since());
            }
            if output.stream.state().is_finished() || Instant::now() >= deadline {
                return None;
            }
            self.fill();
            if let Some(output) = self.output.as_mut()
                && output.freewheel
                && self.staged.is_empty()
                && output.stream.timing().queued_frames() < 1024
            {
                // A freewheeling sink takes frames only when there are some: give it
                // silence to finish the fade on.
                output.writer.write(&SILENCE);
            }
            thread::sleep(POLL);
        }
    }

    /// The heard ring position now.
    pub(super) fn heard_now(&self) -> u64 {
        self.output.as_ref().map_or(0, |output| {
            self.timeline
                .correct(output.stream.timing().heard_position())
                .min(output.writer.frames_written())
        })
    }

    /// Where to continue from the ring position `at`.
    pub(super) fn resume_at(&self, at: u64) -> Option<Resume> {
        let (segment, position) = self.timeline.position(at)?;
        Some(Resume {
            segment: segment.clone(),
            follower: self.timeline.follower().map(|s| s.item.clone()),
            position,
        })
    }

    /// Fades out (if playing) and returns where playback stood.
    pub(super) fn stop_where_heard(&mut self, transition: Transition) -> Option<Resume> {
        let at = self
            .fade_out(transition)
            .unwrap_or_else(|| self.heard_now());
        self.advance_to(at);
        self.resume_at(at)
    }

    /// Puts the sources where `resume` says: the heard item at its position, and the
    /// item that followed it.
    pub(super) fn restart_sources(&mut self, resume: &Resume) -> Result<(), EngineError> {
        let id = resume.segment.item.id;
        if let Some(source) = self.current.as_mut().filter(|s| s.item().id == id) {
            source.seek(resume.position)?;
        } else {
            let old = self.current.take();
            let mut source = Source::open(&self.registry, resume.segment.item.clone())?;
            source.seek(resume.position)?;
            self.current = Some(source);
            if let Some(follower) = &resume.follower {
                let reused = old.filter(|s| s.item().id == follower.id);
                self.next = match reused {
                    Some(mut source) => source.seek(MediaTime::ZERO).ok().map(|_| source),
                    None => Source::open(&self.registry, follower.clone()).ok(),
                };
                self.next_answered = self.next.is_some();
            }
        }
        self.drain = None;
        self.waiting_since = None;
        Ok(())
    }

    /// Plays the current item from its position: on the open stream if it takes the
    /// item's format (dropping what is buffered), otherwise on a new one. Without a
    /// `transition` the audio starts at full level, as at the start of a track;
    /// with one it fades in.
    pub(super) fn play_current(
        &mut self,
        transition: Option<Transition>,
    ) -> Result<(), EngineError> {
        let config = match &self.current {
            Some(source) => self.stream_config(source),
            None => return Err(EngineError::NothingLoaded),
        };
        if self
            .output
            .as_ref()
            .is_some_and(|o| o.requested != config.spec || o.device != config.device)
        {
            self.close_output();
        }
        if self.output.is_none() {
            self.output = Some(self.open_output(config)?);
        }
        self.attach_current()?;
        self.drain = None;
        self.waiting_since = None;
        self.staged = 0..0;
        let fade = transition.map(|t| self.config.processing.fades.of(t));
        let Some(output) = &self.output else {
            return Err(EngineError::NothingLoaded);
        };
        let until = output.writer.frames_written();
        let running = output.running;
        if until == 0 && !running {
            self.timeline.reset(0);
            self.push_segment(0);
            if let Some(duration) = fade {
                self.schedule(0, DeviceAction::SetOpen(false));
                self.schedule(
                    0,
                    DeviceAction::Fade {
                        open: true,
                        duration,
                    },
                );
            }
            self.fill();
            self.start_output()?;
        } else {
            self.schedule(0, DeviceAction::Discard { until, hold: true });
            self.timeline.reset(until);
            self.push_segment(until);
            if !running {
                self.start_output()?;
            }
            self.refill(until);
            let open = match fade {
                Some(duration) => DeviceAction::Fade {
                    open: true,
                    duration,
                },
                None => DeviceAction::SetOpen(true),
            };
            self.schedule(until, open);
        }
        Ok(())
    }

    /// Renders until a little audio is buffered after the ring position `from`, while
    /// the device side drops what came before.
    fn refill(&mut self, from: u64) {
        let deadline = Instant::now() + DEVICE_WAIT;
        loop {
            self.fill();
            let Some(output) = &self.output else {
                return;
            };
            let target = (f64::from(output.spec().rate.hz()) * REFILL.as_secs_f64()) as u64;
            let target = target.min(output.writer.capacity_frames() as u64 / 2);
            let buffered = output.writer.frames_written().saturating_sub(from);
            let ended = self.current.as_ref().is_none_or(Source::is_finished);
            if buffered >= target
                || ended
                || output.stream.state().is_finished()
                || Instant::now() >= deadline
            {
                return;
            }
            thread::sleep(POLL);
        }
    }

    fn start_output(&mut self) -> Result<(), EngineError> {
        match &mut self.output {
            Some(output) => output.start().map_err(|e| e.to_engine_error()),
            None => Err(EngineError::NothingLoaded),
        }
    }

    fn schedule(&mut self, at: u64, action: DeviceAction) {
        if let Some(output) = &mut self.output {
            // The queue only fills when the device side has stopped taking actions;
            // the stream's state reports that.
            let _ = output.writer.schedule(at, action);
        }
    }

    /// Records that the current item starts at the ring position `at`.
    fn push_segment(&mut self, at: u64) {
        let Some(source) = &self.current else {
            return;
        };
        let segment = self.segment_of(source, at);
        let dither = source.plan().is_some_and(|plan| plan.always_dither);
        self.timeline.push(segment);
        self.schedule(at, DeviceAction::AlwaysDither(dither));
    }

    /// A timeline segment for `source` from its current position.
    pub(super) fn segment_of(&self, source: &Source, at: u64) -> Segment {
        Segment {
            ring_start: at,
            item: source.item().clone(),
            spec: source.spec(),
            length: source.length(),
            start_frames: source.position(),
            device_rate: self
                .output
                .as_ref()
                .map_or(source.rate(), |output| output.spec().rate),
            error: None,
        }
    }
}
