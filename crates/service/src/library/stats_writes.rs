// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Play counts and ratings written into the files by themselves, as the
//! `[advanced]` settings `write_play_counts_to_tags` and `write_ratings_to_tags`
//! ask: the library's lazy writer
//! ([`MusicLibrary::write_stats_to_files`]) run by the library service.
//!
//! - It writes a while after plays were recorded or ratings changed, once
//!   nothing new came for [`StatsWriteOptions::quiet`] (at the latest
//!   [`StatsWriteOptions::longest_wait`] after the first change), once after the
//!   start ([`StatsWriteOptions::start_delay`]) for what the last run left, and
//!   at once when a setting is switched on.
//! - It never writes while a scan or rescan runs: a write that is under way
//!   stops after its file, and goes on once the scan has ended.
//! - The file the player has open (playing or paused) is left for later, as
//!   writing replaces the file; it is written once another track plays or
//!   playback has ended.
//! - When the service ends, what is left is written, but for no longer than
//!   [`StatsWriteOptions::close_limit`]; the rest waits for the next start.
//!
//! Only files whose values changed since they were last written are written.
//! These writes are not on the undo history of tag edits. The thread runs at the
//! lowest CPU priority, and does nothing while the settings are off or nothing
//! changed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helikodu_library::CancelToken;
use helikodu_model::settings::{Settings, SettingsGroup};

use super::service::LibraryEvent;
use super::stats_import::stats_to_files;
use super::{MusicLibrary, WatchEvent};
use crate::Event;
use crate::bus::{EventBus, Subscription};
use crate::player::PlayerEvent;
use crate::settings::SettingsEvent;

/// The nice value of the writing thread: the lowest priority.
const NICE: i32 = 19;
/// The files written in one go before the playing file, a scan and the end of
/// the service are looked at again. Small, so that stopping is quick.
const BATCH: usize = 20;
/// A scan that has said nothing for this long counts as ended.
const SCAN_SILENCE: Duration = Duration::from_secs(60);

/// When play counts and ratings are written into the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsWriteOptions {
    /// How long after the start the first write waits, for what the last run
    /// left.
    pub start_delay: Duration,
    /// How long nothing new must be recorded before the changes are written.
    pub quiet: Duration,
    /// The longest a change waits while new ones keep coming.
    pub longest_wait: Duration,
    /// How long the end of the service may spend writing what is left.
    pub close_limit: Duration,
}

impl Default for StatsWriteOptions {
    fn default() -> Self {
        Self {
            start_delay: Duration::from_secs(60),
            quiet: Duration::from_secs(3 * 60),
            longest_wait: Duration::from_secs(20 * 60),
            close_limit: Duration::from_secs(2),
        }
    }
}

/// The writer of play counts and ratings of a library service.
pub(crate) struct StatsWriter {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    // Kept for as long as the writer lives; in a mutex, as a receiver is not `Sync`.
    _subscription: Mutex<Subscription<()>>,
}

impl std::fmt::Debug for StatsWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatsWriter").finish_non_exhaustive()
    }
}

struct Shared {
    library: MusicLibrary,
    bus: EventBus<Event>,
    worker_program: Option<PathBuf>,
    options: StatsWriteOptions,
    state: Mutex<State>,
    changed: Condvar,
    stopping: AtomicBool,
}

struct State {
    /// The settings; `None` until the service hands them over.
    settings: Option<Arc<Settings>>,
    /// When the next write is due; `None` when nothing waits.
    due: Option<Instant>,
    /// The first change not yet written, for the longest wait.
    first_change: Option<Instant>,
    /// The last sign of a running scan; `None` when none runs.
    scanning: Option<Instant>,
    /// The file the player has open.
    playing: Option<PathBuf>,
    /// Whether the open file was left out of a write.
    postponed: bool,
    /// Stops the write under way.
    running: Option<CancelToken>,
}

impl State {
    fn enabled(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(|s| stats_to_files(&s.advanced).any())
    }

    fn scanning(&self) -> bool {
        self.scanning
            .is_some_and(|since| since.elapsed() < SCAN_SILENCE)
    }

    fn interrupt(&self) {
        if let Some(token) = &self.running {
            token.cancel();
        }
    }
}

impl StatsWriter {
    /// Starts the thread, which waits until the settings ask for writes.
    pub(crate) fn start(
        library: MusicLibrary,
        bus: EventBus<Event>,
        worker_program: Option<PathBuf>,
        options: StatsWriteOptions,
    ) -> Self {
        let start_delay = options.start_delay;
        let shared = Arc::new(Shared {
            library,
            bus: bus.clone(),
            worker_program,
            options,
            state: Mutex::new(State {
                settings: None,
                due: Some(Instant::now() + start_delay),
                first_change: None,
                scanning: None,
                playing: None,
                postponed: false,
                running: None,
            }),
            changed: Condvar::new(),
            stopping: AtomicBool::new(false),
        });
        let weak: Weak<Shared> = Arc::downgrade(&shared);
        let subscription = bus.subscribe_with(
            move |event: &Event| {
                if let Some(shared) = weak.upgrade() {
                    shared.observe(event);
                }
                None
            },
            None,
        );
        let for_thread = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("helikodu-stats-writes".into())
            .spawn(move || for_thread.main())
            .inspect_err(|e| log::warn!("cannot start writing play counts to files: {e}"))
            .ok();
        Self {
            shared,
            thread: Mutex::new(thread),
            _subscription: Mutex::new(subscription),
        }
    }

    /// Follows `settings`.
    pub(crate) fn apply(&self, settings: &Settings) {
        self.shared.apply(Arc::new(settings.clone()));
    }

    /// Stops the thread, then writes what is left for at most the close limit.
    pub(crate) fn stop(&self) {
        let deadline = Instant::now() + self.shared.options.close_limit;
        self.shared.stopping.store(true, Ordering::SeqCst);
        let settings = {
            let state = self.shared.lock();
            state.interrupt();
            state.enabled().then(|| state.settings.clone()).flatten()
        };
        self.shared.changed.notify_all();
        if let Some(thread) = lock(&self.thread).take() {
            let _ = thread.join();
        }
        let Some(settings) = settings else {
            return;
        };
        if self.shared.library.is_read_only() {
            return;
        }
        // The player has stopped by now: no file is left out.
        let cancel = CancelToken::new();
        let (done, finished) = mpsc::channel::<()>();
        let timer = cancel.clone();
        let limit = deadline.saturating_duration_since(Instant::now());
        let watch = thread::Builder::new()
            .name("helikodu-stats-close".into())
            .spawn(move || {
                if let Err(RecvTimeoutError::Timeout) = finished.recv_timeout(limit) {
                    timer.cancel();
                }
            });
        if watch.is_err() {
            // Without a clock there is no bound: leave it for the next start.
            return;
        }
        self.shared.write(&settings, &cancel, Vec::new);
        drop(done);
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    fn apply(&self, settings: Arc<Settings>) {
        let mut state = self.lock();
        let was = state.enabled();
        let first = state.settings.is_none();
        state.settings = Some(settings);
        let on = state.enabled();
        if on && !was && !first {
            // Switched on: what is pending is written now.
            state.due = Some(Instant::now());
        } else if !on {
            state.interrupt();
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Follows the events of the bus. Runs on the publishing thread with the bus
    /// locked: it must not publish, and holds the state lock only briefly.
    fn observe(&self, event: &Event) {
        match event {
            Event::Player(PlayerEvent::TrackChanged { path, .. }) => {
                let mut state = self.lock();
                state.playing = Some(path.clone());
                self.after_playing(&mut state);
            }
            Event::Player(PlayerEvent::Ended { .. }) => {
                let mut state = self.lock();
                state.playing = None;
                self.after_playing(&mut state);
            }
            Event::Player(PlayerEvent::ListenRecorded { .. })
            | Event::Library(LibraryEvent::RatingChanged { .. }) => {
                let mut state = self.lock();
                self.changed_now(&mut state);
                drop(state);
                self.changed.notify_all();
            }
            Event::Settings(SettingsEvent::Changed { groups, settings })
                if groups.contains(&SettingsGroup::Advanced)
                    || groups.contains(&SettingsGroup::Library) =>
            {
                self.apply(Arc::clone(settings));
            }
            Event::Library(
                LibraryEvent::Scan(_) | LibraryEvent::Watch(WatchEvent::RescanStarted { .. }),
            ) => {
                let mut state = self.lock();
                state.scanning = Some(Instant::now());
                state.interrupt();
            }
            Event::Library(
                LibraryEvent::ScanEnded(_)
                | LibraryEvent::DuplicatesApplied(_)
                | LibraryEvent::Watch(WatchEvent::RescanFinished { .. }),
            ) => {
                self.lock().scanning = None;
                self.changed.notify_all();
            }
            _ => {}
        }
    }

    /// The file left out may be written now that it is not open any more.
    fn after_playing(&self, state: &mut State) {
        if state.postponed {
            state.postponed = false;
            self.changed_now(state);
            self.changed.notify_all();
        }
    }

    /// Something changed: a write is due once things are quiet.
    fn changed_now(&self, state: &mut State) {
        let now = Instant::now();
        let first = *state.first_change.get_or_insert(now);
        let at = (now + self.options.quiet).min(first + self.options.longest_wait);
        // A write due sooner (the start, a setting switched on) stays sooner.
        state.due = Some(state.due.map_or(at, |due| due.min(at)));
    }

    fn main(self: Arc<Self>) {
        let thread = rustix::thread::gettid();
        let _ = rustix::process::setpriority_process(Some(thread), NICE);
        if self.library.is_read_only() {
            return;
        }
        while let Some((settings, cancel)) = self.wait_for_work() {
            let stopped = self.write(&settings, &cancel, || {
                self.lock().playing.clone().into_iter().collect()
            });
            let mut state = self.lock();
            state.running = None;
            if stopped {
                // Stopped by a scan or the settings: go on later.
                state.due = Some(Instant::now());
            }
        }
    }

    /// Waits until a write is due, the settings ask for it and no scan runs;
    /// `None` when stopping.
    fn wait_for_work(&self) -> Option<(Arc<Settings>, CancelToken)> {
        let mut state = self.lock();
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                return None;
            }
            let wait = match state.due {
                Some(_) if !state.enabled() => None,
                Some(_) if state.scanning() => {
                    let since = state.scanning.unwrap_or_else(Instant::now);
                    Some(SCAN_SILENCE.saturating_sub(since.elapsed()))
                }
                Some(due) if due > Instant::now() => Some(due - Instant::now()),
                Some(_) => {
                    state.due = None;
                    state.first_change = None;
                    state.postponed = false;
                    let cancel = CancelToken::new();
                    state.running = Some(cancel.clone());
                    let settings = state.settings.clone()?;
                    return Some((settings, cancel));
                }
                None => None,
            };
            state = match wait {
                Some(wait) => {
                    self.changed
                        .wait_timeout(state, wait)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
            };
        }
    }

    /// Writes batches until nothing is left (but the files `skip` names), a
    /// write fails or `cancel` stops it; whether it was stopped.
    fn write(
        &self,
        settings: &Settings,
        cancel: &CancelToken,
        skip: impl Fn() -> Vec<PathBuf>,
    ) -> bool {
        loop {
            let skipped = skip();
            let writing = super::lock_tag_writes();
            let report = self.library.write_stats_to_files(
                &settings.library,
                &settings.advanced,
                self.worker_program.as_deref(),
                BATCH,
                &skipped,
                cancel,
            );
            drop(writing);
            let report = match report {
                Ok(report) => report,
                Err(error) => {
                    log::warn!("cannot write play counts and ratings to files: {error}");
                    return false;
                }
            };
            for (path, reason) in &report.failed {
                log::info!(
                    "cannot write the play count or rating to {}: {reason}",
                    path.display()
                );
            }
            if report.written > 0 {
                self.bus.publish(&Event::Library(LibraryEvent::Changed));
            }
            if report.postponed > 0 {
                self.lock().postponed = true;
            }
            if cancel.is_cancelled() {
                return true;
            }
            if report.written == 0 || report.remaining <= report.postponed {
                return false;
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
