// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The drag handle on the border of a pane. It lies over the pane's 1 px border
//! as an overlay child, so it takes no room in the layout, and changes the
//! pane's width within its limits.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::i18n::tr;

const GRIP: i32 = 7;

type Changed = Box<dyn Fn(i32)>;

/// Which side of the window the pane is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Start,
    End,
}

/// A resizable pane: its width and the handle that changes it.
pub(crate) struct PaneHandle {
    pub(crate) handle: gtk::Box,
    pane: gtk::Widget,
    side: Side,
    width: Cell<i32>,
    min: i32,
    max: i32,
    changed: std::cell::RefCell<Option<Changed>>,
}

impl std::fmt::Debug for PaneHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneHandle")
            .field("side", &self.side)
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl PaneHandle {
    /// A handle for `pane`, `width` wide by default, added to `overlay`.
    pub(crate) fn new(
        overlay: &gtk::Overlay,
        pane: &impl IsA<gtk::Widget>,
        side: Side,
        width: i32,
        (min, max): (i32, i32),
    ) -> Rc<Self> {
        let handle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        handle.add_css_class("hk-pane-handle");
        handle.set_size_request(GRIP, -1);
        handle.set_valign(gtk::Align::Fill);
        handle.set_halign(match side {
            Side::Start => gtk::Align::Start,
            Side::End => gtk::Align::End,
        });
        handle.set_cursor_from_name(Some("col-resize"));
        handle.set_tooltip_text(Some(&tr("Drag to change the width")));
        overlay.add_overlay(&handle);
        let me = Rc::new(Self {
            handle,
            pane: pane.clone().upcast(),
            side,
            width: Cell::new(width),
            min,
            max,
            changed: std::cell::RefCell::new(None),
        });
        me.set_width(width);
        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new(width));
        let weak = Rc::downgrade(&me);
        let s = start.clone();
        drag.connect_drag_begin(move |_, _, _| {
            if let Some(me) = weak.upgrade() {
                s.set(me.width.get());
            }
        });
        let weak = Rc::downgrade(&me);
        drag.connect_drag_update(move |_, dx, _| {
            if let Some(me) = weak.upgrade() {
                let dx = dx.round() as i32;
                let delta = match me.side {
                    Side::Start => dx,
                    Side::End => -dx,
                };
                me.set_width(start.get() + delta);
            }
        });
        let weak = Rc::downgrade(&me);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(me) = weak.upgrade()
                && let Some(changed) = me.changed.borrow().as_ref()
            {
                changed(me.width.get());
            }
        });
        me.handle.add_controller(drag);
        me
    }

    /// Calls `changed` with the new width after the user drags the handle.
    pub(crate) fn connect_changed(&self, changed: impl Fn(i32) + 'static) {
        self.changed.replace(Some(Box::new(changed)));
    }

    /// The pane's width.
    pub(crate) fn width(&self) -> i32 {
        self.width.get()
    }

    /// Sets the pane's width within its limits.
    pub(crate) fn set_width(&self, width: i32) {
        let width = width.clamp(self.min, self.max);
        self.width.set(width);
        self.pane.set_size_request(width, -1);
        let margin = (width - (GRIP + 1) / 2).max(0);
        match self.side {
            Side::Start => self.handle.set_margin_start(margin),
            Side::End => self.handle.set_margin_end(margin),
        }
    }

    /// Shows the handle only while the pane is shown.
    pub(crate) fn set_shown(&self, shown: bool) {
        self.handle.set_visible(shown);
    }
}
