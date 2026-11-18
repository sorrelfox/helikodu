// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The lyrics of the playing track and the line sung now, on the main thread:
//! built from the backend's updates, read by the Lyrics tab of the panel and
//! by Now Playing.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::backend::{LyricsView, Update};
use crate::playback::Playback;

/// What changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LyricsChange {
    /// Other lyrics, or the same with another offset.
    Lyrics,
    /// Another line is sung.
    Line,
}

type Listener = Box<dyn Fn(&LyricsModel, LyricsChange)>;

/// The lyrics and their listeners.
#[derive(Default)]
pub(crate) struct LyricsModel {
    lyrics: RefCell<Option<Arc<LyricsView>>>,
    line: RefCell<Option<usize>>,
    listeners: RefCell<Vec<Listener>>,
}

impl std::fmt::Debug for LyricsModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LyricsModel")
            .field(
                "lines",
                &self.lyrics.borrow().as_ref().map(|l| l.lines.len()),
            )
            .field("line", &self.line.borrow())
            .finish()
    }
}

impl LyricsModel {
    /// Follows the lyrics updates that reach `playback`.
    pub(crate) fn follow(playback: &Playback) -> Rc<Self> {
        let model = Rc::new(Self::default());
        let weak = Rc::downgrade(&model);
        playback.connect_event(move |update| {
            let Some(model) = weak.upgrade() else { return };
            match update {
                Update::Lyrics(lyrics) => model.set_lyrics(lyrics.clone()),
                Update::LyricsLine(line) => model.set_line(*line),
                _ => {}
            }
        });
        model
    }

    /// The lyrics shown.
    pub(crate) fn lyrics(&self) -> Option<Arc<LyricsView>> {
        self.lyrics.borrow().clone()
    }

    /// The line sung now.
    pub(crate) fn line(&self) -> Option<usize> {
        *self.line.borrow()
    }

    /// Calls `listener` after each change.
    pub(crate) fn connect(&self, listener: impl Fn(&Self, LyricsChange) + 'static) {
        self.listeners.borrow_mut().push(Box::new(listener));
    }

    pub(crate) fn set_lyrics(&self, lyrics: Option<Arc<LyricsView>>) {
        if *self.lyrics.borrow() == lyrics {
            return;
        }
        self.lyrics.replace(lyrics);
        self.notify(LyricsChange::Lyrics);
    }

    pub(crate) fn set_line(&self, line: Option<usize>) {
        if *self.line.borrow() == line {
            return;
        }
        self.line.replace(line);
        self.notify(LyricsChange::Line);
    }

    fn notify(&self, change: LyricsChange) {
        for listener in self.listeners.borrow().iter() {
            listener(self, change);
        }
    }
}
