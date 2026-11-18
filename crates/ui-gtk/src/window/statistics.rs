// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Library statistics page (`win.show-statistics`, from the Library menu),
//! and what the tests need of the facet browser of the Tracks page.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::{MainWindow, Page};
use crate::backend::{Backend, FacetField, LibraryStats};
use crate::models::CoverCache;
use crate::views::statistics::{Opens, StatisticsView};

/// The name of the page in the view stack.
pub(crate) const PAGE: &str = "statistics";

impl MainWindow {
    pub(super) fn setup_statistics(&self, backend: &Backend, covers: &Rc<CoverCache>) {
        let tracks = self.downgrade();
        let artist = self.downgrade();
        let album = self.downgrade();
        let opens = Opens {
            tracks: Box::new(move |query| {
                if let Some(window) = tracks.upgrade() {
                    window.show_all_tracks(query);
                }
            }),
            artist: Box::new(move |key| {
                if let Some(window) = artist.upgrade() {
                    window.open_artist(key);
                }
            }),
            album: Box::new(move |key| {
                if let Some(window) = album.upgrade() {
                    window.open_album(key);
                }
            }),
        };
        let view = StatisticsView::new(backend, covers, opens);
        self.imp().views.add_named(&view.root, Some(PAGE));
        self.imp().statistics.replace(Some(view));
        let show = gio::ActionEntry::builder("show-statistics")
            .activate(|window: &Self, _, _| window.go(Page::Library(PAGE), true))
            .build();
        self.add_action_entries([show]);
    }

    fn statistics_view(&self) -> Option<Rc<StatisticsView>> {
        self.imp().statistics.borrow().clone()
    }

    /// Shows the statistics page, counting again; Home stays selected in the
    /// sidebar.
    pub(super) fn show_statistics(&self) -> String {
        if let Some(view) = self.statistics_view() {
            view.show();
        }
        self.imp().views.set_visible_child_name(PAGE);
        "home".into()
    }

    pub(super) fn statistics_focus(&self) -> Option<gtk::Widget> {
        self.statistics_view().map(|v| v.focus_target())
    }

    /// The figures of the statistics page once counted (for tests).
    #[doc(hidden)]
    pub fn statistics(&self) -> Option<LibraryStats> {
        self.statistics_view().and_then(|v| v.stats())
    }

    /// The texts of the statistics page (for tests).
    #[doc(hidden)]
    pub fn statistics_texts(&self) -> Vec<String> {
        self.statistics_view()
            .map(|v| v.texts())
            .unwrap_or_default()
    }

    /// Opens or closes the facet browser of the Tracks page (for tests).
    #[doc(hidden)]
    pub fn set_browsing(&self, shown: bool) {
        if let Some(pages) = self.pages() {
            pages.tracks.set_browsing(shown);
        }
    }

    /// Each facet pane's rows: name, tracks and whether it is chosen (for tests).
    #[doc(hidden)]
    pub fn facet_rows(&self) -> Vec<Vec<(String, usize, bool)>> {
        self.pages()
            .and_then(|p| p.tracks.facet_browser())
            .map(|b| b.rows())
            .unwrap_or_default()
    }

    /// Chooses rows of a facet pane as a click and Ctrl-clicks would (for tests).
    #[doc(hidden)]
    pub fn choose_facets(&self, pane: usize, positions: &[u32]) {
        if let Some(browser) = self.pages().and_then(|p| p.tracks.facet_browser()) {
            browser.choose(pane, positions);
        }
    }

    /// Changes the field of a facet pane (for tests).
    #[doc(hidden)]
    pub fn set_facet_field(&self, pane: usize, field: FacetField) {
        if let Some(browser) = self.pages().and_then(|p| p.tracks.facet_browser()) {
            browser.choose_field(pane, field);
        }
    }

    /// The number of rows of the Tracks table and the count in its toolbar
    /// (for tests).
    #[doc(hidden)]
    pub fn tracks_shown(&self) -> (usize, String) {
        self.pages()
            .map(|p| p.tracks.shown_count())
            .unwrap_or_default()
    }
}
