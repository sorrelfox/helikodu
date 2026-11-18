// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Play history page in the window, its actions (`win.show-history` from
//! the Library menu and Home, `win.export-history`), and the export of the
//! history to a file.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use super::{MainWindow, Page};
use crate::i18n::{fill, ntr, tr};
use crate::views::history::HistoryView;

/// The name of the page in the view stack.
pub(crate) const PAGE: &str = "history";

impl MainWindow {
    pub(super) fn setup_history(&self) {
        let show = gio::ActionEntry::builder("show-history")
            .activate(|window: &Self, _, _| window.go(Page::Library(PAGE), true))
            .build();
        let export = gio::ActionEntry::builder("export-history")
            .activate(|window: &Self, _, _| window.export_history())
            .build();
        self.add_action_entries([show, export]);
    }

    /// The history page, built when it is first wanted.
    pub(super) fn history_view(&self) -> Option<Rc<HistoryView>> {
        if let Some(view) = self.imp().history_view.borrow().clone() {
            return Some(view);
        }
        let pages = self.pages()?;
        let view = HistoryView::new(&pages.backend, &pages.playback, &pages.covers);
        self.imp().views.add_named(&view.root, Some(PAGE));
        self.imp().history_view.replace(Some(view.clone()));
        Some(view)
    }

    /// Shows the history page; Home stays selected in the sidebar.
    pub(super) fn show_history(&self) -> String {
        if let Some(view) = self.history_view() {
            view.show();
        }
        self.imp().views.set_visible_child_name(PAGE);
        "home".into()
    }

    /// The history page (for tests).
    #[doc(hidden)]
    pub fn history_counts(&self) -> Option<(usize, usize)> {
        self.history_view().map(|v| v.counts())
    }

    /// Asks where to write the history, then writes it.
    fn export_history(&self) {
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        for (title, suffix) in [
            (tr("ListenBrainz listens (JSON Lines)"), "jsonl"),
            (tr("Scrobbler logs"), "log"),
        ] {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(&title));
            filter.add_suffix(suffix);
            filters.append(&filter);
        }
        let dialog = gtk::FileDialog::builder()
            .title(tr("Export the play history"))
            .initial_name("listens.jsonl")
            .filters(&filters)
            .modal(true)
            .build();
        let weak = self.downgrade();
        dialog.save(Some(self), gio::Cancellable::NONE, move |file| {
            let (Some(window), Ok(Some(path))) = (weak.upgrade(), file.map(|f| f.path())) else {
                return;
            };
            let Some(backend) = window.backend() else {
                return;
            };
            let weak = window.downgrade();
            glib::spawn_future_local(async move {
                let result = backend.query(move |l| l.export_history(&path, None)).await;
                if let Some(window) = weak.upgrade() {
                    match result {
                        Ok(count) => window.toast(
                            &fill(
                                &ntr("{} listen exported", "{} listens exported", count),
                                &[&crate::format::count(count)],
                            ),
                            false,
                        ),
                        Err(error) => window.toast(&error.0, true),
                    }
                }
            });
        });
    }
}
