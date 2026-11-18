// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The player's state on the main thread: built from the backend's updates,
//! read by the deck, the queue panel and the views.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;

use crate::backend::{
    AlbumKey, Backend, LoopPoints, NowPlaying, OutputInfo, PlayModes, PlaybackState, PlaybackStats,
    QueueSnapshot, Repeat, RepeatMode, ShuffleMode, SignalPath, SleepTimer, StopAfter, Update,
};

/// What the player does now.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PlayerView {
    pub(crate) state: PlaybackState,
    pub(crate) now: Option<NowPlaying>,
    pub(crate) elapsed: Duration,
    pub(crate) length: Option<Duration>,
    pub(crate) volume: f64,
    pub(crate) output: OutputInfo,
    pub(crate) queue: QueueSnapshot,
    pub(crate) shuffle: bool,
    pub(crate) repeat: Repeat,
    pub(crate) stop_after: bool,
    /// The signal path of what is heard, as the engine last reported it.
    pub(crate) signal: Option<Rc<SignalPath>>,
    /// The output's counters, as last reported.
    pub(crate) stats: Option<PlaybackStats>,
    /// Every play mode.
    pub(crate) modes: PlayModes,
    /// Stop after the track, the album or the queue.
    pub(crate) stop_after_mode: StopAfter,
    /// The A-B loop of the playing track.
    pub(crate) ab_loop: Option<LoopPoints>,
    /// The sleep timer while one runs, and when it was reported.
    pub(crate) sleep: Option<(SleepTimer, Instant)>,
    /// The queue entry selected for "Continue from the selected track".
    pub(crate) cursor: Option<u64>,
    /// The waveform peaks of the playing track, while it has them.
    pub(crate) waveform: Option<std::sync::Arc<[u8]>>,
}

impl PlayerView {
    /// The album of the playing track.
    pub(crate) fn album(&self) -> Option<AlbumKey> {
        self.now.as_ref().map(|n| n.track.album)
    }

    /// The time left on the sleep timer now, counted down from its report.
    pub(crate) fn sleep_left(&self) -> Option<Duration> {
        self.sleep
            .map(|(timer, at)| timer.remaining.saturating_sub(at.elapsed()))
    }
}

/// Which part of the view an update changed, so listeners can skip work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    State,
    Track,
    Position,
    Volume,
    Output,
    Queue,
    Modes,
    /// The signal path.
    Signal,
    /// The playback counters.
    Stats,
    /// The A-B loop.
    Loop,
    /// The sleep timer.
    Sleep,
    /// The entry selected in the queue.
    Cursor,
    /// The waveform peaks of the playing track.
    Waveform,
}

type Listener = Box<dyn Fn(&PlayerView, Change)>;
type EventListener = Box<dyn Fn(&Update)>;

/// The player state and its listeners.
#[derive(Default)]
pub(crate) struct Playback {
    view: RefCell<PlayerView>,
    listeners: RefCell<Vec<Listener>>,
    events: RefCell<Vec<EventListener>>,
}

impl std::fmt::Debug for Playback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Playback")
            .field("view", &self.view)
            .finish()
    }
}

impl Playback {
    /// Follows the updates of `backend` for as long as the result lives.
    pub(crate) fn follow(backend: &Backend) -> Rc<Self> {
        let playback = Rc::new(Self::default());
        if let Some(updates) = backend.take_updates() {
            let weak = Rc::downgrade(&playback);
            glib::spawn_future_local(async move {
                while let Ok(update) = updates.recv().await {
                    let Some(playback) = weak.upgrade() else {
                        break;
                    };
                    playback.apply(update);
                }
            });
        }
        playback
    }

    /// The current view.
    pub(crate) fn view(&self) -> PlayerView {
        self.view.borrow().clone()
    }

    /// Calls `listener` after each change, and once now.
    pub(crate) fn connect(&self, listener: impl Fn(&PlayerView, Change) + 'static) {
        let view = self.view();
        for change in [
            Change::State,
            Change::Track,
            Change::Volume,
            Change::Output,
            Change::Queue,
            Change::Signal,
            Change::Stats,
            Change::Modes,
            Change::Loop,
            Change::Sleep,
            Change::Cursor,
            Change::Waveform,
        ] {
            listener(&view, change);
        }
        self.listeners.borrow_mut().push(Box::new(listener));
    }

    /// Calls `listener` with each update that is not part of the player's state:
    /// problems, notices, thumbnails, library changes, raise and quit.
    pub(crate) fn connect_event(&self, listener: impl Fn(&Update) + 'static) {
        self.events.borrow_mut().push(Box::new(listener));
    }

    /// Applies one update and tells the listeners.
    pub(crate) fn apply(&self, update: Update) {
        let changes: &[Change] = {
            let mut view = self.view.borrow_mut();
            match update {
                Update::State(state) => {
                    view.state = state;
                    crate::widgets::set_playing_indicators_running(state == PlaybackState::Playing);
                    &[Change::State]
                }
                Update::NowPlaying(now) => {
                    view.length = now.as_ref().and_then(|n| n.track.duration);
                    view.elapsed = Duration::ZERO;
                    view.now = now;
                    &[Change::Track]
                }
                Update::Position { elapsed, length } => {
                    view.elapsed = elapsed;
                    if length.is_some() {
                        view.length = length;
                    }
                    &[Change::Position]
                }
                Update::Volume(volume) => {
                    view.volume = volume;
                    &[Change::Volume]
                }
                Update::Output(output) => {
                    view.output = output;
                    &[Change::Output]
                }
                Update::Queue(queue) => {
                    view.queue = queue;
                    &[Change::Queue]
                }
                Update::Modes { shuffle, repeat } => {
                    view.shuffle = shuffle;
                    view.repeat = repeat;
                    // The full modes follow; until then the coarse ones hold.
                    if shuffle != (view.modes.shuffle != ShuffleMode::Off) {
                        view.modes.shuffle = if shuffle {
                            ShuffleMode::Tracks
                        } else {
                            ShuffleMode::Off
                        };
                    }
                    if repeat != coarse_repeat(view.modes.repeat) {
                        view.modes.repeat = match repeat {
                            Repeat::Off => RepeatMode::Off,
                            Repeat::All => RepeatMode::All,
                            Repeat::One => RepeatMode::One,
                        };
                    }
                    &[Change::Modes]
                }
                Update::StopAfter(on) => {
                    view.stop_after = on;
                    if on != (view.stop_after_mode != StopAfter::Off) {
                        view.stop_after_mode = if on { StopAfter::Track } else { StopAfter::Off };
                    }
                    &[Change::Modes]
                }
                Update::PlayModes(modes) => {
                    view.modes = modes;
                    view.shuffle = modes.shuffle != ShuffleMode::Off;
                    view.repeat = coarse_repeat(modes.repeat);
                    &[Change::Modes]
                }
                Update::StopAfterMode(mode) => {
                    view.stop_after_mode = mode;
                    view.stop_after = mode != StopAfter::Off;
                    &[Change::Modes]
                }
                Update::Loop(points) => {
                    view.ab_loop = points;
                    &[Change::Loop]
                }
                Update::SleepTimer(timer) => {
                    view.sleep = timer.map(|t| (t, Instant::now()));
                    &[Change::Sleep]
                }
                Update::Cursor(cursor) => {
                    view.cursor = cursor;
                    &[Change::Cursor]
                }
                Update::Waveform(peaks) => {
                    view.waveform = peaks;
                    &[Change::Waveform]
                }
                // The output button follows the path: its lamp and the format
                // the device was opened with.
                Update::SignalPath(Some(path)) => {
                    view.output.follow(&path);
                    view.signal = Some(Rc::new(*path));
                    &[Change::Output, Change::Signal]
                }
                Update::SignalPath(None) => {
                    view.signal = None;
                    view.stats = None;
                    &[Change::Signal, Change::Stats]
                }
                Update::Stats(stats) => {
                    view.stats = Some(stats);
                    &[Change::Stats]
                }
                // The playing track keeps its new rating; the listeners show it.
                Update::RatingChanged { track, stars } => {
                    if let Some(now) = view.now.as_mut().filter(|n| n.track.key == track) {
                        now.track.rating = stars;
                    }
                    drop(view);
                    let update = Update::RatingChanged { track, stars };
                    for listener in self.events.borrow().iter() {
                        listener(&update);
                    }
                    return;
                }
                other => {
                    drop(view);
                    for listener in self.events.borrow().iter() {
                        listener(&other);
                    }
                    return;
                }
            }
        };
        let view = self.view();
        for change in changes {
            for listener in self.listeners.borrow().iter() {
                listener(&view, *change);
            }
        }
    }
}

/// Repeat as the coarse modes name it: "this album" counts as repeating.
fn coarse_repeat(mode: RepeatMode) -> Repeat {
    match mode {
        RepeatMode::Off => Repeat::Off,
        RepeatMode::One => Repeat::One,
        _ => Repeat::All,
    }
}
