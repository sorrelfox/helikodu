// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the tables share: a cell that knows the row it shows, so a click can
//! find it, and paints the zebra stripe from the item position (a CSS
//! `:nth-child` rule would shift as rows are recycled).

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::models::TrackItem;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct TableCell {
        pub(super) position: Cell<Option<u32>>,
        pub(super) item: RefCell<Option<TrackItem>>,
        pub(super) loaded: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TableCell {
        const NAME: &'static str = "HkTableCell";
        type Type = super::TableCell;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for TableCell {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().add_css_class("hk-cell");
        }
    }

    impl WidgetImpl for TableCell {}
    impl BoxImpl for TableCell {}
}

glib::wrapper! {
    /// One cell of a table row.
    pub(crate) struct TableCell(ObjectSubclass<imp::TableCell>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl TableCell {
    /// A cell holding `child`.
    pub(crate) fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let cell: Self = glib::Object::new();
        child.set_hexpand(true);
        cell.append(child);
        cell
    }

    /// The child widget.
    pub(crate) fn child(&self) -> Option<gtk::Widget> {
        self.first_child()
    }

    /// Shows the row at `position`; odd positions get the stripe.
    pub(crate) fn set_position(&self, position: Option<u32>) {
        self.imp().position.set(position);
        let odd = position.is_some_and(|p| p % 2 == 1);
        if odd {
            self.add_css_class("hk-alt");
        } else {
            self.remove_css_class("hk-alt");
        }
    }

    /// The position of the row shown.
    pub(crate) fn position(&self) -> Option<u32> {
        self.imp().position.get()
    }

    /// Follows `item`: `show` runs now and again when its row arrives.
    pub(crate) fn follow(&self, item: &TrackItem, show: impl Fn(&Self, &TrackItem) + 'static) {
        self.forget();
        show(self, item);
        let weak = self.downgrade();
        let id = item.connect_loaded(move |item| {
            if let Some(cell) = weak.upgrade() {
                show(&cell, item);
            }
        });
        self.imp().item.replace(Some(item.clone()));
        self.imp().loaded.replace(Some(id));
    }

    /// The item shown.
    pub(crate) fn item(&self) -> Option<TrackItem> {
        self.imp().item.borrow().clone()
    }

    /// Stops following the item, when the cell is recycled.
    pub(crate) fn forget(&self) {
        let imp = self.imp();
        if let (Some(item), Some(id)) = (imp.item.take(), imp.loaded.take()) {
            item.disconnect(id);
        }
    }
}

/// The table cell under the point `(x, y)` of `widget`.
pub(crate) fn cell_at(widget: &impl IsA<gtk::Widget>, x: f64, y: f64) -> Option<TableCell> {
    let mut picked = widget.pick(x, y, gtk::PickFlags::DEFAULT);
    while let Some(w) = picked {
        if let Some(cell) = w.downcast_ref::<TableCell>() {
            return Some(cell.clone());
        }
        // A cell's own children end with the cell; past the widget, give up.
        if w == *widget.as_ref() {
            return None;
        }
        picked = w.parent();
    }
    None
}

/// The positions selected in `model`, in order.
pub(crate) fn selected_positions(model: &impl IsA<gtk::SelectionModel>) -> Vec<u32> {
    let set = model.selection();
    let mut positions = Vec::new();
    if let Some((iter, first)) = gtk::BitsetIter::init_first(&set) {
        positions.push(first);
        positions.extend(iter);
    }
    positions
}
