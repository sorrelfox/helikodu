// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Toasts: one short message at a time, at the bottom centre of the content
//! area, 18 px above the deck. Dark in both schemes. A toast goes after five
//! seconds (eight with an Undo action); hovering it or focusing it stops the
//! clock. A new toast replaces the old one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::i18n::tr;
use crate::widgets::{icon_button, label};

/// What a toast's button does.
pub(crate) struct ToastAction {
    pub(crate) label: String,
    pub(crate) undo: bool,
    pub(crate) run: Box<dyn Fn()>,
}

/// The toast area over the content.
pub(crate) struct ToastOverlay {
    host: gtk::Box,
    current: RefCell<Option<gtk::Box>>,
    generation: Cell<u64>,
    held: Cell<bool>,
}

impl std::fmt::Debug for ToastOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToastOverlay").finish_non_exhaustive()
    }
}

impl ToastOverlay {
    /// A toast area to add as an overlay child of `overlay`.
    pub(crate) fn new(overlay: &gtk::Overlay) -> Rc<Self> {
        let host = gtk::Box::new(gtk::Orientation::Vertical, 0);
        host.set_halign(gtk::Align::Center);
        host.set_valign(gtk::Align::End);
        host.set_margin_bottom(18);
        host.set_margin_start(24);
        host.set_margin_end(24);
        overlay.add_overlay(&host);
        Rc::new(Self {
            host,
            current: RefCell::new(None),
            generation: Cell::new(0),
            held: Cell::new(false),
        })
    }

    /// Shows `text`, with an optional action; `error` announces it assertively.
    pub(crate) fn show(self: &Rc<Self>, text: &str, action: Option<ToastAction>, error: bool) {
        self.dismiss();
        let toast = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toast.add_css_class("hk-toast");
        toast.add_css_class("hk-glass");
        let message = label(text, &[]);
        message.set_hexpand(true);
        message.set_valign(gtk::Align::Center);
        message.set_max_width_chars(70);
        toast.append(&message);
        let seconds = if action.as_ref().is_some_and(|a| a.undo) {
            8
        } else {
            5
        };
        if let Some(action) = action {
            let button = gtk::Button::with_label(&action.label);
            button.add_css_class("action");
            button.set_valign(gtk::Align::Center);
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                (action.run)();
                if let Some(me) = weak.upgrade() {
                    me.dismiss();
                }
            });
            toast.append(&button);
        }
        let close = icon_button("window-close-symbolic", &tr("Dismiss"));
        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.dismiss();
            }
        });
        toast.append(&close);

        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_enter(move |_, _, _| {
            if let Some(me) = weak.upgrade() {
                me.held.set(true);
            }
        });
        let weak = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let Some(me) = weak.upgrade() {
                me.held.set(false);
            }
        });
        toast.add_controller(motion);

        self.host.append(&toast);
        let priority = if error {
            gtk::AccessibleAnnouncementPriority::High
        } else {
            gtk::AccessibleAnnouncementPriority::Medium
        };
        toast.announce(text, priority);
        self.current.replace(Some(toast));
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.held.set(false);
        self.countdown(generation, seconds);
    }

    fn countdown(self: &Rc<Self>, generation: u64, seconds: u32) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(u64::from(seconds)), move || {
            let Some(me) = weak.upgrade() else { return };
            if me.generation.get() != generation {
                return;
            }
            let focused = me
                .current
                .borrow()
                .as_ref()
                .is_some_and(|t| t.has_focus() || t.focus_child().is_some());
            if me.held.get() || focused {
                me.countdown(generation, 2);
            } else {
                me.dismiss();
            }
        });
    }

    /// Removes the toast.
    pub(crate) fn dismiss(&self) {
        if let Some(toast) = self.current.take() {
            self.host.remove(&toast);
        }
        self.generation.set(self.generation.get() + 1);
    }

    /// The text of the toast shown (for tests).
    pub(crate) fn text(&self) -> Option<String> {
        let toast = self.current.borrow();
        let first = toast.as_ref()?.first_child()?;
        first
            .downcast_ref::<gtk::Label>()
            .map(|l| l.text().to_string())
    }
}
