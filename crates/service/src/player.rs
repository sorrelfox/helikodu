// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The player service: a queue of files played by the engine.
//!
//! [`Player::spawn`] starts the service actor, which owns an engine and the queue.
//! Front ends send [`PlayerCommand`]s with [`Player::send`] and receive
//! [`PlayerEvent`]s; every change of state goes through the actor's mailbox.
//!
//! ```no_run
//! use helikodu_model::audio::OutputSelection;
//! use helikodu_service::player::{Player, PlayerCommand, PlayerConfig, PlayerEvent};
//!
//! let config = PlayerConfig::new(OutputSelection::default());
//! let (player, events) = Player::spawn(config)?;
//! player.send(PlayerCommand::PlayFiles { files: vec!["a.flac".into()], start: 0 })?;
//! for event in events.iter() {
//!     if let PlayerEvent::Ended { .. } = event {
//!         break;
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Playing the queue
//!
//! - The queue plays in order. Each next file is handed to the engine when it asks
//!   for one ([`NearEnd`](helikodu_model::player::EngineEvent::NearEnd)), so files
//!   of the same format join without a gap.
//! - A file that cannot be opened or decoded is reported with
//!   [`PlayerEvent::Problem`] and skipped, in the direction playback was moving.
//! - [`PlayerCommand::Previous`] restarts the current file when more than three
//!   seconds of it have played, and goes to the file before otherwise.
//!   [`PlayerCommand::Next`] on the last file ends playback.
//! - When playback stops for good, [`PlayerEvent::Ended`] says why and how many
//!   files played and failed.
//! - With loudness normalization on ([`PlayerConfig::replay_gain`]), each file's
//!   ReplayGain or Opus R128 tags are read just before it is handed to the engine.

mod actor;
mod config;
mod error;
mod event;
mod loudness;
mod outputs;
mod queue;

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use helikodu_engine::transport::Engine;

pub use self::actor::PlayerCommand;
use self::actor::{Actor, Input};
pub use self::config::{PlayerConfig, ReplayGain, ReplayGainMode, SinkPacing};
pub use self::error::PlayerError;
pub use self::event::{EndReason, PlayerEvent};
pub use self::outputs::{list_outputs, next_output, resolve_backend, sound_backends};

/// A running player. Dropping it stops playback, closes the device and ends the
/// service thread.
#[derive(Debug)]
pub struct Player {
    inputs: Sender<Input>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    /// Starts a player with `config` and returns it with the receiver of its events.
    ///
    /// Fails if the configuration is invalid or a thread cannot be started. A backend
    /// or device that cannot be used is reported later, as a
    /// [`PlayerEvent::Problem`], when playback starts.
    pub fn spawn(config: PlayerConfig) -> Result<(Self, Receiver<PlayerEvent>), PlayerError> {
        config.validate()?;
        let selection = config.output.selection();
        let (engine, engine_events) =
            Engine::spawn(config.engine_config()).map_err(PlayerError::Spawn)?;
        let (inputs, mailbox) = mpsc::channel();
        let (events, event_receiver) = mpsc::channel();
        let forward = inputs.clone();
        thread::Builder::new()
            .name("helikodu-player-events".into())
            .spawn(move || {
                for event in engine_events {
                    if forward.send(Input::Engine(event)).is_err() {
                        break;
                    }
                }
            })
            .map_err(PlayerError::Spawn)?;
        let actor = Actor::new(
            engine,
            events,
            selection,
            config.volume,
            config.replay_gain.mode,
        );
        let thread = thread::Builder::new()
            .name("helikodu-player".into())
            .spawn(move || actor.run(&mailbox))
            .map_err(PlayerError::Spawn)?;
        let player = Self {
            inputs,
            thread: Some(thread),
        };
        Ok((player, event_receiver))
    }

    /// Sends a command. Fails only if the service thread has ended.
    pub fn send(&self, command: PlayerCommand) -> Result<(), PlayerError> {
        self.inputs
            .send(Input::Command(command))
            .map_err(|_| PlayerError::Gone)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.inputs.send(Input::Shutdown);
        if let Some(thread) = self.thread.take() {
            // A panic on the service thread has already been reported by the hook.
            let _ = thread.join();
        }
    }
}
