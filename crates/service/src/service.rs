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
//! - `settings.toml` edited outside the program (the command line, a text editor) is
//!   read again while it runs, and applies as a change made in the window does.
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
use crate::library::backups::{BackupScheduler, StartNotice};
use crate::library::{
    BackgroundAnalysisOptions, DATABASE_FILE, LibraryError, LibraryService, LibraryServiceOptions,
    MusicLibrary, StatsWriteOptions,
};
use crate::lyrics::{LocalLyrics, LyricsService};
use crate::player::{
    CommandSender, Player, PlayerCommand, PlayerConfig, PlayerError, PlayerEvent, PlayerPrefs,
    ReplayGain, SessionConfig, SinkPacing,
};
use crate::settings::{
    SETTINGS_FILE, SettingsEvent, SettingsStore, SettingsWatcher, UI_STATE_FILE, UiStateStore,
};
use crate::waveforms::{PeakStore, WAVEFORM_FOLDER, WantedSources, WaveformService};

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
    /// How long after the start the default library's first daily backup is
    /// considered; `None` makes no backups by themselves.
    pub backups_after: Option<Duration>,
    /// How the background track analysis behaves (the `[library]
    /// analyze_in_background` setting turns it on).
    pub analysis: BackgroundAnalysisOptions,
    /// How long after an edit of `settings.toml` from outside the program it is
    /// read again; `None` does not watch the file.
    pub settings_debounce: Option<Duration>,
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
            backups_after: Some(Duration::from_secs(120)),
            analysis: BackgroundAnalysisOptions::default(),
            settings_debounce: Some(SettingsWatcher::DEBOUNCE),
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
    lyrics: Option<LyricsService>,
    waveforms: Option<WaveformService>,
    player: Player,
    library: Option<LibraryService>,
    backups: Option<BackupScheduler>,
    notices: Vec<StartNotice>,
    settings: Arc<SettingsStore>,
    // Reads edits of the settings file from outside while it is kept.
    _settings_watch: Option<SettingsWatcher>,
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
        crate::logging::configure(
            current.advanced.log_detail,
            current.library.storage.logs_mib,
        );
        let mut notices = Vec::new();
        let mut backups = None;
        let music = match config.library {
            LibraryChoice::Default => {
                // Without its library the player still plays files; the window
                // says why the library is missing.
                let (opened, found) = MusicLibrary::open_at_start(
                    &dirs.data.join(DATABASE_FILE),
                    &current.appearance.language,
                );
                notices = found;
                let music = opened.ok();
                if let (Some(music), Some(delay)) = (&music, config.backups_after)
                    && !music.is_read_only()
                {
                    backups = BackupScheduler::start(music.clone(), delay)
                        .inspect_err(|e| log::warn!("cannot start the backup schedule: {e}"))
                        .ok();
                }
                music
            }
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
                    language: current.appearance.language.clone(),
                    analysis: config.analysis,
                    stats_writes: StatsWriteOptions::default(),
                },
            )?),
            None => None,
        };
        // The library follows `[library.duplicates]`, now and when it changes.
        if let Some(library) = &library {
            library.apply_duplicate_settings(&current.library);
            library.set_import_playlists(&current.library);
            library.set_background_analysis(&current.library);
            library.set_stats_writes(&current);
        }
        let mut session = SessionConfig::new(&dirs.state);
        session.restore = current.playback.on_start == StartAction::Restore;
        session.debounce = config.session_debounce;
        let player_config = PlayerConfig {
            output: current.output.clone(),
            volume: Volume::FULL,
            pacing: config.pacing,
            replay_gain: ReplayGain::from_settings(&current.loudness),
            prefs: PlayerPrefs::from_settings(&current.playback),
            smart_play: current.playback.smart_play(),
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
        let wanted_library = player_config.library.clone();
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
        // Without the lyrics the rest works on.
        let lyrics = LyricsService::start(
            &bus,
            Arc::clone(&config.clock),
            Arc::new(LocalLyrics::default()),
            &player.status(),
            Some(&dirs.state),
        )
        .inspect_err(|e| log::warn!("cannot start the lyrics service: {e}"))
        .ok();
        // Without the waveforms the seek bar stays flat.
        let waveforms = WaveformService::start(
            Arc::new(PeakStore::new(
                dirs.cache.join(WAVEFORM_FOLDER),
                u64::from(current.library.storage.analysis_mib) << 20,
            )),
            &bus,
            helikodu_engine::decode::DecoderRegistry::with_settings(&current.playback.decoders),
            wanted_sources(player.queue_reader(), wanted_library),
        )
        .inspect_err(|e| log::warn!("cannot start the waveform service: {e}"))
        .ok();
        Ok(Self {
            glue: Some((stop, glue)),
            lyrics,
            waveforms,
            player,
            library,
            backups,
            notices,
            _settings_watch: config.settings_debounce.and_then(|debounce| {
                // Without it, edits from outside apply at the next start.
                SettingsWatcher::start(Arc::clone(&settings), debounce)
                    .inspect_err(|e| log::warn!("cannot watch the settings file: {e}"))
                    .ok()
            }),
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

    /// The lyrics of the playing track, unless their thread could not start.
    pub fn lyrics(&self) -> Option<&LyricsService> {
        self.lyrics.as_ref()
    }

    /// The waveform peaks of the seek bar, unless their thread could not start.
    pub fn waveforms(&self) -> Option<&WaveformService> {
        self.waveforms.as_ref()
    }

    /// The library, unless the service runs without one.
    pub fn library(&self) -> Option<&LibraryService> {
        self.library.as_ref()
    }

    /// What the window should tell about the start: a library opened
    /// read-only, a backup put back, a library that could not be opened.
    pub fn start_notices(&self) -> &[StartNotice] {
        &self.notices
    }

    /// Whether backups are made by themselves while the service runs.
    pub fn makes_backups(&self) -> bool {
        self.backups.is_some()
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

/// The sources the waveform service gives peaks: the current entry, the next few
/// and the few played last.
fn wanted_sources(
    queue: crate::player::QueueReader,
    library: Option<MusicLibrary>,
) -> WantedSources {
    use helikodu_model::library::TrackRef;
    const AHEAD: usize = 8;
    const BEHIND: usize = 2;
    Box::new(move || {
        let snapshot = queue.queue();
        let entries = snapshot
            .current_entry()
            .into_iter()
            .chain(snapshot.upcoming().iter().take(AHEAD))
            .chain(snapshot.history().iter().rev().take(BEHIND));
        entries
            .filter_map(|entry| match &entry.track {
                TrackRef::File(source) => Some(source.clone()),
                TrackRef::Library(id) => library
                    .as_ref()?
                    .track_source(*id)
                    .ok()
                    .flatten()
                    .map(|found| found.source),
            })
            .collect()
    })
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
                if groups.contains(&SettingsGroup::Advanced)
                    || groups.contains(&SettingsGroup::Library)
                {
                    crate::logging::configure(
                        now.advanced.log_detail,
                        now.library.storage.logs_mib,
                    );
                }
                if groups.contains(&SettingsGroup::Playback) {
                    let prefs = PlayerPrefs::from_settings(&now.playback);
                    let _ = player.send(PlayerCommand::Configure(prefs));
                    let smart = now.playback.smart_play();
                    let _ = player.send(PlayerCommand::ConfigureSmartPlay(smart));
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
