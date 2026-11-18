// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The whole service as the application starts it: settings, library, player and the
//! event bus, kept in step with each other.
//!
//! - The player starts with the output, loudness and playback settings, and restores
//!   the session when the settings ask for it.
//! - A changed `[playback]` table reconfigures the player at once; a changed output
//!   device, backend or mode moves playback there; the other `[output]` settings and
//!   the `[loudness]` and `[dsp]` tables reach the engine while it plays.
//! - A device chosen in the player (the quick switcher sends
//!   [`SetOutput`](crate::player::PlayerCommand::SetOutput)) is written back to the
//!   `[output]` settings, so the settings page and the next start agree.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use helikodu_model::audio::OutputSelection;
use helikodu_model::player::Volume;
use helikodu_model::settings::{SettingsGroup, StartAction};
use thiserror::Error;

use crate::SharedReservation;
use crate::bus::{EventBus, Subscription};
use crate::clock::{SharedClock, SystemClock};
use crate::event::{Event, SubscribeOptions, subscribe};
use crate::files::AppDirs;
use crate::library::{
    DATABASE_FILE, LibraryError, LibraryService, LibraryServiceOptions, MusicLibrary,
};
use crate::player::{
    CommandSender, Player, PlayerCommand, PlayerConfig, PlayerError, PlayerEvent, PlayerPrefs,
    ReplayGain, SessionConfig, SinkPacing,
};
use crate::settings::{SETTINGS_FILE, SettingsEvent, SettingsStore, UI_STATE_FILE, UiStateStore};

/// Which library the service opens.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum LibraryChoice {
    /// `library.db` in the data folder, created if needed.
    #[default]
    Default,
    /// One already open, such as [`MusicLibrary::in_memory`].
    Open(MusicLibrary),
    /// None: only files play and nothing is recorded.
    None,
}

/// How the service starts.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// The folders for settings, session, database and caches.
    pub dirs: AppDirs,
    /// The library.
    pub library: LibraryChoice,
    /// The time source.
    pub clock: SharedClock,
    /// How the test outputs take audio.
    pub pacing: SinkPacing,
    /// Whether cover thumbnails are made (in `<cache>/thumbnails`).
    pub thumbnails: bool,
    /// The scan worker program, when it is not beside the running program.
    pub worker_program: Option<PathBuf>,
    /// How long after a change the session is saved.
    pub session_debounce: Duration,
    /// The shuffle seed; from the time when `None`.
    pub shuffle_seed: Option<u64>,
    /// How exclusive output takes a sound card from the sound server (see
    /// [`PlayerConfig::reservation`]); the default takes nothing.
    pub reservation: SharedReservation,
}

impl ServiceConfig {
    /// The defaults for the folders `dirs`.
    pub fn new(dirs: AppDirs) -> Self {
        Self {
            dirs,
            library: LibraryChoice::Default,
            clock: SystemClock::shared(),
            pacing: SinkPacing::RealTime,
            thumbnails: true,
            worker_program: None,
            session_debounce: Duration::from_secs(1),
            shuffle_seed: None,
            reservation: SharedReservation::default(),
        }
    }
}

/// Why the service could not start.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServiceError {
    /// The library could not be opened.
    #[error(transparent)]
    Library(#[from] LibraryError),
    /// The player could not start.
    #[error(transparent)]
    Player(#[from] PlayerError),
    /// A thread could not be started.
    #[error("cannot start the service: {0}")]
    Spawn(#[source] std::io::Error),
}

/// The running service. Dropping it saves the session and stops everything.
#[derive(Debug)]
pub struct Service {
    // Dropped in this order: the glue first, then the player (which saves the
    // session), then the library.
    glue: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
    player: Player,
    library: Option<LibraryService>,
    settings: Arc<SettingsStore>,
    ui_state: UiStateStore,
    bus: EventBus<Event>,
    clock: SharedClock,
    dirs: AppDirs,
}

impl Service {
    /// Starts the service. Problems with the settings file do not stop it (see
    /// [`SettingsStore::load_notice`]).
    pub fn start(config: ServiceConfig) -> Result<Self, ServiceError> {
        let bus = EventBus::new();
        let dirs = config.dirs;
        let settings = Arc::new(SettingsStore::open(
            dirs.config.join(SETTINGS_FILE),
            bus.clone(),
        ));
        let current = settings.get();
        let music = match config.library {
            LibraryChoice::Default => Some(MusicLibrary::open(&dirs.data.join(DATABASE_FILE))?),
            LibraryChoice::Open(library) => Some(library),
            LibraryChoice::None => None,
        };
        let library = match &music {
            Some(music) => Some(LibraryService::start(
                music.clone(),
                bus.clone(),
                LibraryServiceOptions {
                    thumbnails: config.thumbnails.then(|| dirs.cache.join("thumbnails")),
                    thumbnail_bytes: u64::from(current.library.storage.covers_mib) << 20,
                    worker_program: config.worker_program,
                },
            )?),
            None => None,
        };
        let mut session = SessionConfig::new(&dirs.state);
        session.restore = current.playback.on_start == StartAction::Restore;
        session.debounce = config.session_debounce;
        let player_config = PlayerConfig {
            output: current.output.clone(),
            volume: Volume::FULL,
            pacing: config.pacing,
            replay_gain: ReplayGain::from_settings(&current.loudness),
            prefs: PlayerPrefs::from_settings(&current.playback),
            library: music,
            session: Some(session),
            clock: Arc::clone(&config.clock),
            bus: Some(bus.clone()),
            shuffle_seed: config.shuffle_seed,
            reservation: config.reservation,
            dsp: current.dsp.clone(),
            transitions: current.playback.transitions,
            decoders: current.playback.decoders.clone(),
        };
        let (player, _) = Player::spawn(player_config)?;
        // A restored session may have played on another device than the settings
        // name: the settings follow it.
        player.sync()?;
        sync_output(&settings, &player.status().output);

        let stop = Arc::new(AtomicBool::new(false));
        let events = subscribe(
            &bus,
            Arc::clone(&config.clock),
            SubscribeOptions::all(),
            |e| match e {
                Event::Settings(_) | Event::Player(PlayerEvent::OutputChanged(_)) => {
                    Some(e.clone())
                }
                _ => None,
            },
        );
        let glue = {
            let stop = Arc::clone(&stop);
            let settings = Arc::clone(&settings);
            let sender = player.sender();
            let output = player.status().output;
            thread::Builder::new()
                .name("helikodu-service".into())
                .spawn(move || keep_in_step(&events, &stop, &settings, &sender, output))
                .map_err(ServiceError::Spawn)?
        };
        Ok(Self {
            glue: Some((stop, glue)),
            player,
            library,
            settings,
            ui_state: UiStateStore::new(dirs.state.join(UI_STATE_FILE)),
            bus,
            clock: config.clock,
            dirs,
        })
    }

    /// Subscribes to the service's events.
    pub fn subscribe(&self, options: SubscribeOptions) -> Subscription<Event> {
        subscribe(&self.bus, Arc::clone(&self.clock), options, |e| {
            Some(e.clone())
        })
    }

    /// The player.
    pub fn player(&self) -> &Player {
        &self.player
    }

    /// The library, unless the service runs without one.
    pub fn library(&self) -> Option<&LibraryService> {
        self.library.as_ref()
    }

    /// The settings.
    pub fn settings(&self) -> &SettingsStore {
        &self.settings
    }

    /// The window state.
    pub fn ui_state(&self) -> &UiStateStore {
        &self.ui_state
    }

    /// The folders in use.
    pub fn dirs(&self) -> &AppDirs {
        &self.dirs
    }

    /// The bus.
    pub fn bus(&self) -> &EventBus<Event> {
        &self.bus
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        if let Some((stop, thread)) = self.glue.take() {
            stop.store(true, Ordering::SeqCst);
            let _ = thread.join();
        }
    }
}

/// Writes `output` into the `[output]` settings if they name another device.
fn sync_output(settings: &SettingsStore, output: &OutputSelection) {
    if settings.get().output.selection() != *output {
        // A newer settings file refuses changes; the device then stays as it is there.
        let _ = settings.update(|s| {
            s.output.backend = output.backend;
            s.output.device = output.device.clone();
            s.output.mode = output.mode;
        });
    }
}

fn keep_in_step(
    events: &Subscription<Event>,
    stop: &AtomicBool,
    settings: &SettingsStore,
    player: &CommandSender,
    mut output: OutputSelection,
) {
    while !stop.load(Ordering::SeqCst) {
        let Ok(event) = events.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        match event {
            Event::Settings(SettingsEvent::Changed {
                groups,
                settings: now,
            }) => {
                if groups.contains(&SettingsGroup::Playback) {
                    let prefs = PlayerPrefs::from_settings(&now.playback);
                    let _ = player.send(PlayerCommand::Configure(prefs));
                    let transitions = now.playback.transitions;
                    let _ = player.send(PlayerCommand::ConfigureTransitions(transitions));
                    let decoders = now.playback.decoders.clone();
                    let _ = player.send(PlayerCommand::ConfigureDecoders(decoders));
                }
                if groups.contains(&SettingsGroup::Output) && now.output.selection() != output {
                    output = now.output.selection();
                    let _ = player.send(PlayerCommand::SetOutput(output.clone()));
                }
                if groups.contains(&SettingsGroup::Output)
                    || groups.contains(&SettingsGroup::Loudness)
                    || groups.contains(&SettingsGroup::Dsp)
                {
                    // The engine compares: only a change of the stream reopens it.
                    let _ = player.send(PlayerCommand::ConfigureAudio {
                        output: now.output.clone(),
                        loudness: now.loudness,
                        dsp: Box::new(now.dsp.clone()),
                    });
                }
            }
            Event::Player(PlayerEvent::OutputChanged(selection)) => {
                output = selection;
                sync_output(settings, &output);
            }
            _ => {}
        }
    }
}
