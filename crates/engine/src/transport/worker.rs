// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine thread: its state, its loop, and the events it sends.
//!
//! One thread does both control and rendering. It waits for input (commands, and the
//! backends' events through a forwarding thread) with a timeout; while playing, the
//! timeout is a few milliseconds and every wake-up tops up the ring buffer, follows
//! the heard position and sends the events that are due. While stopped or paused it
//! sleeps until the next input.
//!
//! Commands are carried out one at a time and to the end. The transitions that wait
//! for the device (a fade out before a pause, seek, stop or device switch; refilling
//! the buffer after one) take a few tens of milliseconds and keep rendering while
//! they wait.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use helikodu_model::audio::{MediaTime, OutputMode, OutputSelection};
use helikodu_model::player::{
    EngineCommand, EngineError, EngineEvent, ItemId, PlaybackItem, TransportState, Volume,
};

use super::EngineConfig;
use super::device::{Backends, Output};
use super::source::Source;
use super::stats::EngineStats;
use super::timeline::{Segment, Timeline};
use crate::decode::DecoderRegistry;
use crate::output::{EventSender, OutputEvent};

/// What reaches the engine thread.
#[derive(Debug)]
pub(super) enum Input {
    Command(EngineCommand),
    Output(OutputEvent),
    Shutdown,
}

/// Where playback continues while paused: the heard item, the item written after it
/// (if the render side had already joined it), and the position.
#[derive(Debug, Clone)]
pub(super) struct Resume {
    pub(super) segment: Segment,
    pub(super) follower: Option<PlaybackItem>,
    pub(super) position: MediaTime,
}

/// The end of the rendered audio, waiting for the buffer to play out.
#[derive(Debug, Clone, Copy)]
pub(super) struct Drain {
    /// Open a new stream for the next item afterwards (its format differs).
    pub(super) reopen: bool,
    /// When the ring buffer was first seen empty.
    pub(super) empty_since: Option<Instant>,
}

/// The engine thread's state.
#[derive(Debug)]
pub(super) struct Worker {
    pub(super) config: EngineConfig,
    pub(super) registry: DecoderRegistry,
    inputs: Receiver<Input>,
    events: Sender<EngineEvent>,
    pub(super) backends: Backends,
    pub(super) selection: OutputSelection,
    pub(super) state: TransportState,
    pub(super) volume: Volume,
    pub(super) output: Option<Output>,
    /// The item the render side decodes.
    pub(super) current: Option<Source>,
    /// The item to join when `current` ends, opened ahead.
    pub(super) next: Option<Source>,
    /// Whether the item after `current` has been decided (a `SetNext`, even of
    /// nothing, since `current` started).
    pub(super) next_answered: bool,
    pub(super) timeline: Timeline,
    /// Rendered frames on their way into the ring buffer.
    pub(super) staging: Vec<f32>,
    pub(super) staged: std::ops::Range<usize>,
    pub(super) resume: Option<Resume>,
    pub(super) drain: Option<Drain>,
    /// Since when a freewheeling sink waits for the next item.
    pub(super) waiting_since: Option<Instant>,
    pub(super) near_end_sent: Option<ItemId>,
    pub(super) paused_since: Option<Instant>,
    last_position: Option<Instant>,
    /// Events collected while advancing the timeline.
    pub(super) pending: Vec<EngineEvent>,
    /// The output counters shared with the [`Engine`](super::Engine) handle.
    pub(super) stats: EngineStats,
}

impl Worker {
    pub(super) fn new(
        config: EngineConfig,
        registry: DecoderRegistry,
        inputs: Receiver<Input>,
        events: Sender<EngineEvent>,
        output_events: EventSender,
        stats: EngineStats,
    ) -> Self {
        let selection = config.output.selection();
        let volume = config.volume;
        let backends = Backends::new(
            output_events,
            config.sink_pacing,
            config.device_reservation.clone(),
        );
        Self {
            config,
            registry,
            inputs,
            events,
            backends,
            selection,
            state: TransportState::Stopped,
            volume,
            output: None,
            current: None,
            next: None,
            next_answered: false,
            timeline: Timeline::default(),
            staging: Vec::new(),
            staged: 0..0,
            resume: None,
            drain: None,
            waiting_since: None,
            near_end_sent: None,
            paused_since: None,
            last_position: None,
            pending: Vec::new(),
            stats,
        }
    }

    /// Runs until the engine is shut down or dropped.
    pub(super) fn run(mut self) {
        loop {
            let input = match self.wait_time() {
                Some(timeout) => match self.inputs.recv_timeout(timeout) {
                    Ok(input) => Some(input),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match self.inputs.recv() {
                    Ok(input) => Some(input),
                    Err(_) => break,
                },
            };
            if let Some(input) = input {
                if !self.handle(input) {
                    break;
                }
                // Take whatever else has arrived before rendering.
                while let Ok(input) = self.inputs.try_recv() {
                    if !self.handle(input) {
                        self.shut_down();
                        return;
                    }
                }
            }
            self.tick();
        }
        self.shut_down();
    }

    /// How long to wait for input: briefly while audio flows, until the release of an
    /// exclusive device while paused, otherwise for ever.
    fn wait_time(&self) -> Option<Duration> {
        match self.state {
            TransportState::Playing
            | TransportState::Draining
            | TransportState::Seeking
            | TransportState::Reconfiguring
            | TransportState::Loading => {
                let buffer = Duration::from_millis(u64::from(self.config.output.buffer_ms));
                Some((buffer / 16).clamp(Duration::from_millis(2), Duration::from_millis(10)))
            }
            TransportState::Paused => self
                .release_deadline()
                .map(|deadline| deadline.saturating_duration_since(Instant::now())),
            TransportState::Stopped => None,
        }
    }

    /// When a paused exclusive stream is to be closed.
    fn release_deadline(&self) -> Option<Instant> {
        let output = self.output.as_ref()?;
        let seconds = self.config.output.exclusive_release_s;
        (output.mode == OutputMode::Exclusive && seconds > 0)
            .then(|| {
                self.paused_since
                    .map(|since| since + Duration::from_secs(seconds.into()))
            })
            .flatten()
    }

    /// Carries out one input. `false` ends the thread.
    fn handle(&mut self, input: Input) -> bool {
        match input {
            Input::Command(command) => self.command(command),
            Input::Output(event) => self.output_event(event),
            Input::Shutdown => return false,
        }
        true
    }

    fn command(&mut self, command: EngineCommand) {
        match command {
            EngineCommand::Load {
                item,
                start,
                paused,
            } => self.load(item, start, paused),
            EngineCommand::Play => self.play(),
            EngineCommand::Pause => self.pause(),
            EngineCommand::Stop => self.stop(),
            EngineCommand::Seek(to) => self.seek(to),
            EngineCommand::SetNext(item) => self.set_next(item),
            EngineCommand::SetVolume(volume) => {
                self.volume = volume;
                if let Some(output) = &mut self.output {
                    output.set_volume(volume);
                }
            }
            EngineCommand::SetOutput(selection) => self.set_output(selection),
        }
    }

    fn output_event(&mut self, event: OutputEvent) {
        let backend = match &event {
            OutputEvent::DeviceAdded(info) => info.backend,
            OutputEvent::DeviceRemoved { backend, .. }
            | OutputEvent::DefaultDeviceChanged { backend, .. } => *backend,
            OutputEvent::DeviceLost { .. } | OutputEvent::Error { .. } => {
                // The stream's own state says whether it is ours; `tick` acts on it.
                self.check_stream();
                return;
            }
        };
        if let Some(connected) = self.backends.connected(backend)
            && let Ok(devices) = connected.devices()
        {
            self.emit(EngineEvent::DevicesChanged { backend, devices });
        }
    }

    /// The periodic work: feeding the device, following the heard position, ending
    /// the buffer, releasing a paused exclusive device.
    fn tick(&mut self) {
        match self.state {
            TransportState::Playing | TransportState::Draining => {
                if self.check_stream() {
                    return;
                }
                self.fill();
                self.follow();
                if let Some(output) = &mut self.output {
                    output.publish_stats();
                }
                self.check_drain();
            }
            TransportState::Paused => {
                if self.release_deadline().is_some_and(|d| Instant::now() >= d)
                    && let Some(output) = self.output.take()
                {
                    output.close();
                }
            }
            _ => {}
        }
    }

    /// Follows the heard position: item starts and ends, `NearEnd`, position ticks.
    pub(super) fn follow(&mut self) {
        let Some(output) = &self.output else {
            return;
        };
        let timing = output.stream.timing();
        let at = self
            .timeline
            .correct(timing.heard_position())
            .min(output.writer.frames_written());
        self.advance_to(at);
        self.check_near_end(at);
        let due = self
            .last_position
            .is_none_or(|last| last.elapsed() >= self.config.position_interval);
        if due {
            self.send_position(at);
        }
    }

    /// Moves the timeline to `at` and sends the events of what was passed.
    pub(super) fn advance_to(&mut self, at: u64) {
        let mut events = std::mem::take(&mut self.pending);
        self.timeline.advance(at, &mut events);
        for event in events.drain(..) {
            if let EngineEvent::TrackStarted { .. } = event {
                self.near_end_sent = None;
            }
            self.emit(event);
        }
        self.pending = events;
    }

    fn check_near_end(&mut self, at: u64) {
        let Some((segment, position)) = self.timeline.position(at) else {
            return;
        };
        let id = segment.item.id;
        if self.near_end_sent == Some(id) {
            return;
        }
        let close = segment.length.is_some_and(|length| {
            length.as_duration().saturating_sub(position.as_duration()) <= self.config.near_end
        });
        // Rendered to the end already: it ends within the buffer length.
        let rendered = self.timeline.len() > 1
            || self
                .current
                .as_ref()
                .is_none_or(|source| source.item().id != id || source.is_finished());
        if close || rendered {
            self.near_end_sent = Some(id);
            self.emit(EngineEvent::NearEnd { item: id });
        }
    }

    /// Sends the position heard at the ring position `at`.
    pub(super) fn send_position(&mut self, at: u64) {
        if let Some((segment, position)) = self.timeline.position(at) {
            let item = segment.item.id;
            self.emit(EngineEvent::Position { item, position });
        }
        self.last_position = Some(Instant::now());
    }

    /// Sends a position that is known without the timeline (after a seek, while
    /// paused).
    pub(super) fn send_position_of(&mut self, item: ItemId, position: MediaTime) {
        self.emit(EngineEvent::Position { item, position });
        self.last_position = Some(Instant::now());
    }

    /// Acts on a stream whose device went away or that failed. Returns whether it
    /// did.
    pub(super) fn check_stream(&mut self) -> bool {
        let Some(output) = &self.output else {
            return false;
        };
        let state = output.stream.state();
        if !state.is_finished() || state == crate::output::StreamState::Closed {
            return false;
        }
        let error = if state == crate::output::StreamState::Lost {
            EngineError::DeviceLost {
                backend: output.stream.backend(),
                device: output.device.clone(),
            }
        } else {
            EngineError::Output {
                reason: format!("the {} stream failed", output.stream.backend()),
            }
        };
        self.device_failed(error);
        true
    }

    pub(super) fn set_state(&mut self, state: TransportState) {
        if self.state != state {
            self.state = state;
            self.paused_since = (state == TransportState::Paused).then(Instant::now);
            self.emit(EngineEvent::StateChanged(state));
        }
    }

    pub(super) fn emit(&self, event: EngineEvent) {
        // Nobody listening is no reason to stop playing.
        let _ = self.events.send(event);
    }

    pub(super) fn error(&self, item: Option<ItemId>, error: EngineError) {
        self.emit(EngineEvent::Error { item, error });
    }

    fn shut_down(&mut self) {
        if let Some(output) = self.output.take() {
            output.close();
        }
        self.current = None;
        self.next = None;
    }
}
