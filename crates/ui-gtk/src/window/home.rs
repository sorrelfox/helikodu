// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Home page in the window: its links open the albums by date added and
//! the play history (`win.show-history`).

use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::{MainWindow, Page};
use crate::backend::Backend;
use crate::models::CoverCache;
use crate::playback::Playback;
use crate::views::home::HomeView;

/// The name of the page in the view stack and the sidebar.
pub(crate) const PAGE: &str = "home";

impl MainWindow {
    pub(super) fn setup_home(
        &self,
        backend: &Backend,
        playback: &Rc<Playback>,
        covers: &Rc<CoverCache>,
    ) {
        let view = HomeView::new(backend, playback, covers);
        let weak = self.downgrade();
        view.connect_open(move |key| {
            if let Some(window) = weak.upgrade() {
                window.open_album(key);
            }
        });
        let weak = self.downgrade();
        view.connect_show_all(move || {
            let Some(window) = weak.upgrade() else { return };
            window.go(Page::Library("albums"), true);
            if let Some(albums) = window.imp().albums.borrow().as_ref() {
                let _ = albums.activate_action("albums.sort", Some(&"added".to_variant()));
            }
        });
        self.imp().views.add_named(&view.root, Some(PAGE));
        self.imp().home_view.replace(Some(view));
    }

    fn home_view(&self) -> Option<Rc<HomeView>> {
        self.imp().home_view.borrow().clone()
    }

    /// Shows the Home page.
    pub(super) fn show_home(&self) -> String {
        if let Some(view) = self.home_view() {
            view.show();
        }
        self.imp().views.set_visible_child_name(PAGE);
        PAGE.into()
    }

    /// Reads Home again after the library changed.
    pub(super) fn home_changed(&self) {
        if let Some(view) = self.home_view() {
            view.library_changed();
        }
    }

    pub(super) fn home_focus_target(&self) -> Option<gtk::Widget> {
        Some(self.home_view()?.focus_target())
    }

    /// What Home shows: the tiles in each row and the two lists (for tests).
    #[doc(hidden)]
    pub fn home_texts(&self) -> Option<(usize, usize, Vec<String>, Vec<String>)> {
        self.home_view().map(|v| v.texts())
    }
}
