// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track analysis in the background: the job of [`crate::jobs::analysis`] run
//! by the library service whenever there may be tracks to analyse, as the
//! `[library] analyze_in_background` setting allows.
//!
//! - It runs after the start (after [`BackgroundAnalysisOptions::start_delay`]) and
//!   after every scan, rescan or change of the library (after
//!   [`BackgroundAnalysisOptions::settle`]), one track at a time on one thread.
//! - The thread runs at the lowest CPU priority (nice 19). Linux derives the I/O
//!   priority of a thread without one of its own from its nice value, so its reads
//!   also get the lowest best-effort I/O priority. (The idle I/O class would need
//!   a system call that no safe binding offers.)
//! - Playback is protected beyond that: while the player plays or opens a file on
//!   the same device (the same `st_dev`) as the track being analysed, the job
//!   yields: it works at most [`BackgroundAnalysisOptions::share_percent`] of the
//!   time, in bursts of 50 ms, and sleeps the rest, so the disk and the decoder
//!   threads of playback are rarely kept waiting. Files on another device are
//!   analysed at full (low-priority) speed.
//! - It pauses while a scan or rescan runs (they read the same disks), and on
//!   battery power when [`BackgroundAnalysisOptions::pause_on_battery`].
//! - Turning the setting off, or dropping the service, stops it within a decoded
//!   block; what it finished stays stored, and it resumes with the tracks still
//!   pending.
//!
//! Its progress is announced as [`LibraryEvent::Analysis`] (when its state changes,
//! and at most every two seconds while it works) and can be asked for with
//! [`LibraryService::analysis_progress`](super::LibraryService::analysis_progress).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helikodu_model::player::PlaybackState;
use helikodu_model::settings::{LibrarySettings, SettingsGroup};

use super::service::LibraryEvent;
use super::{MusicLibrary, WatchEvent};
use crate::Event;
use crate::bus::{EventBus, Subscription};
use crate::jobs::analysis::{AnalysisEvent, AnalysisGate, AnalysisOptions, coverage, run};
use crate::player::PlayerEvent;
use crate::settings::SettingsEvent;

/// The nice value of the analysis thread: the lowest priority.
const NICE: i32 = 19;
/// The length of a burst of work while yielding to playback.
const BURST: Duration = Duration::from_millis(50);
/// How often progress is announced while the job works.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(2);
/// A scan that has said nothing for this long counts as ended.
const SCAN_SILENCE: Duration = Duration::from_secs(60);
/// How often the power supply is looked at while paused on battery.
const BATTERY_RECHECK: Duration = Duration::from_secs(60);

/// How the background analysis behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundAnalysisOptions {
    /// How long after the start the first run waits, so that the program and its
    /// first scan settle.
    pub start_delay: Duration,
    /// How long after a scan or a change of the library a run waits.
    pub settle: Duration,
    /// Whether it pauses while the computer runs on battery.
    pub pause_on_battery: bool,
    /// The share of the time it works, in percent (1 to 100), while playback reads
    /// from the same device.
    pub share_percent: u32,
    /// What the job does besides the analysis.
    pub job: AnalysisOptions,
}

impl Default for BackgroundAnalysisOptions {
    fn default() -> Self {
        Self {
            start_delay: Duration::from_secs(30),
            settle: Duration::from_secs(5),
            pause_on_battery: true,
            share_percent: 25,
            job: AnalysisOptions::default(),
        }
    }
}

/// Where the background analysis stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum AnalysisState {
    /// Turned off by the setting (or the library is read-only).
    #[default]
    Off,
    /// Waiting for work (or to start).
    Idle,
    /// Analysing.
    Running,
    /// Analysing slowly, because playback reads from the same device.
    Yielding,
    /// Paused, for the reason given.
    Paused(PauseReason),
}

/// Why the background analysis is paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PauseReason {
    /// A scan or rescan runs.
    Scanning,
    /// The computer runs on battery.
    OnBattery,
}

/// How far the analysis of the library is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnalysisProgress {
    /// The tracks whose files are present.
    pub total: usize,
    /// Those with a current analysis.
    pub analyzed: usize,
    /// Those that could not be analysed (not tried again until their files
    /// change).
    pub failed: usize,
    /// What the background analysis does.
    pub state: AnalysisState,
}

impl AnalysisProgress {
    /// The tracks still to analyse.
    pub fn remaining(&self) -> usize {
        self.total.saturating_sub(self.analyzed + self.failed)
    }
}

/// The background analysis of a library service.
pub(crate) struct BackgroundAnalysis {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    // Kept for as long as the analysis lives; in a mutex, as a receiver is not `Sync`.
    _subscription: Mutex<Subscription<()>>,
}

impl std::fmt::Debug for BackgroundAnalysis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundAnalysis").finish_non_exhaustive()
    }
}

struct Shared {
    library: MusicLibrary,
    bus: EventBus<Event>,
    options: BackgroundAnalysisOptions,
    state: Mutex<State>,
    changed: Condvar,
    enabled: AtomicBool,
    stopping: AtomicBool,
    /// Whether the player plays or opens a file.
    playing: AtomicBool,
    /// Whether the track being analysed is on the device playback reads from.
    same_device: AtomicBool,
}

struct State {
    /// Whether there may be work: set by start, scans and changes.
    work: bool,
    /// The earliest start of the next run.
    wake_at: Option<Instant>,
    /// The last sign of a running scan; `None` when none runs.
    scanning: Option<Instant>,
    /// The file the player plays.
    playing_path: Option<PathBuf>,
    progress: AnalysisProgress,
    published: Option<(AnalysisProgress, Instant)>,
    burst: Option<Instant>,
    battery: Option<(bool, Instant)>,
}

impl BackgroundAnalysis {
    /// Starts the thread, which waits until the setting turns the analysis on.
    pub(crate) fn start(
        library: MusicLibrary,
        bus: EventBus<Event>,
        options: BackgroundAnalysisOptions,
    ) -> Self {
        let start_delay = options.start_delay;
        let shared = Arc::new(Shared {
            library,
            bus: bus.clone(),
            options,
            state: Mutex::new(State {
                work: true,
                wake_at: Some(Instant::now() + start_delay),
                scanning: None,
                playing_path: None,
                progress: AnalysisProgress::default(),
                published: None,
                burst: None,
                battery: None,
            }),
            changed: Condvar::new(),
            enabled: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            same_device: AtomicBool::new(false),
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
            .name("helikodu-analysis".into())
            .spawn(move || for_thread.main())
            .inspect_err(|e| log::warn!("cannot start the track analysis: {e}"))
            .ok();
        Self {
            shared,
            thread: Mutex::new(thread),
            _subscription: Mutex::new(subscription),
        }
    }

    /// Turns the analysis on or off as `settings` say.
    pub(crate) fn apply(&self, settings: &LibrarySettings) {
        self.shared.set_enabled(settings.analyze_in_background);
    }

    /// Runs as soon as possible, without waiting for the start delay.
    pub(crate) fn wake(&self) {
        let mut state = self.shared.lock();
        state.work = true;
        state.wake_at = None;
        drop(state);
        self.shared.changed.notify_all();
    }

    /// Where the analysis stands.
    pub(crate) fn progress(&self) -> AnalysisProgress {
        self.shared.lock().progress
    }

    /// Stops the thread and waits for it.
    pub(crate) fn stop(&self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        drop(self.shared.lock());
        self.shared.changed.notify_all();
        let thread = lock(&self.thread).take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    fn set_enabled(&self, on: bool) {
        let was = self.enabled.swap(on, Ordering::SeqCst);
        if was != on {
            drop(self.lock());
            self.changed.notify_all();
        }
    }

    /// Follows the events of the bus. Runs on the publishing thread with the bus
    /// locked: it must not publish, and holds the state lock only briefly.
    fn observe(&self, event: &Event) {
        match event {
            Event::Player(PlayerEvent::State { playback, .. }) => {
                let active = matches!(playback, PlaybackState::Playing | PlaybackState::Buffering);
                self.playing.store(active, Ordering::SeqCst);
            }
            Event::Player(PlayerEvent::TrackChanged { path, .. }) => {
                self.lock().playing_path = Some(path.clone());
            }
            Event::Settings(SettingsEvent::Changed { groups, settings })
                if groups.contains(&SettingsGroup::Library) =>
            {
                self.set_enabled(settings.library.analyze_in_background);
            }
            Event::Library(
                LibraryEvent::Scan(_) | LibraryEvent::Watch(WatchEvent::RescanStarted { .. }),
            ) => {
                self.lock().scanning = Some(Instant::now());
            }
            Event::Library(
                LibraryEvent::ScanEnded(_)
                | LibraryEvent::DuplicatesApplied(_)
                | LibraryEvent::Watch(WatchEvent::RescanFinished { .. }),
            ) => {
                let mut state = self.lock();
                state.scanning = None;
                self.more_work(&mut state);
                drop(state);
                self.changed.notify_all();
            }
            Event::Library(LibraryEvent::Changed) => {
                let mut state = self.lock();
                self.more_work(&mut state);
                drop(state);
                self.changed.notify_all();
            }
            _ => {}
        }
    }

    fn more_work(&self, state: &mut State) {
        state.work = true;
        let settled = Instant::now() + self.options.settle;
        state.wake_at = Some(state.wake_at.map_or(settled, |at| at.max(settled)));
    }

    fn main(self: Arc<Self>) {
        let thread = rustix::thread::gettid();
        let _ = rustix::process::setpriority_process(Some(thread), NICE);
        if self.library.is_read_only() {
            return;
        }
        match coverage(&self.library) {
            Ok(found) => {
                let mut state = self.lock();
                state.progress.total = found.tracks;
                state.progress.analyzed = found.analyzed;
                state.progress.failed = found.failed;
            }
            Err(error) => log::warn!("cannot count the analysed tracks: {error}"),
        }
        loop {
            if !self.wait_for_work() {
                return;
            }
            let summary = run(&self.library, &self.options.job, &*self, |event| {
                self.job_event(&event)
            });
            if let Some(error) = &summary.error {
                log::warn!("the track analysis stopped: {error}");
            }
            self.same_device.store(false, Ordering::SeqCst);
        }
    }

    /// Waits until there is work and the analysis is on; `false` when stopping.
    fn wait_for_work(&self) -> bool {
        let mut state = self.lock();
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                return false;
            }
            let on = self.enabled.load(Ordering::SeqCst);
            let shown = if on {
                AnalysisState::Idle
            } else {
                AnalysisState::Off
            };
            state = self.show(state, shown);
            if on && state.work {
                match state.wake_at {
                    Some(at) if at > Instant::now() => {
                        let wait = at - Instant::now();
                        state = self
                            .changed
                            .wait_timeout(state, wait)
                            .unwrap_or_else(PoisonError::into_inner)
                            .0;
                        continue;
                    }
                    _ => {
                        state.work = false;
                        state.wake_at = None;
                        return true;
                    }
                }
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Sets the state shown, announcing it if it changed; the lock is let go
    /// while announcing.
    fn show<'a>(
        &'a self,
        mut state: MutexGuard<'a, State>,
        shown: AnalysisState,
    ) -> MutexGuard<'a, State> {
        state.progress.state = shown;
        let progress = state.progress;
        let due = match state.published {
            None => true,
            Some((last, at)) => {
                last.state != progress.state
                    || (last != progress && at.elapsed() >= PROGRESS_INTERVAL)
            }
        };
        if !due {
            return state;
        }
        state.published = Some((progress, Instant::now()));
        drop(state);
        self.bus
            .publish(&Event::Library(LibraryEvent::Analysis(progress)));
        self.lock()
    }

    fn job_event(&self, event: &AnalysisEvent) {
        let mut state = self.lock();
        match event {
            AnalysisEvent::Started(found) => {
                state.progress.total = found.tracks;
                state.progress.analyzed = found.analyzed;
                state.progress.failed = found.failed;
            }
            AnalysisEvent::Track {
                pending: true,
                result,
                ..
            } => match result {
                Ok(()) => state.progress.analyzed += 1,
                Err(_) => state.progress.failed += 1,
            },
            AnalysisEvent::Track { .. } => {}
            AnalysisEvent::Finished(_) => {
                // Settle on the stored counts: tracks may have come or gone.
                drop(state);
                let found = coverage(&self.library);
                state = self.lock();
                if let Ok(found) = found {
                    state.progress.total = found.tracks;
                    state.progress.analyzed = found.analyzed;
                    state.progress.failed = found.failed;
                }
            }
        }
        let shown = state.progress.state;
        drop(self.show(state, shown));
    }

    /// Whether the computer runs on battery, looked at once a minute at most.
    fn on_battery(&self, state: &mut State) -> bool {
        if !self.options.pause_on_battery {
            return false;
        }
        match state.battery {
            Some((on, at)) if at.elapsed() < BATTERY_RECHECK => on,
            _ => {
                let on = discharging(Path::new("/sys/class/power_supply"));
                state.battery = Some((on, Instant::now()));
                on
            }
        }
    }
}

impl AnalysisGate for Shared {
    fn before_track(&self, path: &Path) -> bool {
        let mut state = self.lock();
        loop {
            if self.stopping.load(Ordering::SeqCst) || !self.enabled.load(Ordering::SeqCst) {
                return false;
            }
            let pause = if state
                .scanning
                .is_some_and(|since| since.elapsed() < SCAN_SILENCE)
            {
                Some((PauseReason::Scanning, Duration::from_secs(1)))
            } else if self.on_battery(&mut state) {
                Some((PauseReason::OnBattery, BATTERY_RECHECK))
            } else {
                None
            };
            let Some((reason, wait)) = pause else {
                break;
            };
            state = self.show(state, AnalysisState::Paused(reason));
            state = self
                .changed
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        let playing_path = state.playing_path.clone();
        drop(state);
        let same = playing_path.is_some_and(|playing| same_device(&playing, path));
        self.same_device.store(same, Ordering::SeqCst);
        let yielding = same && self.playing.load(Ordering::SeqCst);
        let state = self.lock();
        drop(self.show(
            state,
            if yielding {
                AnalysisState::Yielding
            } else {
                AnalysisState::Running
            },
        ));
        true
    }

    fn between_blocks(&self) -> bool {
        if self.stopping.load(Ordering::SeqCst) || !self.enabled.load(Ordering::SeqCst) {
            return false;
        }
        let yielding =
            self.same_device.load(Ordering::SeqCst) && self.playing.load(Ordering::SeqCst);
        let mut state = self.lock();
        if !yielding {
            state.burst = None;
            return true;
        }
        let Some(started) = state.burst else {
            state.burst = Some(Instant::now());
            return true;
        };
        let worked = started.elapsed();
        if worked < BURST {
            return true;
        }
        let share = self.options.share_percent.clamp(1, 100);
        let rest = worked * (100 - share) / share;
        state = self
            .changed
            .wait_timeout(state, rest)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
        state.burst = Some(Instant::now());
        !self.stopping.load(Ordering::SeqCst) && self.enabled.load(Ordering::SeqCst)
    }
}

/// Whether two files are on the same device.
fn same_device(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev(),
        _ => false,
    }
}

/// Whether a battery in the power supply class folder `class` is discharging.
fn discharging(class: &Path) -> bool {
    let Ok(entries) = fs::read_dir(class) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let read = |name: &str| {
            fs::read_to_string(entry.path().join(name))
                .map(|text| text.trim().to_owned())
                .unwrap_or_default()
        };
        read("type") == "Battery" && read("status") == "Discharging"
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_discharging_battery_is_found() {
        let dir = std::env::temp_dir().join(format!("helikodu-power-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let supply = |name: &str, kind: &str, status: &str| {
            let path = dir.join(name);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("type"), format!("{kind}\n")).unwrap();
            fs::write(path.join("status"), format!("{status}\n")).unwrap();
        };
        assert!(!discharging(&dir));
        supply("AC", "Mains", "");
        supply("BAT0", "Battery", "Charging");
        assert!(!discharging(&dir));
        supply("BAT0", "Battery", "Discharging");
        assert!(discharging(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_in_one_folder_share_a_device() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(same_device(&here.join("Cargo.toml"), &here.join("src")));
        assert!(!same_device(&here.join("missing"), &here.join("src")));
    }
}
