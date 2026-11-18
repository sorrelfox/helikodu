// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the main window does for the dialogs: the full-output gate in front
//! of the player, the output switcher on the deck's output button, the scan
//! footer in the sidebar, the first run when the library has no folder, and
//! a few hooks the dialogs and the tests call.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::MainWindow;
use crate::backend::{Backend, Update};
use crate::dialogs::{first_run, settings, volume_confirm};
use crate::playback::Playback;

/// The first descendant of `root` that `test` accepts.
fn find(root: &gtk::Widget, test: &dyn Fn(&gtk::Widget) -> bool) -> Option<gtk::Widget> {
    let mut pending = vec![root.clone()];
    while let Some(widget) = pending.pop() {
        if test(&widget) {
            return Some(widget);
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c);
        }
    }
    None
}

impl MainWindow {
    /// Sets up the parts the dialogs need. Called once from `setup`.
    pub(super) fn setup_dialogs(&self, backend: &Backend, playback: &Rc<Playback>) {
        volume_confirm::install_gate(self.upcast_ref(), backend, playback);
        if let Some(button) = self.output_button() {
            crate::popovers::output_switcher::attach(self.upcast_ref(), &button, backend, playback);
        }
        let footer = Rc::new(first_run::ScanFooter::new());
        footer.root.set_widget_name("scan-footer");
        if let Some(sidebar) = self.imp().sidebar.borrow().as_ref() {
            sidebar.root.append(&footer.root);
        }
        let weak = self.downgrade();
        playback.connect_event(move |update| {
            // Applying the copies setting reads files as a scan does, and
            // ends without a scan's end.
            if let Update::DuplicatesApplied(_) = update {
                footer.update(&crate::backend::services::ScanStatus {
                    finished: true,
                    ..Default::default()
                });
                return;
            }
            let Update::Scan(status) = update else { return };
            footer.update(status);
            if status.finished
                && let Some(window) = weak.upgrade()
            {
                let (message, error) = first_run::finished_message(status);
                window.toast(&message, error);
            }
        });
        if let Some(sidebar) = self.imp().sidebar.borrow().as_ref() {
            crate::dialogs::loudness_footer::attach(
                self,
                &sidebar.root,
                backend.services(),
                playback,
            );
        }
        first_run::show_if_needed(self);
    }

    /// Opens the copies window of the track `track` (for tests and
    /// screenshots).
    #[doc(hidden)]
    pub fn open_copies(&self, track: u64) -> Option<gtk::Window> {
        crate::tools::copies::open(self, crate::backend::TrackKey(track))
    }

    /// Opens Find duplicates, matching by `by` (`same-recording`,
    /// `same-album-track`, `identical-files` or `same-file`), for tests and
    /// screenshots.
    #[doc(hidden)]
    pub fn open_find_duplicates(&self, by: &str) -> Option<gtk::Window> {
        let window = crate::tools::duplicates::open(self)?;
        crate::tools::duplicates::choose(&window, by);
        Some(window)
    }

    /// The deck's output button.
    fn output_button(&self) -> Option<gtk::Button> {
        let deck = self.imp().deck.get().upcast::<gtk::Widget>();
        find(&deck, &|w| {
            w.has_css_class("hk-output") && w.is::<gtk::Button>()
        })
        .and_downcast::<gtk::Button>()
    }

    /// What the player is doing, as the window follows it.
    pub(crate) fn playback(&self) -> Option<Rc<Playback>> {
        self.imp().playback.borrow().clone()
    }

    /// Shows or hides the menu bar (Classic style only).
    pub(crate) fn set_menubar_shown(&self, shown: bool) {
        let Some(action) = self
            .lookup_action("show-menubar")
            .and_downcast::<gio::SimpleAction>()
        else {
            return;
        };
        let now = action
            .state()
            .and_then(|s| s.get::<bool>())
            .unwrap_or(false);
        if now != shown {
            action.activate(None);
        }
    }

    /// Opens the Settings window at the page named `page` (`library`,
    /// `playback`, `output`, `loudness`, `appearance` or `advanced`).
    #[doc(hidden)]
    pub fn open_settings(&self, page: &str) -> gtk::Window {
        let page = match page {
            "playback" => settings::Page::Playback,
            "output" => settings::Page::Output,
            "loudness" => settings::Page::Loudness,
            "dsp" => settings::Page::Dsp,
            "appearance" => settings::Page::Appearance,
            "advanced" => settings::Page::Advanced,
            _ => settings::Page::Library,
        };
        settings::open(self, Some(page))
    }

    /// Opens Settings › DSP and EQ at the editor of `stage` (`equalizer`,
    /// `crossfeed`, `channel-mix` or `limiter`).
    #[doc(hidden)]
    pub fn open_dsp_stage(&self, stage: &str) -> gtk::Window {
        use crate::dialogs::settings::dsp::{Stage, open_stage};
        let stage = match stage {
            "crossfeed" => Stage::Crossfeed,
            "channel-mix" => Stage::ChannelMix,
            "limiter" => Stage::Limiter,
            _ => Stage::Equalizer,
        };
        open_stage(self, stage)
    }

    /// The page shown in the Settings window, if it is open.
    #[doc(hidden)]
    pub fn settings_page(&self) -> Option<String> {
        settings::shown_page(self)
    }

    /// Opens the output switcher above the deck's output button.
    #[doc(hidden)]
    pub fn open_output_switcher(&self) -> Option<gtk::Popover> {
        let button = self.output_button()?;
        let popover = find(button.upcast_ref(), &|w| w.is::<gtk::Popover>())
            .and_downcast::<gtk::Popover>()?;
        popover.popup();
        Some(popover)
    }

    /// Opens the tag editor for `tracks` (the views call it with their
    /// selection).
    #[doc(hidden)]
    pub fn edit_tags(&self, tracks: &[crate::backend::TrackKey]) {
        crate::dialogs::tag_editor::open(self, tracks);
    }

    /// Whether the first run is shown.
    #[doc(hidden)]
    pub fn first_run_shown(&self) -> bool {
        first_run::is_shown(self)
    }

    /// Whether the scan footer is shown in the sidebar.
    #[doc(hidden)]
    pub fn scan_footer_shown(&self) -> bool {
        let Some(sidebar) = self.imp().sidebar.borrow().clone() else {
            return false;
        };
        find(sidebar.root.upcast_ref(), &|w| {
            w.widget_name() == "scan-footer"
        })
        .is_some_and(|w| w.is_visible())
    }

    /// Shows a scan's progress in the scan footer, as the library reports it.
    #[doc(hidden)]
    pub fn show_scan_status(&self, status: &crate::backend::services::ScanStatus) {
        let Some(sidebar) = self.imp().sidebar.borrow().clone() else {
            return;
        };
        if let Some(footer) = find(sidebar.root.upcast_ref(), &|w| {
            w.widget_name() == "scan-footer"
        })
        .and_downcast::<gtk::Box>()
        .and_then(first_run::ScanFooter::from_root)
        {
            footer.update(status);
        }
    }

    /// Whether the player plays.
    #[doc(hidden)]
    pub fn is_playing(&self) -> bool {
        self.playback()
            .is_some_and(|p| p.view().state == crate::backend::PlaybackState::Playing)
    }
}
