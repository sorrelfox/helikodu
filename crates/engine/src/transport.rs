// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The transport: the engine thread that plays what it is told.
//!
//! [`Engine::spawn`] starts the engine thread and returns a handle that takes
//! [`EngineCommand`]s, and the receiver of the [`EngineEvent`]s the engine sends.
//! The engine decides nothing about what plays next: it asks with
//! [`EngineEvent::NearEnd`], and the sender answers with
//! [`EngineCommand::SetNext`].
//!
//! ```no_run
//! use helikodu_engine::transport::{Engine, EngineConfig};
//! use helikodu_model::audio::MediaTime;
//! use helikodu_model::library::MediaSource;
//! use helikodu_model::player::{EngineCommand, EngineEvent, ItemId, PlaybackItem};
//!
//! let (engine, events) = Engine::spawn(EngineConfig::default())?;
//! let item = PlaybackItem::new(ItemId::new(1), MediaSource::file("/music/a.flac"));
//! engine.send(EngineCommand::Load { item, start: MediaTime::ZERO, paused: false })?;
//! for event in events.iter() {
//!     if let EngineEvent::NearEnd { .. } = event {
//!         engine.send(EngineCommand::SetNext(None))?;
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # States and events
//!
//! The states are those of [`TransportState`](helikodu_model::player::TransportState):
//!
//! - `Load` sends `TrackEnded` (`Interrupted`) for the item it replaces, then
//!   `StateChanged(Loading)`, `TrackStarted`, `StateChanged(Playing)` (or `Paused`)
//!   and `Position`. An item that does not open gives `Error` and `TrackEnded`
//!   (`Failed`), and the engine stops.
//! - `Pause` fades out and sends `StateChanged(Paused)` and the `Position` where the
//!   sound stopped; `Play` continues from exactly there with a fade in.
//! - `Seek` sends `StateChanged(Seeking)`, fades out, moves, fades in, and sends
//!   `StateChanged(Playing)` and the new `Position`. While paused it only moves the
//!   position.
//! - `Stop` fades out, sends `TrackEnded` (`Interrupted`) and `StateChanged(Stopped)`,
//!   closes the device and unloads the item; `Play` then answers
//!   [`EngineError::NothingLoaded`](helikodu_model::player::EngineError::NothingLoaded).
//! - `SetOutput` while playing sends `StateChanged(Reconfiguring)`, fades out, opens
//!   the new device and continues at the same position.
//! - `UpdateSettings` replaces the output and loudness settings. A new sample rate
//!   policy, sample format, buffer length or volume control reopens the stream the
//!   same way (`Reconfiguring`) while playing, and on `Play` while paused. A loudness
//!   change keeps the stream: the gain ramps (20 ms) to its new value from the next
//!   rendered frame, which is heard after the audio already buffered.
//! - At the end of an item with a next one, the next one joins without a gap if it
//!   takes the same stream format (sample rate, sample format and channels the
//!   settings ask the device for); otherwise the buffer plays out and the device is
//!   reopened (`Reconfiguring`). `TrackEnded` (`Finished`) and `TrackStarted` are
//!   sent when the join is heard.
//! - At the end of the last item the engine sends `StateChanged(Draining)`, and once
//!   the buffer has played out `TrackEnded` and `StateChanged(Stopped)`.
//! - `NearEnd` is sent once per item, when the heard position is within
//!   [`EngineConfig::near_end`] of its end, or when it has been decoded to its end.
//!   A `SetNext` item that does not open gives `Error` and `TrackEnded` (`Failed`)
//!   at once, and `NearEnd` is sent again. An item that fails while decoding ends
//!   there, with `Error` and `TrackEnded` (`Failed`) when that point is heard, and
//!   playback goes on with the next item.
//! - `Position` is sent every [`EngineConfig::position_interval`] while playing,
//!   from the frames the device has played (its delay subtracted), and not while
//!   paused.
//! - When the device goes away the engine sends `Error` and applies
//!   [`OutputSettings::on_device_lost`](helikodu_model::settings::OutputSettings::on_device_lost):
//!   pause, continue on the default device, or stop. The backend's device list is
//!   sent as `DevicesChanged` when it is connected and when it changes.
//!
//! # Fades and volume
//!
//! Volume, the anti-click fades and dither run on the device side of the ring
//! buffer ([`DeviceAction`](crate::output::DeviceAction)), so pause, seek, stop and
//! volume changes are heard within one device period, not after the buffer. A track
//! that starts from its beginning is not faded in, so the path stays bit-perfect.

mod commands;
mod config;
mod device;
mod recovery;
mod render;
mod settings;
mod source;
mod stats;
mod timeline;
mod worker;

use std::fmt;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use helikodu_model::player::{EngineCommand, EngineEvent};

pub use self::config::EngineConfig;
pub use self::stats::{EngineStats, StatsSnapshot};
use self::worker::{Input, Worker};
use crate::decode::DecoderRegistry;
use crate::output::OutputEvent;

/// A running engine. Dropping it stops playback, closes the device and ends the
/// engine thread.
#[derive(Debug)]
pub struct Engine {
    sender: EngineSender,
    stats: EngineStats,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    /// Starts an engine with the default decoders.
    pub fn spawn(config: EngineConfig) -> std::io::Result<(Self, Receiver<EngineEvent>)> {
        Self::spawn_with_registry(config, DecoderRegistry::default())
    }

    /// Starts an engine that opens items with `registry`.
    pub fn spawn_with_registry(
        config: EngineConfig,
        registry: DecoderRegistry,
    ) -> std::io::Result<(Self, Receiver<EngineEvent>)> {
        let (inputs, input_receiver) = mpsc::channel();
        let (events, event_receiver) = mpsc::channel();
        let (output_events, output_receiver) = mpsc::channel::<OutputEvent>();
        // Backends report on a channel of their own; forward it into the engine's.
        let forward = inputs.clone();
        thread::Builder::new()
            .name("helikodu-output-events".into())
            .spawn(move || {
                for event in output_receiver {
                    if forward.send(Input::Output(event)).is_err() {
                        break;
                    }
                }
            })?;
        let stats = EngineStats::default();
        let worker = Worker::new(
            config,
            registry,
            input_receiver,
            events,
            output_events,
            stats.clone(),
        );
        let thread = thread::Builder::new()
            .name("helikodu-engine".into())
            .spawn(move || worker.run())?;
        let engine = Self {
            sender: EngineSender(inputs),
            stats,
            thread: Some(thread),
        };
        Ok((engine, event_receiver))
    }

    /// Sends a command. Fails only if the engine thread has ended.
    pub fn send(&self, command: EngineCommand) -> Result<(), EngineGone> {
        self.sender.send(command)
    }

    /// A handle for sending commands from other threads.
    pub fn sender(&self) -> EngineSender {
        self.sender.clone()
    }

    /// The output counters (underruns, frames delivered, streams opened), summed
    /// over every stream the engine has opened. The handle stays valid after the
    /// engine is dropped.
    pub fn stats(&self) -> EngineStats {
        self.stats.clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.sender.0.send(Input::Shutdown);
        if let Some(thread) = self.thread.take() {
            // A panic on the engine thread has already been reported by the hook.
            let _ = thread.join();
        }
    }
}

/// Sends commands to an [`Engine`]. Cloning gives another handle to the same engine.
#[derive(Debug, Clone)]
pub struct EngineSender(Sender<Input>);

impl EngineSender {
    /// Sends a command. Fails only if the engine thread has ended.
    pub fn send(&self, command: EngineCommand) -> Result<(), EngineGone> {
        self.0.send(Input::Command(command)).map_err(|_| EngineGone)
    }
}

/// The engine thread has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineGone;

impl fmt::Display for EngineGone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the engine has shut down")
    }
}

impl std::error::Error for EngineGone {}
