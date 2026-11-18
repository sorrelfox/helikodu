// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The player's state on the main thread: built from the backend's updates,
//! read by the deck, the queue panel and the views.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;

use crate::backend::{
    AlbumKey, Backend, NowPlaying, OutputInfo, PlaybackState, PlaybackStats, QueueSnapshot, Repeat,
    SignalPath, Update,
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
}

impl PlayerView {
    /// The album of the playing track.
    pub(crate) fn album(&self) -> Option<AlbumKey> {
        self.now.as_ref().map(|n| n.track.album)
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
                    &[Change::Modes]
                }
                Update::StopAfter(on) => {
                    view.stop_after = on;
                    &[Change::Modes]
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
