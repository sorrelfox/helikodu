// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The deck's heart: it shows whether the playing track is in Favourites, the
//! built-in playlist first in the sidebar, and adds it there or takes it out.
//! Only tracks of the library can be favourites; for anything else the button
//! is off.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::Deck;
use crate::backend::TrackKey;
use crate::i18n::tr;

impl Deck {
    /// Connects the heart. Called once from `bind`.
    pub(super) fn setup_favourite(&self) {
        let weak = self.downgrade();
        self.imp().favourite.connect_clicked(move |_| {
            if let Some(deck) = weak.upgrade() {
                deck.toggle_favourite();
            }
        });
    }

    /// Follows the playing track `track`: asks the library whether it is a
    /// favourite when it is another one than before.
    pub(super) fn follow_favourite(&self, track: Option<TrackKey>) {
        let imp = self.imp();
        let track = track.filter(|t| t.0 != 0);
        if imp.favourite_track.get() == track {
            return;
        }
        imp.favourite_track.set(track);
        imp.favourite.set_sensitive(track.is_some());
        self.show_favourite(false);
        if let Some(track) = track {
            self.ask_favourite(track);
        }
    }

    /// Shows whether `track` is a favourite once the library has answered.
    fn ask_favourite(&self, track: TrackKey) {
        let Some(backend) = self.imp().backend.borrow().clone() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let on = backend
                .query(move |l| l.is_favourite(track))
                .await
                .unwrap_or(false);
            if let Some(deck) = weak.upgrade()
                && deck.imp().favourite_track.get() == Some(track)
            {
                deck.show_favourite(on);
            }
        });
    }

    /// Asks the library again, after a playlist changed.
    pub(crate) fn refresh_favourite(&self) {
        if let Some(track) = self.imp().favourite_track.get() {
            self.ask_favourite(track);
        }
    }

    /// Whether the heart shows the playing track as a favourite.
    pub(crate) fn is_favourite_shown(&self) -> bool {
        self.imp().favourite_on.get()
    }

    fn show_favourite(&self, on: bool) {
        let imp = self.imp();
        imp.favourite_on.set(on);
        let name = if on {
            imp.favourite.add_css_class("active");
            tr("Remove from Favourites")
        } else {
            imp.favourite.remove_css_class("active");
            tr("Add to Favourites")
        };
        imp.favourite.set_tooltip_text(Some(&name));
        imp.favourite
            .update_property(&[gtk::accessible::Property::Label(&name)]);
        imp.favourite
            .update_state(&[gtk::accessible::State::Pressed(if on {
                gtk::AccessibleTristate::True
            } else {
                gtk::AccessibleTristate::False
            })]);
    }

    /// Adds the playing track to Favourites, or takes it out, and has the
    /// sidebar count it again.
    pub(crate) fn toggle_favourite(&self) {
        let imp = self.imp();
        let (Some(track), Some(backend)) =
            (imp.favourite_track.get(), imp.backend.borrow().clone())
        else {
            return;
        };
        let want = !imp.favourite_on.get();
        self.show_favourite(want);
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = backend.query(move |l| l.set_favourite(track, want)).await;
            let Some(deck) = weak.upgrade() else { return };
            let window = deck.root().and_downcast::<crate::MainWindow>();
            match result {
                Ok(on) => {
                    if deck.imp().favourite_track.get() == Some(track) {
                        deck.show_favourite(on);
                    }
                    if let Some(window) = window {
                        let list = window
                            .playlist_rows()
                            .into_iter()
                            .find(|p| p.builtin)
                            .map(|p| p.key);
                        window.playlists_changed(list);
                    }
                }
                Err(error) => {
                    if deck.imp().favourite_track.get() == Some(track) {
                        deck.show_favourite(!want);
                    }
                    if let Some(window) = window {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }
}
