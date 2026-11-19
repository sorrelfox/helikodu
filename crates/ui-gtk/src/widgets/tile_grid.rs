// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Children of equal width side by side, as many as fit; when the row is too
//! narrow for all of them, they continue on further rows, as evenly as they
//! can (five tiles become three and two, not four and one). A column is never
//! narrower than the widest child's minimum width, or than the least column
//! width given, so a child is not cut off. As wide as there is room for, it is
//! one row of equal columns, as a homogeneous box draws it.

use std::cell::Cell;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// How many columns `count` children of at least `least` wide take in
/// `width`, with `gap` between columns: as many as fit, then spread evenly
/// over the rows they need.
pub(crate) fn columns_for(count: i32, least: i32, gap: i32, width: i32) -> i32 {
    if count <= 0 {
        return 1;
    }
    let fit = ((width + gap) / (least + gap).max(1)).clamp(1, count);
    let rows = (count + fit - 1) / fit;
    (count + rows - 1) / rows
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct TileGrid {
        /// The gap between columns.
        pub(super) spacing: Cell<i32>,
        /// The gap between rows.
        pub(super) row_spacing: Cell<i32>,
        /// The least width of a column.
        pub(super) least: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TileGrid {
        const NAME: &'static str = "HkTileGrid";
        type Type = super::TileGrid;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for TileGrid {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for TileGrid {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let children = self.children();
            let count = children.len() as i32;
            let gap = self.spacing.get();
            if orientation == gtk::Orientation::Horizontal {
                // At least one column; at best all in one row of equal columns.
                let least = self.least_column(&children);
                let widest = children
                    .iter()
                    .map(|c| c.measure(gtk::Orientation::Horizontal, -1).1)
                    .max()
                    .unwrap_or(0)
                    .max(least);
                let nat = widest * count + gap * (count - 1).max(0);
                return (least, nat.max(least), -1, -1);
            }
            let width = if for_size < 0 { i32::MAX / 2 } else { for_size };
            let (columns, column) = self.columns(&children, width);
            let rows = self.row_heights(&children, columns, column);
            let total =
                rows.iter().sum::<i32>() + self.row_spacing.get() * (rows.len() as i32 - 1).max(0);
            (total, total, -1, -1)
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            let children = self.children();
            let (columns, column) = self.columns(&children, width);
            let rows = self.row_heights(&children, columns, column);
            let gap = self.spacing.get();
            let rtl = self.obj().direction() == gtk::TextDirection::Rtl;
            let mut y = 0;
            for (n, row) in children.chunks(columns.max(1) as usize).enumerate() {
                let height = rows.get(n).copied().unwrap_or(0);
                for (i, child) in row.iter().enumerate() {
                    let x = i as i32 * (column + gap);
                    let left = if rtl { width - x - column } else { x };
                    child.size_allocate(&gtk::Allocation::new(left, y, column, height), -1);
                }
                y += height + self.row_spacing.get();
            }
        }
    }

    impl TileGrid {
        /// The visible children.
        pub(super) fn children(&self) -> Vec<gtk::Widget> {
            let mut out = Vec::new();
            let mut child = self.obj().first_child();
            while let Some(c) = child {
                if c.should_layout() {
                    out.push(c.clone());
                }
                child = c.next_sibling();
            }
            out
        }

        /// The least width of a column: the widest child's minimum.
        fn least_column(&self, children: &[gtk::Widget]) -> i32 {
            children
                .iter()
                .map(|c| c.measure(gtk::Orientation::Horizontal, -1).0)
                .max()
                .unwrap_or(0)
                .max(self.least.get())
        }

        /// The number of columns in `width` and the width of each.
        fn columns(&self, children: &[gtk::Widget], width: i32) -> (i32, i32) {
            let gap = self.spacing.get();
            let least = self.least_column(children);
            let count = children.len() as i32;
            let columns = columns_for(count, least, gap, width);
            let column = ((width - gap * (columns - 1)) / columns.max(1)).max(least);
            (columns, column)
        }

        /// The height of each row, `columns` children of `column` wide a row.
        fn row_heights(&self, children: &[gtk::Widget], columns: i32, column: i32) -> Vec<i32> {
            children
                .chunks(columns.max(1) as usize)
                .map(|row| {
                    row.iter()
                        .map(|c| c.measure(gtk::Orientation::Vertical, column).1)
                        .max()
                        .unwrap_or(0)
                })
                .collect()
        }
    }
}

glib::wrapper! {
    /// Children in equal columns that continue on further rows when narrow.
    pub(crate) struct TileGrid(ObjectSubclass<imp::TileGrid>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TileGrid {
    /// An empty grid with `spacing` between columns and `row_spacing` between
    /// rows, whose columns are at least `least` wide.
    pub(crate) fn new(spacing: i32, row_spacing: i32, least: i32) -> Self {
        let grid: Self = glib::Object::new();
        grid.imp().spacing.set(spacing);
        grid.imp().row_spacing.set(row_spacing);
        grid.imp().least.set(least);
        grid
    }

    /// Adds `child` at the end.
    pub(crate) fn append(&self, child: &impl IsA<gtk::Widget>) {
        child.set_parent(self);
    }
}

#[cfg(test)]
mod tests {
    use super::columns_for;

    #[test]
    fn rows_are_filled_evenly() {
        // Room for all five tiles: one row.
        assert_eq!(columns_for(5, 150, 12, 844), 5);
        // Room for four: three and two rather than four and one.
        assert_eq!(columns_for(5, 150, 12, 700), 3);
        // Room for two: two, two and one.
        assert_eq!(columns_for(5, 150, 12, 320), 2);
        // Never fewer than one column.
        assert_eq!(columns_for(5, 150, 12, 100), 1);
        // Two columns that do not fit side by side are stacked.
        assert_eq!(columns_for(2, 300, 48, 600), 1);
        assert_eq!(columns_for(2, 300, 48, 648), 2);
    }
}
