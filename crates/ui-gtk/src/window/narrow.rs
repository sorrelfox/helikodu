// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The narrow window: the breakpoints and what changes at them.
//!
//! - When the content would be narrower than [`MIN_CONTENT`] with the queue
//!   panel beside it, the panel leaves the row and opens over the content from
//!   the right; the header's queue button opens it. A hidden sidebar leaves its
//!   width to the content, so the panel stays beside it longer.
//! - When the content would be narrower than that even without the panel, the
//!   sidebar does the same from the left; a sidebar button starts the header and
//!   the forward button is left out.
//! - Below [`COMPACT_DECK`] the deck is compact.
//!
//! An open pane is drawn over the content with a shadow, and black at 50 %
//! covers the rest; a click beside it, Esc, its key (F9 for the sidebar) or
//! choosing a view closes it. The deck stays in use.
//!
//! The width is followed through an empty drawing area over the panes, whose
//! `resize` signal comes only when the size changes; nothing runs while the
//! window keeps its size. Showing or hiding the sidebar lays the panes out
//! again too.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::MainWindow;
use crate::i18n::tr;

/// The least width the content keeps beside the panes.
pub(crate) const MIN_CONTENT: i32 = 560;
/// Below this window width the deck is compact.
pub(crate) const COMPACT_DECK: i32 = 720;
/// The narrowest the window can be made.
const MIN_WINDOW: i32 = 400;

/// Which panes are drawn over the content, and whether the deck is compact,
/// at a window `width` with the given pane widths. A sidebar that is hidden
/// counts as 0 wide for the panel; whether the sidebar itself goes over the
/// content is decided with its own width, so that showing it in a narrow
/// window opens it over the content.
pub(crate) fn layout(width: i32, sidebar: i32, panel: i32) -> (bool, bool, bool) {
    let sidebar_over = width - sidebar < MIN_CONTENT;
    let beside = if sidebar_over { 0 } else { sidebar };
    let panel_over = width - beside - panel < MIN_CONTENT;
    (sidebar_over, panel_over, width < COMPACT_DECK)
}

/// The state of the narrow layout.
#[derive(Debug)]
pub(crate) struct Narrow {
    /// The layer behind an open pane.
    scrim: gtk::Box,
    /// The header's sidebar button, shown while the sidebar is over the content.
    sidebar_button: gtk::Button,
    sidebar_over: Cell<bool>,
    panel_over: Cell<bool>,
    compact: Cell<bool>,
    /// Whether the sidebar was shown before it went over the content.
    sidebar_was_shown: Cell<bool>,
    /// Whether the panel was wanted before it went over the content.
    panel_was_wanted: Cell<bool>,
    /// The last width seen, and whether a layout pass is waiting.
    width: Cell<i32>,
    pending: Cell<bool>,
    /// Whether a layout pass is running (moving a pane shows or hides it).
    busy: Cell<bool>,
}

impl MainWindow {
    pub(super) fn setup_narrow(&self) {
        let imp = self.imp();
        self.set_size_request(MIN_WINDOW, -1);
        let overlay = imp.panes_overlay.get();
        let scrim = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        scrim.add_css_class("hk-scrim");
        scrim.set_visible(false);
        overlay.add_overlay(&scrim);
        let sidebar_button = gtk::Button::from_icon_name("helikodu-sidebar-symbolic");
        sidebar_button.add_css_class("hk-icon");
        sidebar_button.set_valign(gtk::Align::Center);
        sidebar_button.set_action_name(Some("win.toggle-sidebar"));
        sidebar_button.set_tooltip_text(Some(&tr("Show the sidebar")));
        sidebar_button
            .update_property(&[gtk::accessible::Property::Label(&tr("Show the sidebar"))]);
        sidebar_button.set_visible(false);
        if let Some(start) = imp.back.parent().and_downcast::<gtk::Box>() {
            start.prepend(&sidebar_button);
        }
        let narrow = Rc::new(Narrow {
            scrim: scrim.clone(),
            sidebar_button,
            sidebar_over: Cell::new(false),
            panel_over: Cell::new(false),
            compact: Cell::new(false),
            sidebar_was_shown: Cell::new(true),
            panel_was_wanted: Cell::new(true),
            width: Cell::new(0),
            pending: Cell::new(false),
            busy: Cell::new(false),
        });
        imp.narrow.replace(Some(narrow.clone()));

        // A click beside the open pane closes it.
        let click = gtk::GestureClick::new();
        let weak = self.downgrade();
        click.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(window) = weak.upgrade() {
                window.close_overlays();
            }
        });
        scrim.add_controller(click);
        // So does Esc.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(window) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if key == gtk::gdk::Key::Escape && window.overlay_open() {
                window.close_overlays();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.add_controller(keys);
        // And choosing a view.
        let weak = self.downgrade();
        imp.views.connect_visible_child_notify(move |_| {
            if let Some(window) = weak.upgrade() {
                window.close_overlays();
            }
        });
        for revealer in [imp.sidebar_revealer.get(), imp.panel_revealer.get()] {
            let weak = self.downgrade();
            revealer.connect_reveal_child_notify(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.update_scrim();
                }
            });
        }
        // Showing or hiding the sidebar in the row changes the room for the
        // panel: at once, so that the content never has less than its room.
        let weak = self.downgrade();
        imp.sidebar_revealer.connect_reveal_child_notify(move |_| {
            if let Some(window) = weak.upgrade() {
                window.apply_narrow();
            }
        });

        // The window state keeps the panes as they are in a wide window, not
        // whether one is open over the content now. (The window saves its
        // state first; this runs after it.)
        self.connect_close_request(|window| {
            window.keep_wide_panes();
            glib::Propagation::Proceed
        });

        // The width, followed without drawing anything.
        let sensor = gtk::DrawingArea::new();
        sensor.set_can_target(false);
        sensor.set_can_focus(false);
        sensor.set_accessible_role(gtk::AccessibleRole::Presentation);
        overlay.add_overlay(&sensor);
        overlay.set_measure_overlay(&sensor, false);
        let weak = self.downgrade();
        sensor.connect_resize(move |_, width, _| {
            if let Some(window) = weak.upgrade() {
                window.width_changed(width);
            }
        });
        // The size asked for, before the panes' widths hold the window wider
        // than that.
        self.connect_default_width_notify(|window| {
            let (width, _) = window.default_size();
            if width > 0 && !window.is_maximized() && !window.is_fullscreen() {
                window.width_changed(width);
            }
        });
        let (width, _) = self.default_size();
        if width > 0 {
            self.width_changed(width);
        }
    }

    /// Lays the panes out for `width` once the current allocation is done.
    fn width_changed(&self, width: i32) {
        let Some(narrow) = self.narrow() else { return };
        narrow.width.set(width);
        // Before the window is shown, at once: its minimum width must already
        // fit the size asked for when it is first laid out.
        if !self.is_mapped() {
            self.apply_narrow();
            return;
        }
        if !narrow.pending.replace(true) {
            let weak = self.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(window) = weak.upgrade() {
                    if let Some(narrow) = window.narrow() {
                        narrow.pending.set(false);
                    }
                    window.apply_narrow();
                }
            });
        }
    }

    /// Writes the wide layout's panes into the saved window state while a pane
    /// is over the content.
    fn keep_wide_panes(&self) {
        let Some(narrow) = self.narrow() else { return };
        if !narrow.sidebar_over.get() && !narrow.panel_over.get() {
            return;
        }
        let Some(backend) = self.imp().backend.borrow().clone() else {
            return;
        };
        let store = backend.ui_state();
        let mut state = store.load();
        if narrow.sidebar_over.get() {
            state.panes.sidebar_visible = narrow.sidebar_was_shown.get();
        }
        if narrow.panel_over.get() {
            state.panes.queue_visible = narrow.panel_was_wanted.get();
        }
        if let Err(error) = store.save(&state) {
            log::warn!("cannot save the window state: {error}");
        }
    }

    fn narrow(&self) -> Option<Rc<Narrow>> {
        self.imp().narrow.borrow().clone()
    }

    /// Moves the panes in or out of the row for the width last seen.
    fn apply_narrow(&self) {
        let Some(narrow) = self.narrow() else { return };
        let width = narrow.width.get();
        if width <= 0 || narrow.busy.replace(true) {
            return;
        }
        self.apply_narrow_now(&narrow, width);
        narrow.busy.set(false);
    }

    fn apply_narrow_now(&self, narrow: &Narrow, width: i32) {
        let (sidebar, panel) = {
            let handles = self.imp().handles.borrow();
            (
                handles.first().map_or(228, |h| h.width()),
                handles.get(1).map_or(320, |h| h.width()),
            )
        };
        let (sidebar_over, _, compact) = layout(width, sidebar, panel);
        if narrow.sidebar_over.get() != sidebar_over {
            self.set_sidebar_over(narrow, sidebar_over);
        }
        // The panel has the room the sidebar leaves in the row.
        let in_row = !sidebar_over && self.imp().sidebar_revealer.reveals_child();
        let (_, panel_over, _) = layout(width, if in_row { sidebar } else { 0 }, panel);
        if narrow.panel_over.get() != panel_over {
            self.set_panel_over(narrow, panel_over);
        }
        if narrow.compact.replace(compact) != compact {
            self.imp().deck.set_compact(compact);
            self.set_narrow_search(compact);
            if let Some(pages) = self.pages() {
                pages.set_narrow(compact);
            }
        }
        self.update_scrim();
    }

    fn set_sidebar_over(&self, narrow: &Narrow, over: bool) {
        let imp = self.imp();
        let revealer = imp.sidebar_revealer.get();
        let overlay = imp.panes_overlay.get();
        narrow.sidebar_over.set(over);
        let sidebar = imp.sidebar.borrow().as_ref().map(|s| s.root.clone());
        if over {
            narrow.sidebar_was_shown.set(revealer.reveals_child());
            let Some(panes) = revealer.parent().and_downcast::<gtk::Box>() else {
                return;
            };
            revealer.set_reveal_child(false);
            panes.remove(&revealer);
            revealer.set_halign(gtk::Align::Start);
            overlay.add_overlay(&revealer);
            if let Some(sidebar) = &sidebar {
                sidebar.add_css_class("hk-overlay-pane");
            }
        } else {
            let Some(panes) = overlay.child().and_downcast::<gtk::Box>() else {
                return;
            };
            revealer.set_reveal_child(false);
            overlay.remove_overlay(&revealer);
            revealer.set_halign(gtk::Align::Fill);
            panes.prepend(&revealer);
            revealer.set_reveal_child(narrow.sidebar_was_shown.get());
            if let Some(sidebar) = &sidebar {
                sidebar.remove_css_class("hk-overlay-pane");
            }
        }
        narrow.sidebar_button.set_visible(over);
        imp.forward.set_visible(!over);
        self.update_header();
    }

    fn set_panel_over(&self, narrow: &Narrow, over: bool) {
        let imp = self.imp();
        let revealer = imp.panel_revealer.get();
        let overlay = imp.panes_overlay.get();
        narrow.panel_over.set(over);
        if over {
            narrow.panel_was_wanted.set(imp.panel_wanted.get());
            let Some(panes) = revealer.parent().and_downcast::<gtk::Box>() else {
                return;
            };
            revealer.set_reveal_child(false);
            imp.panel_wanted.set(false);
            panes.remove(&revealer);
            revealer.set_halign(gtk::Align::End);
            overlay.add_overlay(&revealer);
            imp.panel.add_css_class("hk-overlay-pane");
            imp.panel.add_css_class("end");
        } else {
            let Some(panes) = overlay.child().and_downcast::<gtk::Box>() else {
                return;
            };
            revealer.set_reveal_child(false);
            overlay.remove_overlay(&revealer);
            revealer.set_halign(gtk::Align::Fill);
            panes.append(&revealer);
            imp.panel.remove_css_class("hk-overlay-pane");
            imp.panel.remove_css_class("end");
            imp.panel_wanted.set(narrow.panel_was_wanted.get());
            if let Some(page) = self.current_page() {
                self.show_panel_for(page);
            }
        }
        self.update_header();
    }

    /// The search entry's shorter placeholder in the narrow header, without the
    /// key hint beside it.
    fn set_narrow_search(&self, narrow: bool) {
        let center = self
            .imp()
            .back
            .parent()
            .and_then(|b| b.parent())
            .and_downcast::<gtk::CenterBox>()
            .and_then(|c| c.center_widget());
        let (entry, hint) = match center.clone().and_downcast::<gtk::Overlay>() {
            Some(overlay) => (
                overlay.child().and_downcast::<gtk::SearchEntry>(),
                overlay.last_child().filter(|w| w.has_css_class("hint")),
            ),
            None => (center.and_downcast::<gtk::SearchEntry>(), None),
        };
        let Some(entry) = entry else { return };
        if narrow {
            entry.set_placeholder_text(Some(&tr("Search")));
            entry.set_width_chars(8);
            entry.set_margin_start(8);
            entry.set_margin_end(8);
        } else {
            entry.set_placeholder_text(Some(&tr("Search artists, albums, tracks")));
            entry.set_width_chars(20);
            entry.set_margin_start(16);
            entry.set_margin_end(16);
        }
        if let Some(hint) = hint {
            hint.set_opacity(if narrow { 0.0 } else { 1.0 });
        }
    }

    /// The placeholder of the header's search entry (for tests).
    #[doc(hidden)]
    pub fn search_placeholder(&self) -> String {
        let center = self
            .imp()
            .back
            .parent()
            .and_then(|b| b.parent())
            .and_downcast::<gtk::CenterBox>()
            .and_then(|c| c.center_widget());
        let entry = match center.clone().and_downcast::<gtk::Overlay>() {
            Some(overlay) => overlay.child().and_downcast::<gtk::SearchEntry>(),
            None => center.and_downcast::<gtk::SearchEntry>(),
        };
        entry
            .and_then(|e| e.placeholder_text())
            .map(|t| t.to_string())
            .unwrap_or_default()
    }

    /// Whether a pane is open over the content.
    fn overlay_open(&self) -> bool {
        let Some(narrow) = self.narrow() else {
            return false;
        };
        let imp = self.imp();
        (narrow.sidebar_over.get() && imp.sidebar_revealer.reveals_child())
            || (narrow.panel_over.get() && imp.panel_revealer.reveals_child())
    }

    /// Closes the panes open over the content.
    fn close_overlays(&self) {
        let Some(narrow) = self.narrow() else { return };
        let imp = self.imp();
        if narrow.sidebar_over.get() && imp.sidebar_revealer.reveals_child() {
            imp.sidebar_revealer.set_reveal_child(false);
        }
        if narrow.panel_over.get() && imp.panel_revealer.reveals_child() {
            imp.panel_revealer.set_reveal_child(false);
            imp.panel_wanted.set(false);
        }
        self.update_header();
    }

    fn update_scrim(&self) {
        if let Some(narrow) = self.narrow() {
            narrow.scrim.set_visible(self.overlay_open());
        }
    }

    /// Which panes are over the content, and whether the deck is compact (for
    /// tests).
    #[doc(hidden)]
    pub fn narrow_state(&self) -> (bool, bool, bool) {
        self.narrow().map_or((false, false, false), |n| {
            (n.sidebar_over.get(), n.panel_over.get(), n.compact.get())
        })
    }

    /// Whether a pane is open over the content and the layer behind it is
    /// shown (for tests).
    #[doc(hidden)]
    pub fn narrow_overlay_open(&self) -> (bool, bool) {
        let shown = self.narrow().is_some_and(|n| n.scrim.is_visible());
        (self.overlay_open(), shown)
    }

    /// The rate and bits the compact deck's output button shows (for tests).
    #[doc(hidden)]
    pub fn compact_output_text(&self) -> Option<String> {
        self.imp().deck.compact_output_text()
    }

    /// Whether the header shows the sidebar button and the forward button
    /// (for tests).
    #[doc(hidden)]
    pub fn narrow_header(&self) -> (bool, bool) {
        let sidebar = self.narrow().is_some_and(|n| n.sidebar_button.is_visible());
        (sidebar, self.imp().forward.is_visible())
    }

    /// Closes the panes open over the content, as a click beside them does
    /// (for tests).
    #[doc(hidden)]
    pub fn click_beside_overlay(&self) {
        self.close_overlays();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoints_follow_the_content_width() {
        // The design's default widths: the sidebar 228, the panel 320.
        assert_eq!(layout(1440, 228, 320), (false, false, false));
        // About 1100 px: the panel goes over the content first.
        assert_eq!(layout(1108, 228, 320), (false, false, false));
        assert_eq!(layout(1100, 228, 320), (false, true, false));
        // About 800 px: then the sidebar.
        assert_eq!(layout(790, 228, 320), (false, true, false));
        assert_eq!(layout(780, 228, 320), (true, true, false));
        // The compact deck below 720 px.
        assert_eq!(layout(640, 228, 320), (true, true, true));
        // A narrower panel stays in the row longer.
        assert_eq!(layout(1100, 228, 260), (false, false, false));
    }

    #[test]
    fn a_hidden_sidebar_leaves_its_room_to_the_panel() {
        // A hidden sidebar counts as 0 wide for the panel: at 1000 px the
        // panel stays beside the content (1000 - 320 = 680).
        assert_eq!(layout(1000, 228, 320), (false, true, false));
        assert_eq!(layout(1000, 0, 320), (false, false, false));
        // Down to the content's least width beside the panel alone.
        assert_eq!(layout(880, 0, 320), (false, false, false));
        assert_eq!(layout(879, 0, 320), (false, true, false));
        // The sidebar goes over the content by its own width, hidden or not
        // (`apply_narrow` asks with its width for that); then the panel has
        // the whole window.
        assert_eq!(layout(780, 228, 320), (true, true, false));
        assert_eq!(layout(780, 0, 320), (false, true, false));
        // The compact deck does not depend on the panes.
        assert_eq!(layout(700, 0, 320), (false, true, true));
    }
}
