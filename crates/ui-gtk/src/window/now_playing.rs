// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Now Playing in the main window, and the lyrics actions the Lyrics tab and
//! Now Playing share.
//!
//! The view takes the place of the sidebar, the content and the panel; the
//! deck stays, with the next track in its start group. F11, the deck's cover
//! and the cover of the Lyrics tab open it; F11, Escape and "Library" leave it.
//!
//! Actions: `win.now-playing` (toggle), `win.lyrics-sooner` and
//! `win.lyrics-later` (the offset in steps of 0.1 s), `win.lyrics-show-times`,
//! `win.lyrics-reload`, `win.lyrics-open-folder` and `win.lyrics-edit`.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::{MainWindow, WindowStyle};
use crate::backend::{Backend, PlayerRequest, TrackKey};
use crate::models::CoverCache;
use crate::models::lyrics::{LyricsChange, LyricsModel};
use crate::playback::{Change, Playback};
use crate::views::now_playing::{NowPlayingView, next_text};

/// One step of the offset buttons, in milliseconds.
const OFFSET_STEP: i64 = 100;

impl MainWindow {
    /// Builds Now Playing and the lyrics actions.
    pub(super) fn setup_now_playing(
        &self,
        backend: &Backend,
        playback: &Rc<Playback>,
        lyrics: &Rc<LyricsModel>,
        covers: &Rc<CoverCache>,
    ) {
        let imp = self.imp();
        let view = NowPlayingView::new(backend, playback, lyrics, covers);
        // Beside the panes, in their place while shown.
        if let Some(parent) = imp.panes_overlay.parent().and_downcast::<gtk::Box>() {
            parent.insert_child_after(view.widget(), Some(&imp.panes_overlay.get()));
        }
        let keys = gtk::ShortcutController::new();
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(gtk::NamedAction::new("win.now-playing")),
        ));
        view.widget().add_controller(keys);
        imp.now_playing.replace(Some(view));

        let weak = self.downgrade();
        imp.deck.connect_cover_activated(move || {
            if let Some(window) = weak.upgrade() {
                window.set_now_playing(!window.now_playing_shown());
            }
        });
        // The deck shows the next track while the view is open.
        let weak = self.downgrade();
        playback.connect(move |player, change| {
            if let Some(window) = weak.upgrade()
                && window.now_playing_shown()
                && matches!(change, Change::Track | Change::Queue)
            {
                window
                    .imp()
                    .deck
                    .show_next(Some(&next_text(player).unwrap_or_default()));
            }
        });
        // The offset and the menu follow the lyrics.
        let weak = self.downgrade();
        lyrics.connect(move |model, change| {
            if let Some(window) = weak.upgrade()
                && change == LyricsChange::Lyrics
            {
                window.update_lyrics_actions(model);
            }
        });

        let toggle = gio::ActionEntry::builder("now-playing")
            .activate(|window: &Self, _, _| {
                window.set_now_playing(!window.now_playing_shown());
            })
            .build();
        let send = |name: &str, request: PlayerRequest| {
            let backend = backend.clone();
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(move |_, _| backend.send(request.clone()));
            action
        };
        self.add_action(&send(
            "lyrics-sooner",
            PlayerRequest::AdjustLyricsOffset(OFFSET_STEP),
        ));
        self.add_action(&send(
            "lyrics-later",
            PlayerRequest::AdjustLyricsOffset(-OFFSET_STEP),
        ));
        self.add_action(&send("lyrics-reload", PlayerRequest::ReloadLyrics));
        let show_times = gio::ActionEntry::builder("lyrics-show-times")
            .state(false.to_variant())
            .activate(|window: &Self, action, _| {
                let shown = !action
                    .state()
                    .and_then(|s| s.get::<bool>())
                    .unwrap_or(false);
                action.set_state(&shown.to_variant());
                if let Some(page) = window.imp().panel.lyrics_page() {
                    page.lines().set_show_times(shown);
                }
            })
            .build();
        let lyrics_model = Rc::downgrade(lyrics);
        let open_folder = gio::ActionEntry::builder("lyrics-open-folder")
            .activate(move |window: &Self, _, _| {
                let file = lyrics_model
                    .upgrade()
                    .and_then(|m| m.lyrics())
                    .and_then(|l| l.file.clone());
                crate::views::actions::open_folder(window, file);
            })
            .build();
        let pb = Rc::downgrade(playback);
        let edit = gio::ActionEntry::builder("lyrics-edit")
            .activate(move |window: &Self, _, _| {
                let key = pb
                    .upgrade()
                    .and_then(|p| p.view().now)
                    .map(|n| n.track.key)
                    .filter(|k| *k != TrackKey(0));
                match key {
                    Some(key) => window.open_tag_editor(vec![key]),
                    None => window.toast(
                        &crate::i18n::tr("Only tracks of the library can be edited."),
                        true,
                    ),
                }
            })
            .build();
        self.add_action_entries([toggle, show_times, open_folder, edit]);
        self.update_lyrics_actions(lyrics);
    }

    /// Enables the lyrics actions that apply to the lyrics shown.
    fn update_lyrics_actions(&self, model: &LyricsModel) {
        let lyrics = model.lyrics();
        let synced = lyrics.as_ref().is_some_and(|l| l.synced);
        let file = lyrics.as_ref().is_some_and(|l| l.file.is_some());
        for (name, enabled) in [
            ("lyrics-sooner", synced),
            ("lyrics-later", synced),
            ("lyrics-show-times", synced),
            ("lyrics-open-folder", file),
        ] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
    }

    /// Opens or leaves Now Playing.
    pub fn set_now_playing(&self, shown: bool) {
        let imp = self.imp();
        let Some(view) = imp.now_playing.borrow().clone() else {
            return;
        };
        if view.is_active() == shown {
            return;
        }
        let integrated = imp.style.get() == WindowStyle::Integrated;
        imp.panes_overlay.set_visible(!shown);
        view.set_active(shown, integrated);
        let next = shown
            .then(|| {
                imp.playback
                    .borrow()
                    .as_ref()
                    .map(|p| next_text(&p.view()).unwrap_or_default())
            })
            .flatten();
        imp.deck.show_next(next.as_deref());
        if shown {
            view.remember_focus(gtk::prelude::RootExt::focus(self));
            view.focus_target().grab_focus();
        } else {
            // Back where the user was.
            let back = view.take_focus().filter(|w| w.is_mapped());
            match back {
                Some(widget) => {
                    widget.grab_focus();
                }
                None => {
                    imp.deck.focus_target().grab_focus();
                }
            }
        }
    }

    /// Whether Now Playing is shown.
    pub fn now_playing_shown(&self) -> bool {
        self.imp()
            .now_playing
            .borrow()
            .as_ref()
            .is_some_and(|v| v.is_active())
    }

    /// What Now Playing shows, in words: title, album line, page, note, the
    /// lines (the current one after "> "), the signal chain.
    #[doc(hidden)]
    pub fn now_playing_texts(&self) -> Vec<String> {
        self.imp()
            .now_playing
            .borrow()
            .as_ref()
            .map(|v| v.texts())
            .unwrap_or_default()
    }

    /// Shows the panel's Lyrics tab.
    #[doc(hidden)]
    pub fn show_lyrics_tab(&self) {
        self.imp().panel.show_lyrics();
    }

    /// What the Lyrics tab shows: its lines, the current one, the source.
    #[doc(hidden)]
    pub fn lyrics_tab_texts(&self) -> (Vec<String>, Option<usize>, String) {
        self.imp()
            .panel
            .lyrics_page()
            .map(|p| p.texts())
            .unwrap_or_default()
    }

    /// Activates line `index` of the Lyrics tab (Now Playing's when it is
    /// shown), as a click does; whether there is such a line.
    #[doc(hidden)]
    pub fn activate_lyric_line(&self, index: usize) -> bool {
        if let Some(shown) = self
            .with_now_playing(|v| v.is_active().then(|| v.lines().activate(index)))
            .flatten()
        {
            return shown;
        }
        self.imp()
            .panel
            .lyrics_page()
            .is_some_and(|p| p.lines().activate(index))
    }

    /// Runs `f` on the view, for tests.
    pub(crate) fn with_now_playing<T>(&self, f: impl FnOnce(&NowPlayingView) -> T) -> Option<T> {
        self.imp().now_playing.borrow().as_deref().map(f)
    }
}
