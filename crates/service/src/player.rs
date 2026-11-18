// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The player service: the play queue, played by the engine.
//!
//! [`Player::spawn`] starts the service actor, a thread that owns the engine, the
//! queue and the session. Front ends send [`PlayerCommand`]s with [`Player::send`]
//! and follow [`PlayerEvent`]s through subscriptions; every change of state goes
//! through the actor's mailbox. [`Player::queue`] and [`Player::status`] read the
//! latest state without waiting for the actor.
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
//! - The queue holds entries in play order, with the context they came from. Each
//!   next entry is handed to the engine when it asks for one
//!   ([`NearEnd`](helikodu_model::player::EngineEvent::NearEnd)), so tracks of the
//!   same format join without a gap; when the queue, the modes or stop-after change
//!   after that, the engine is told the new next entry.
//! - Shuffle stores its order, so "Previous" walks back the order played; repeat is
//!   off, one or all; stop-after ends playback when the current track ends.
//! - An entry that cannot be opened or decoded is reported with
//!   [`PlayerEvent::Problem`] and skipped, in the direction playback was moving.
//! - "Previous" restarts the current entry after the time set in
//!   [`PlayerPrefs::previous_restart`] (three seconds by default) and goes to the
//!   entry before otherwise.
//! - With a library, every listen to a library track (also a file that is in the
//!   library) is recorded with the play and skip rules of
//!   [`PlayerPrefs::play_rules`], on a thread of its own.
//! - With a session folder, the queue, position, modes, volume and output are saved
//!   after changes and at the end, and restored paused at the start; the crash
//!   sentinel keeps a track that ended the program twice in a row from playing
//!   automatically (see [`session`]).

mod actor;
mod command;
mod config;
mod error;
mod event;
mod listen;
mod loudness;
mod outputs;
mod queue;
mod rng;
pub mod session;
mod shared;
mod timers;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use helikodu_engine::decode::DecoderRegistry;
use helikodu_engine::transport::Engine;
use helikodu_model::audio::MediaTime;
use helikodu_model::player::StopAfter;

use self::actor::{Actor, Input, Startup};
use self::listen::Recorder;
use self::queue::Queue;
use self::rng::Rng;
use self::session::SessionFiles;
use self::shared::Shared;
use crate::Event;
use crate::bus::{EventBus, Subscription};
use crate::clock::SharedClock;
use crate::event::{SubscribeOptions, subscribe};

pub use self::command::{ContextRequest, PlayerCommand};
pub use self::config::{
    PlayerConfig, PlayerPrefs, ReplayGain, ReplayGainMode, SessionConfig, SinkPacing,
};
pub use self::error::PlayerError;
pub use self::event::{EndReason, Notice, PlayerEvent};
pub use self::outputs::{list_outputs, next_output, resolve_backend, sound_backends};
pub use self::shared::{NowPlaying, PlayerStatus, QueueSnapshot, Splice};
pub use self::timers::{AbLoop, SLEEP_FADE, SleepTimerStatus, SleepWhen};

/// A running player. Dropping it ends the listen in progress, saves the session,
/// removes the crash sentinel, stops playback and ends the service thread.
#[derive(Debug)]
pub struct Player {
    inputs: Sender<Input>,
    thread: Option<JoinHandle<()>>,
    bus: EventBus<Event>,
    clock: SharedClock,
    shared: Arc<Shared>,
}

impl Player {
    /// Starts a player with `config` and returns it with a subscription to all its
    /// events (every position included).
    ///
    /// Fails if the configuration is invalid or a thread cannot be started. A backend
    /// or device that cannot be used is reported later, as a
    /// [`PlayerEvent::Problem`], when playback starts.
    pub fn spawn(config: PlayerConfig) -> Result<(Self, Subscription<PlayerEvent>), PlayerError> {
        config.validate()?;
        let bus = config.bus.clone().unwrap_or_default();
        let rng = config.shuffle_seed.map_or_else(Rng::from_entropy, Rng::new);
        let mut startup = Startup {
            queue: Queue::new(rng.clone()),
            position: None,
            volume: config.volume,
            selection: config.output.selection(),
            suspects: HashSet::new(),
            strikes: HashMap::new(),
            stop_after: StopAfter::Off,
            crash: None,
            session: None,
            prefs: config.prefs,
            library: config.library.clone(),
        };
        if let Some(session_config) = &config.session {
            let mut files = SessionFiles::new(&session_config.dir);
            let report = files.take_crash_report();
            let saved = files.load();
            if let Some(saved) = saved {
                startup.suspects.extend(saved.suspects.iter().cloned());
                if session_config.restore {
                    startup.queue = Queue::restore(
                        rng,
                        saved.context,
                        saved.entries,
                        saved.linear,
                        saved.current,
                        saved.modes,
                    );
                    startup.position = Some(MediaTime::from_millis(saved.position_ms));
                    startup.volume = saved.volume;
                    startup.stop_after = saved.stop_after;
                    startup.selection = saved.output;
                }
            }
            startup.suspects.extend(report.suspects().cloned());
            if report.crashed {
                let mut suspects: Vec<_> = report.suspects().cloned().collect();
                suspects.sort_by_key(|t| format!("{t:?}"));
                let mut tracks: Vec<_> = report.strikes.keys().cloned().collect();
                tracks.sort_by_key(|t| format!("{t:?}"));
                startup.crash = Some(Notice::RecoveredAfterCrash { tracks, suspects });
            }
            startup.strikes = report.strikes;
            startup.session = Some((files, session_config.clone()));
        }

        let mut engine_config = config.engine_config(&startup.selection);
        engine_config.volume = startup.volume;
        let registry = DecoderRegistry::with_settings(&config.decoders);
        let (engine, engine_events) =
            Engine::spawn_with_registry(engine_config, registry).map_err(PlayerError::Spawn)?;
        let (inputs, mailbox) = mpsc::channel();
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
        let recorder = match &config.library {
            Some(library) => {
                let results = inputs.clone();
                Some(
                    Recorder::start(library.clone(), move |recorded| {
                        let _ = results.send(Input::Recorded(recorded));
                    })
                    .map_err(PlayerError::Spawn)?,
                )
            }
            None => None,
        };
        let shared = Arc::new(Shared::default());
        let events = subscribe(
            &bus,
            Arc::clone(&config.clock),
            SubscribeOptions::player(),
            player_event,
        );
        let actor = Actor::new(
            engine,
            bus.clone(),
            Arc::clone(&config.clock),
            Arc::clone(&shared),
            startup,
            recorder,
        );
        let thread = thread::Builder::new()
            .name("helikodu-player".into())
            .spawn(move || actor.run(&mailbox))
            .map_err(PlayerError::Spawn)?;
        let player = Self {
            inputs,
            thread: Some(thread),
            bus,
            clock: config.clock,
            shared,
        };
        Ok((player, events))
    }

    /// Sends a command. Fails only if the service thread has ended.
    pub fn send(&self, command: PlayerCommand) -> Result<(), PlayerError> {
        self.inputs
            .send(Input::Command(command))
            .map_err(|_| PlayerError::Gone)
    }

    /// A handle that sends commands to this player from another thread, such as an
    /// MPRIS or D-Bus endpoint. It fails once the player is dropped.
    pub fn sender(&self) -> CommandSender {
        CommandSender(self.inputs.clone())
    }

    /// Waits until every command sent before has been handled and the session saved.
    pub fn sync(&self) -> Result<(), PlayerError> {
        let (reply, done) = mpsc::channel();
        self.inputs
            .send(Input::Sync(reply))
            .map_err(|_| PlayerError::Gone)?;
        done.recv().map_err(|_| PlayerError::Gone)
    }

    /// Subscribes to the player's events with `options` (its library and settings
    /// flags are ignored).
    pub fn subscribe(&self, options: SubscribeOptions) -> Subscription<PlayerEvent> {
        subscribe(&self.bus, Arc::clone(&self.clock), options, player_event)
    }

    /// The bus the player publishes on.
    pub fn bus(&self) -> &EventBus<Event> {
        &self.bus
    }

    /// The queue as of the last change. Never waits for the player's thread.
    pub fn queue(&self) -> Arc<QueueSnapshot> {
        self.shared.queue()
    }

    /// The state of playback as of the last change. Never waits for the player's
    /// thread.
    pub fn status(&self) -> PlayerStatus {
        self.shared.status()
    }
}

/// Sends commands to a player; see [`Player::sender`].
#[derive(Debug, Clone)]
pub struct CommandSender(Sender<Input>);

impl CommandSender {
    /// Sends a command. Fails only if the player has ended.
    pub fn send(&self, command: PlayerCommand) -> Result<(), PlayerError> {
        self.0
            .send(Input::Command(command))
            .map_err(|_| PlayerError::Gone)
    }
}

fn player_event(event: &Event) -> Option<PlayerEvent> {
    match event {
        Event::Player(event) => Some(event.clone()),
        _ => None,
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
