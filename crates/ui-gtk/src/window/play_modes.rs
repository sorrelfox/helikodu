// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the main window does for the play modes: the Playback menu with the
//! modes in its labels, the states of the mode actions, "Sleep timer…",
//! "Follow the playing track" in the View menu, and the queue panel's
//! selection, which is the entry "Continue from the selected track" goes on
//! from. A single click selects a row there; a double click or Enter plays it.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use super::MainWindow;
use crate::backend::{Backend, PlayerRequest};
use crate::i18n::tr;
use crate::playback::{Change, Playback, PlayerView};

/// The submenu of the main menu labelled `label`.
fn submenu(menu: &gio::MenuModel, label: &str) -> Option<gio::Menu> {
    for section in 0..menu.n_items() {
        let Some(items) = menu.item_link(section, gio::MENU_LINK_SECTION) else {
            continue;
        };
        for i in 0..items.n_items() {
            let name = items
                .item_attribute_value(i, gio::MENU_ATTRIBUTE_LABEL, None)
                .and_then(|v| v.get::<String>());
            if name.as_deref() == Some(label) {
                return items
                    .item_link(i, gio::MENU_LINK_SUBMENU)
                    .and_downcast::<gio::Menu>();
            }
        }
    }
    None
}

/// The labels of `menu` and its submenus, depth first.
fn labels(menu: &gio::MenuModel, out: &mut Vec<String>) {
    for i in 0..menu.n_items() {
        if let Some(label) = menu
            .item_attribute_value(i, gio::MENU_ATTRIBUTE_LABEL, None)
            .and_then(|v| v.get::<String>())
        {
            out.push(label);
        }
        for link in [gio::MENU_LINK_SECTION, gio::MENU_LINK_SUBMENU] {
            if let Some(inner) = menu.item_link(i, link) {
                labels(&inner, out);
            }
        }
    }
}

impl MainWindow {
    /// Sets up the play modes. Called once from `setup`.
    pub(super) fn setup_play_modes(&self, backend: &Backend, playback: &Rc<Playback>) {
        let imp = self.imp();
        let menu: gio::MenuModel = imp.main_menu.get().upcast();
        if let Some(view_menu) = submenu(&menu, &tr("View")) {
            view_menu.append(
                Some(&tr("Follow the playing track")),
                Some("win.follow-playing"),
            );
        }

        let sleep = gio::ActionEntry::builder("sleep-timer")
            .activate(|window: &Self, _, _| {
                window.imp().deck.open_sleep_timer();
            })
            .build();
        let output = gio::ActionEntry::builder("output-switcher")
            .activate(|window: &Self, _, _| {
                window.open_output_switcher();
            })
            .build();
        // Remembered between runs with the window's state.
        let follow_on = backend.ui_state().load().follow_playing;
        let follow = gio::ActionEntry::builder("follow-playing")
            .state(follow_on.to_variant())
            .activate(|window: &Self, action, _| {
                let on = !action
                    .state()
                    .and_then(|s| s.get::<bool>())
                    .unwrap_or(false);
                action.set_state(&on.to_variant());
                window.save_follow_playing(on);
            })
            .build();
        self.add_action_entries([sleep, output, follow]);

        self.setup_queue_cursor(backend, playback);

        // What the Playback menu's labels show. A popover menu never drops
        // the pages of submenus taken out of its model, so the menu is filled
        // while no popover follows it and the popovers then build it anew —
        // not while the main menu is open, but once it has closed.
        let shown = Rc::new(std::cell::Cell::new(None));
        let weak = self.downgrade();
        let at_start = playback.view();
        playback.connect(move |view, change| {
            let Some(window) = weak.upgrade() else { return };
            match change {
                Change::Modes | Change::State => {
                    let key = (
                        view.state == crate::backend::PlaybackState::Playing,
                        view.modes,
                        view.stop_after_mode,
                    );
                    if shown.replace(Some(key)) != Some(key) {
                        let open = window
                            .imp()
                            .menu_button
                            .popover()
                            .is_some_and(|p| p.is_visible());
                        let pending = &window.imp().pending_playback_menu;
                        if open {
                            pending.replace(Some(view.clone()));
                        } else {
                            pending.replace(None);
                            window.refill_playback_menu(view);
                        }
                    }
                    if let Some(application) = window.application() {
                        crate::play_modes::sync_actions(&application, view);
                    }
                }
                Change::Track => {
                    if window.follows_playing() {
                        window.scroll_queue_to_current();
                    }
                    // The panel showed the queue anew.
                    window.select_cursor(view);
                }
                Change::Queue => {
                    window.take_start_selection(view);
                    window.select_cursor(view);
                }
                Change::Cursor => window.select_cursor(view),
                _ => {}
            }
        });
        // The queue the player restored, when it comes.
        self.imp().awaiting_queue.set(true);
        if !at_start.queue.items.is_empty() {
            self.take_start_selection(&at_start);
            self.select_cursor(&at_start);
        }
    }

    /// The first queue shown: its current entry is selected (without making
    /// it the cursor or playing it) and scrolled into view.
    fn take_start_selection(&self, view: &PlayerView) {
        let imp = self.imp();
        if !imp.awaiting_queue.replace(false) || view.cursor.is_some() {
            return;
        }
        let current = current_entry(view);
        imp.start_entry.set(current);
        if current.is_some() {
            self.reveal_queue_current();
        }
    }

    /// Shows in the Playback menu what changed while the main menu was open.
    pub(super) fn main_menu_closed(&self) {
        let view = self.imp().pending_playback_menu.take();
        if let Some(view) = view {
            self.refill_playback_menu(&view);
        }
    }

    /// Fills the Playback menu for `view` and has the main menu and the menu
    /// bar build their popovers anew.
    fn refill_playback_menu(&self, view: &PlayerView) {
        let imp = self.imp();
        let menu: gio::MenuModel = imp.main_menu.get().upcast();
        let Some(playback_menu) = submenu(&menu, &tr("Playback")) else {
            return;
        };
        let popover = imp.menu_button.popover().and_downcast::<gtk::PopoverMenu>();
        if let Some(popover) = &popover {
            popover.set_menu_model(None::<&gio::MenuModel>);
        }
        let bar = imp.menubar.menu_model();
        imp.menubar.set_menu_model(None::<&gio::MenuModel>);
        crate::play_modes::fill_playback_menu(&playback_menu, view);
        if let Some(popover) = &popover {
            popover.set_menu_model(Some(&menu));
            crate::widgets::menu_names::name_menus(popover);
        }
        if bar.is_some() {
            imp.menubar
                .set_menu_model(Some(&super::menubar_model(&menu)));
            crate::widgets::menu_names::name_menus(&*imp.menubar);
        }
    }

    /// Remembers whether "Follow the playing track" is on.
    fn save_follow_playing(&self, on: bool) {
        let Some(backend) = self.imp().backend.borrow().clone() else {
            return;
        };
        let store = backend.ui_state();
        let mut state = store.load();
        state.follow_playing = on;
        if let Err(error) = store.save(&state) {
            eprintln!("helikodu: cannot save the window state: {error}");
        }
    }

    /// Whether "Follow the playing track" is on.
    fn follows_playing(&self) -> bool {
        self.lookup_action("follow-playing")
            .and_then(|a| a.state())
            .and_then(|s| s.get::<bool>())
            .unwrap_or(false)
    }

    /// The queue list sends its selection as the cursor.
    fn setup_queue_cursor(&self, backend: &Backend, playback: &Rc<Playback>) {
        let list = self.imp().panel.queue_list();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_activate_on_single_click(false);
        let b = backend.clone();
        let source = Rc::downgrade(playback);
        let weak = self.downgrade();
        list.connect_row_selected(move |_, row| {
            // Rows go away whenever the queue is shown again; only a choice
            // of the user is a new cursor.
            let Some(window) = weak.upgrade() else { return };
            if window.imp().queue_selecting.get() {
                return;
            }
            let Some(entry) = row.and_then(|r| r.widget_name().parse::<u64>().ok()) else {
                return;
            };
            window.imp().start_entry.set(None);
            if source
                .upgrade()
                .is_some_and(|p| p.view().cursor != Some(entry))
            {
                b.send(PlayerRequest::SetCursor(Some(entry)));
            }
        });
    }

    /// Selects the row of the cursor again after the queue was shown anew.
    fn select_cursor(&self, view: &PlayerView) {
        let list = self.imp().panel.queue_list();
        let start = &self.imp().start_entry;
        if view.cursor.is_some() || start.get() != current_entry(view) {
            start.set(None);
        }
        let wanted = view.cursor.or(start.get());
        let selecting = &self.imp().queue_selecting;
        selecting.set(true);
        let mut child = list.first_child();
        let mut chosen = None;
        while let Some(widget) = child {
            child = widget.next_sibling();
            let Ok(row) = widget.downcast::<gtk::ListBoxRow>() else {
                continue;
            };
            match row.widget_name().parse::<u64>() {
                Ok(entry) => {
                    if Some(entry) == wanted {
                        chosen = Some(row);
                    }
                }
                // Section labels cannot be chosen.
                Err(_) => row.set_selectable(false),
            }
        }
        match chosen {
            Some(row) => list.select_row(Some(&row)),
            None => list.unselect_all(),
        }
        selecting.set(false);
    }

    /// Scrolls the queue panel to the current row once it has been laid out:
    /// at once if it is shown, else when the panel is first shown.
    fn reveal_queue_current(&self) {
        // Frames to wait for the list to be laid out before giving up.
        const TRIES: u32 = 30;
        let list = self.imp().panel.queue_list();
        let tries = std::cell::Cell::new(0);
        list.add_tick_callback(move |list, _| {
            if scroll_to_current(list) || tries.replace(tries.get() + 1) >= TRIES {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// Scrolls the queue panel to the playing row.
    fn scroll_queue_to_current(&self) {
        let list = self.imp().panel.queue_list();
        let weak = list.downgrade();
        // After the panel has shown the new queue.
        glib::idle_add_local_once(move || {
            if let Some(list) = weak.upgrade() {
                scroll_to_current(&list);
            }
        });
    }

    /// The labels of the Playback menu and its submenus.
    #[doc(hidden)]
    pub fn playback_menu_labels(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(menu) = submenu(self.imp().main_menu.upcast_ref(), &tr("Playback")) {
            labels(menu.upcast_ref(), &mut out);
        }
        out
    }

    /// The Playback menu (for tests and screenshots).
    #[doc(hidden)]
    pub fn playback_menu(&self) -> Option<gio::MenuModel> {
        submenu(self.imp().main_menu.upcast_ref(), &tr("Playback")).map(|m| m.upcast())
    }

    /// The deck's shuffle and repeat buttons.
    #[doc(hidden)]
    pub fn deck_mode_buttons(&self) -> (gtk::Button, gtk::Button) {
        self.imp().deck.mode_buttons()
    }

    /// The texts of the deck's loop chip and sleep chip, while shown.
    #[doc(hidden)]
    pub fn deck_chip_texts(&self) -> (Option<String>, Option<String>) {
        self.imp().deck.chip_texts()
    }

    /// Whether the deck's heart shows the playing track as a favourite.
    #[doc(hidden)]
    pub fn deck_favourite(&self) -> bool {
        self.imp().deck.is_favourite_shown()
    }

    /// Clicks the deck's heart.
    #[doc(hidden)]
    pub fn toggle_deck_favourite(&self) {
        self.imp().deck.toggle_favourite();
    }

    /// The playlists in the sidebar's order, with their counts, and whether
    /// each is built in.
    #[doc(hidden)]
    pub fn playlist_counts(&self) -> Vec<(String, usize, bool)> {
        self.playlist_rows()
            .into_iter()
            .map(|p| (p.name, p.items, p.builtin))
            .collect()
    }

    /// Opens the sleep timer popover.
    #[doc(hidden)]
    pub fn open_sleep_timer(&self) -> Option<gtk::Popover> {
        self.imp().deck.open_sleep_timer()
    }

    /// The queue panel's list.
    #[doc(hidden)]
    pub fn queue_list(&self) -> gtk::ListBox {
        self.imp().panel.queue_list()
    }

    /// The entry of "Continue from the selected track", as the player has it.
    #[doc(hidden)]
    pub fn queue_cursor(&self) -> Option<u64> {
        self.imp().playback.borrow().as_ref()?.view().cursor
    }
}

/// The queue entry of `view`'s current track.
fn current_entry(view: &PlayerView) -> Option<u64> {
    view.queue
        .current
        .and_then(|i| view.queue.items.get(i))
        .and_then(|t| t.entry)
}

/// Scrolls `list` so that its current row shows a third of the way down;
/// whether that row has been laid out.
fn scroll_to_current(list: &gtk::ListBox) -> bool {
    let mut child = list.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if widget.has_css_class("current") {
            let Some(bounds) = widget.compute_bounds(list) else {
                return false;
            };
            let Some(adjustment) = list
                .ancestor(gtk::ScrolledWindow::static_type())
                .and_downcast::<gtk::ScrolledWindow>()
                .map(|s| s.vadjustment())
            else {
                return false;
            };
            if bounds.height() <= 0.0 || adjustment.page_size() <= 0.0 {
                return false;
            }
            let top = f64::from(bounds.y()) - adjustment.page_size() / 3.0;
            adjustment.set_value(top.max(0.0));
            return true;
        }
    }
    false
}
