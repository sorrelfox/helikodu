// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The service actor: the one thread that owns the queue and the engine.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use helikodu_engine::transport::Engine;
use helikodu_model::audio::{AudioSpec, MediaTime, OutputSelection};
use helikodu_model::library::MediaSource;
use helikodu_model::player::{
    EngineCommand, EngineError, EngineEvent, ItemId, PlaybackItem, TrackEndReason, TransportState,
    Volume,
};

use super::config::ReplayGainMode;
use super::loudness::LoudnessTags;
use super::outputs::{next_output, output_choices};
use super::queue::{Direction, Queue};
use super::{EndReason, PlayerEvent};

/// How much of a file must have played for [`PlayerCommand::Previous`] to restart it
/// instead of going to the file before.
const RESTART_AFTER: Duration = Duration::from_secs(3);

/// A command to the player.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerCommand {
    /// Replace the queue with `files` and play from the file at `start`.
    PlayFiles {
        /// The files, in the order they play.
        files: Vec<PathBuf>,
        /// The index of the first file to play.
        start: usize,
    },
    /// Pause when playing; play otherwise.
    PlayPause,
    /// Resume, or after a stop play the current file again.
    Play,
    /// Pause, keeping the position.
    Pause,
    /// Stop and release the device.
    Stop,
    /// Go to the next file; past the last one, playback ends.
    Next,
    /// Restart the current file, or go to the one before (see the module docs).
    Previous,
    /// Move to a position in the current file.
    Seek(MediaTime),
    /// Move forward in the current file.
    SeekForward(Duration),
    /// Move back in the current file.
    SeekBackward(Duration),
    /// Set the volume.
    SetVolume(Volume),
    /// Change the volume by this much of the whole range (from -1.0 to 1.0).
    AdjustVolume(f32),
    /// Move playback to another backend, device or mode, keeping the position.
    SetOutput(OutputSelection),
    /// Move playback to the next output device: through the devices of every
    /// sound backend, or of the null backend while it is used.
    NextOutput,
}

/// What arrives in the actor's mailbox.
#[derive(Debug)]
pub(super) enum Input {
    Command(PlayerCommand),
    Engine(EngineEvent),
    Shutdown,
}

/// A file handed to the engine.
#[derive(Debug, Clone, Copy)]
struct Slot {
    id: ItemId,
    index: usize,
    length: Option<MediaTime>,
}

#[derive(Debug)]
pub(super) struct Actor {
    engine: Engine,
    events: Sender<PlayerEvent>,
    queue: Queue,
    /// The loudness tags of the queued files.
    loudness: LoudnessTags,
    last_id: u64,
    /// The file playing, or being loaded.
    current: Option<Slot>,
    /// The file sent with `SetNext`, until it starts.
    pending: Option<Slot>,
    /// The item of a `Load` that has neither started nor failed yet.
    in_flight: Option<ItemId>,
    /// The index of a file that failed as the next one: the following `NearEnd`
    /// looks after it.
    failed_next: Option<usize>,
    direction: Direction,
    engine_state: TransportState,
    position: MediaTime,
    volume: Volume,
    selection: OutputSelection,
    /// No file is to follow the current one.
    exhausted: bool,
    stop_requested: bool,
    /// `Ended` has been sent, or nothing has played yet.
    ended: bool,
    played: usize,
    failed: usize,
}

impl Actor {
    pub(super) fn new(
        engine: Engine,
        events: Sender<PlayerEvent>,
        selection: OutputSelection,
        volume: Volume,
        replay_gain: ReplayGainMode,
    ) -> Self {
        Self {
            engine,
            events,
            queue: Queue::default(),
            loudness: LoudnessTags::new(replay_gain),
            last_id: 0,
            current: None,
            pending: None,
            in_flight: None,
            failed_next: None,
            direction: Direction::Forward,
            engine_state: TransportState::Stopped,
            position: MediaTime::ZERO,
            volume,
            selection,
            exhausted: false,
            stop_requested: false,
            ended: true,
            played: 0,
            failed: 0,
        }
    }

    pub(super) fn run(mut self, mailbox: &Receiver<Input>) {
        for input in mailbox {
            match input {
                Input::Command(command) => self.command(command),
                Input::Engine(event) => self.engine_event(event),
                Input::Shutdown => return,
            }
        }
    }

    fn emit(&self, event: PlayerEvent) {
        // Nobody listening is no reason to stop playing.
        let _ = self.events.send(event);
    }

    fn send(&self, command: EngineCommand) {
        if let Err(gone) = self.engine.send(command) {
            self.emit(PlayerEvent::Problem {
                file: None,
                error: EngineError::Output {
                    reason: gone.to_string(),
                },
            });
        }
    }

    fn command(&mut self, command: PlayerCommand) {
        match command {
            PlayerCommand::PlayFiles { files, start } => self.play_files(files, start),
            PlayerCommand::PlayPause => match self.engine_state {
                TransportState::Paused | TransportState::Stopped => self.play(),
                _ => self.send(EngineCommand::Pause),
            },
            PlayerCommand::Play => self.play(),
            PlayerCommand::Pause => {
                if !self.is_stopped() {
                    self.send(EngineCommand::Pause);
                }
            }
            PlayerCommand::Stop => {
                if !self.is_stopped() {
                    self.stop_requested = true;
                    self.send(EngineCommand::Stop);
                }
            }
            PlayerCommand::Next => self.next(),
            PlayerCommand::Previous => self.previous(),
            PlayerCommand::Seek(to) => self.seek(to),
            PlayerCommand::SeekForward(by) => {
                let to = self.position.as_duration().saturating_add(by);
                self.seek(MediaTime::from_duration(to));
            }
            PlayerCommand::SeekBackward(by) => {
                let to = self.position.as_duration().saturating_sub(by);
                self.seek(MediaTime::from_duration(to));
            }
            PlayerCommand::SetVolume(volume) => self.set_volume(volume),
            PlayerCommand::AdjustVolume(by) => {
                self.set_volume(Volume::clamped(self.volume.position() + by));
            }
            PlayerCommand::SetOutput(selection) => self.set_output(selection),
            PlayerCommand::NextOutput => {
                let choices = output_choices(&self.selection);
                match next_output(&self.selection, &choices) {
                    Some(selection) => self.set_output(selection),
                    None => self.emit(PlayerEvent::Problem {
                        file: None,
                        error: EngineError::Output {
                            reason: "there is no other output device".into(),
                        },
                    }),
                }
            }
        }
    }

    /// Whether the engine is stopped with nothing on the way.
    fn is_stopped(&self) -> bool {
        self.engine_state == TransportState::Stopped && self.in_flight.is_none()
    }

    fn play_files(&mut self, files: Vec<PathBuf>, start: usize) {
        self.queue = Queue::new(files);
        self.loudness.clear();
        self.played = 0;
        self.failed = 0;
        self.current = None;
        if self.queue.get(start).is_some() {
            self.load(start, Direction::Forward);
        } else {
            if !self.is_stopped() {
                self.send(EngineCommand::Stop);
            }
            self.ended = true;
            self.emit(PlayerEvent::Ended {
                reason: EndReason::QueueFinished,
                played: 0,
                failed: 0,
            });
        }
    }

    /// Loads the file at `index`, replacing whatever plays.
    fn load(&mut self, index: usize, direction: Direction) {
        let Some(path) = self.queue.get(index) else {
            return;
        };
        self.last_id += 1;
        let id = ItemId::new(self.last_id);
        let item = PlaybackItem::new(id, MediaSource::file(path));
        let item = self.loudness.complete(&self.queue, index, item);
        self.current = Some(Slot {
            id,
            index,
            length: None,
        });
        self.pending = None;
        self.in_flight = Some(id);
        self.failed_next = None;
        self.direction = direction;
        self.position = MediaTime::ZERO;
        self.exhausted = false;
        self.stop_requested = false;
        self.ended = false;
        self.send(EngineCommand::Load {
            item,
            start: MediaTime::ZERO,
            paused: false,
        });
    }

    fn play(&mut self) {
        match self.engine_state {
            TransportState::Paused => self.send(EngineCommand::Play),
            TransportState::Stopped if self.in_flight.is_none() => {
                let index = self.current.map_or(0, |slot| slot.index);
                self.load(index, Direction::Forward);
            }
            _ => {}
        }
    }

    fn next(&mut self) {
        let Some(current) = self.current else {
            return;
        };
        match self.queue.step(current.index, Direction::Forward) {
            Some(index) => self.load(index, Direction::Forward),
            None if !self.is_stopped() => {
                self.exhausted = true;
                self.send(EngineCommand::Stop);
            }
            None => {}
        }
    }

    fn previous(&mut self) {
        let Some(current) = self.current else {
            return;
        };
        let restart = !self.is_stopped() && self.position.as_duration() > RESTART_AFTER;
        match self.queue.step(current.index, Direction::Backward) {
            Some(index) if !restart => self.load(index, Direction::Backward),
            _ if self.is_stopped() => self.load(current.index, Direction::Forward),
            _ => self.seek(MediaTime::ZERO),
        }
    }

    fn seek(&mut self, to: MediaTime) {
        if !self.is_stopped() {
            self.send(EngineCommand::Seek(to));
        }
    }

    fn set_volume(&mut self, volume: Volume) {
        self.volume = volume;
        self.send(EngineCommand::SetVolume(volume));
        self.emit(PlayerEvent::VolumeChanged(volume));
    }

    fn set_output(&mut self, selection: OutputSelection) {
        self.selection = selection.clone();
        self.send(EngineCommand::SetOutput(selection.clone()));
        self.emit(PlayerEvent::OutputChanged(selection));
    }

    fn engine_event(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::StateChanged(state) => {
                self.engine_state = state;
                self.emit(PlayerEvent::State(state));
                self.end_if_stopped();
            }
            EngineEvent::TrackStarted { item, spec, length } => {
                self.track_started(item, spec, length);
            }
            EngineEvent::Position { item, position } => {
                if let Some(current) = self.current.filter(|slot| slot.id == item) {
                    self.position = position;
                    self.emit(PlayerEvent::Position {
                        index: current.index,
                        position,
                        length: current.length,
                    });
                }
            }
            EngineEvent::NearEnd { item } => self.near_end(item),
            EngineEvent::TrackEnded { item, reason } => self.track_ended(item, reason),
            EngineEvent::Error { item, error } => {
                let file = item.and_then(|id| self.slot(id)).and_then(|slot| {
                    let path = self.queue.get(slot.index)?;
                    Some((slot.index, path.to_path_buf()))
                });
                self.emit(PlayerEvent::Problem { file, error });
            }
            EngineEvent::DevicesChanged { .. } => {}
        }
    }

    /// The current or the pending slot with the identifier `id`.
    fn slot(&self, id: ItemId) -> Option<Slot> {
        [self.current, self.pending]
            .into_iter()
            .flatten()
            .find(|slot| slot.id == id)
    }

    fn track_started(&mut self, item: ItemId, spec: AudioSpec, length: Option<MediaTime>) {
        if self.in_flight == Some(item) {
            self.in_flight = None;
        }
        let slot = match (self.current, self.pending) {
            (Some(current), _) if current.id == item => current,
            (_, Some(pending)) if pending.id == item => {
                self.pending = None;
                pending
            }
            _ => return,
        };
        let slot = Slot { length, ..slot };
        self.current = Some(slot);
        self.played += 1;
        self.position = MediaTime::ZERO;
        self.direction = Direction::Forward;
        let path = self.queue.get(slot.index).map(PathBuf::from);
        self.emit(PlayerEvent::TrackChanged {
            index: slot.index,
            count: self.queue.len(),
            path: path.unwrap_or_default(),
            spec,
            length,
        });
    }

    fn near_end(&mut self, item: ItemId) {
        let Some(current) = self.current.filter(|slot| slot.id == item) else {
            return;
        };
        let after = self.failed_next.take().unwrap_or(current.index);
        let next = self.queue.step(after, Direction::Forward);
        let Some((index, path)) = next.and_then(|i| Some((i, self.queue.get(i)?))) else {
            self.pending = None;
            self.send(EngineCommand::SetNext(None));
            return;
        };
        self.last_id += 1;
        let id = ItemId::new(self.last_id);
        let item = PlaybackItem::new(id, MediaSource::file(path));
        let item = self.loudness.complete(&self.queue, index, item);
        self.pending = Some(Slot {
            id,
            index,
            length: None,
        });
        self.send(EngineCommand::SetNext(Some(item)));
    }

    fn track_ended(&mut self, item: ItemId, reason: TrackEndReason) {
        if reason == TrackEndReason::Failed
            && let Some(pending) = self.pending.filter(|slot| slot.id == item)
        {
            // The engine asks for another one with `NearEnd`.
            self.failed += 1;
            self.failed_next = Some(pending.index);
            self.pending = None;
            return;
        }
        let Some(current) = self.current.filter(|slot| slot.id == item) else {
            return;
        };
        if reason == TrackEndReason::Failed {
            self.failed += 1;
        }
        if self.in_flight == Some(item) {
            // The file did not open: skip it.
            self.in_flight = None;
            if reason == TrackEndReason::Failed && !self.stop_requested {
                let mut direction = self.direction;
                let mut next = self.queue.step(current.index, direction);
                if next.is_none() && direction == Direction::Backward {
                    direction = Direction::Forward;
                    next = self.queue.step(current.index, direction);
                }
                if let Some(index) = next {
                    self.load(index, direction);
                    return;
                }
                self.exhausted = true;
            }
        } else if reason != TrackEndReason::Interrupted && self.pending.is_none() {
            self.exhausted = true;
        }
        self.end_if_stopped();
    }

    /// Sends `Ended` once the engine has stopped for good.
    fn end_if_stopped(&mut self) {
        if self.ended || !self.is_stopped() {
            return;
        }
        self.ended = true;
        let reason = if self.stop_requested {
            EndReason::Stopped
        } else if self.exhausted {
            EndReason::QueueFinished
        } else {
            EndReason::OutputFailed
        };
        self.emit(PlayerEvent::Ended {
            reason,
            played: self.played,
            failed: self.failed,
        });
    }
}
