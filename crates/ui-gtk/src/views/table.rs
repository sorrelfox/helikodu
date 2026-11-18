// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the tables share: a cell that knows the row it shows, so a click can
//! find it, and paints the zebra stripe from the item position (a CSS
//! `:nth-child` rule would shift as rows are recycled).
//!
//! A cell is one widget: it draws its text itself, or holds one child (the
//! rating stars, the playing indicator, a playlist line). A table keeps cells
//! for a couple of hundred rows, so a label in a box per cell would double the
//! widgets of the table.

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::pango;
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
        pub(super) text: RefCell<String>,
        pub(super) xalign: Cell<f32>,
        /// OpenType features, as the CSS `font-feature-settings` of the
        /// classes would give a label.
        pub(super) features: Cell<Option<&'static str>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TableCell {
        const NAME: &'static str = "HkTableCell";
        type Type = super::TableCell;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for TableCell {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().add_css_class("hk-cell");
        }

        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for TableCell {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let obj = self.obj();
            let (mut min, mut nat) = (0, 0);
            if let Some(child) = obj.first_child() {
                let (m, n, _, _) = child.measure(orientation, for_size);
                min = m;
                nat = n;
            }
            if !self.text.borrow().is_empty() {
                let (width, height) = self.layout().pixel_size();
                match orientation {
                    // Ellipsized: no minimum width.
                    gtk::Orientation::Horizontal => nat = nat.max(width),
                    _ => {
                        min = min.max(height);
                        nat = nat.max(height);
                    }
                }
            }
            (min, nat, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                child.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            if !self.text.borrow().is_empty() {
                let layout = self.layout();
                let width = obj.width();
                layout.set_width(width * pango::SCALE);
                layout.set_ellipsize(pango::EllipsizeMode::End);
                let (text_width, text_height) = layout.pixel_size();
                // As a label's: mirrored right to left.
                let xalign = if obj.direction() == gtk::TextDirection::Rtl {
                    1.0 - self.xalign.get()
                } else {
                    self.xalign.get()
                };
                let x = ((width - text_width).max(0) as f32 * xalign).round();
                let y = ((obj.height() - text_height) / 2) as f32;
                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new(x, y));
                snapshot.append_layout(&layout, &obj.color());
                snapshot.restore();
            }
            if let Some(child) = obj.first_child() {
                obj.snapshot_child(&child, snapshot);
            }
        }
    }

    impl TableCell {
        /// A layout of the text in the cell's font; made when needed, not
        /// kept, as the cell's render node keeps the glyphs.
        fn layout(&self) -> pango::Layout {
            let layout = self.obj().create_pango_layout(Some(&self.text.borrow()));
            layout.set_single_paragraph_mode(true);
            if let Some(features) = self.features.get() {
                let attrs = pango::AttrList::new();
                attrs.insert(pango::AttrFontFeatures::new(features));
                layout.set_attributes(Some(&attrs));
            }
            layout
        }
    }
}

glib::wrapper! {
    /// One cell of a table row.
    pub(crate) struct TableCell(ObjectSubclass<imp::TableCell>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TableCell {
    /// A cell holding `child`.
    pub(crate) fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let cell: Self = glib::Object::new();
        child.set_hexpand(true);
        child.set_parent(&cell);
        cell
    }

    /// A cell that draws a line of text, aligned by `xalign`, with the style
    /// `classes`. Its accessible name is the text.
    pub(crate) fn text(xalign: f32, classes: &[&str]) -> Self {
        let cell: Self = glib::Object::builder()
            .property("accessible-role", gtk::AccessibleRole::Label)
            .build();
        cell.imp().xalign.set(xalign);
        for class in classes {
            cell.add_css_class(class);
        }
        // Tabular figures, as `.hk-numeric` and `.hk-mono-meta` ask of labels.
        let features = if classes.contains(&"hk-mono-meta") {
            Some("\"tnum\" 1, \"liga\" 0, \"calt\" 0")
        } else if classes.contains(&"hk-numeric") {
            Some("\"tnum\" 1")
        } else {
            None
        };
        cell.imp().features.set(features);
        cell
    }

    /// Shows `text`.
    pub(crate) fn set_text(&self, text: &str) {
        let imp = self.imp();
        if *imp.text.borrow() == text {
            return;
        }
        let was_empty = imp.text.borrow().is_empty();
        text.clone_into(&mut imp.text.borrow_mut());
        self.update_property(&[gtk::accessible::Property::Label(text)]);
        if was_empty != text.is_empty() {
            self.queue_resize();
        } else {
            self.queue_draw();
        }
    }

    /// The child widget.
    pub(crate) fn child(&self) -> Option<gtk::Widget> {
        self.first_child()
    }

    /// Puts `child` in the cell, replacing the one before.
    pub(crate) fn set_child(&self, child: &impl IsA<gtk::Widget>) {
        if let Some(old) = self.first_child() {
            old.unparent();
        }
        child.set_parent(self);
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
