// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The row of a group in a grouped track table: 36 px high, `win` background,
//! a line above it except above the first group; a chevron in `t3` (down when
//! open, right when closed), the value in 600 (`t2` for "No value") and the
//! number of tracks in 12 px `t3`.
//!
//! A row of the table is a cell per column, and a cell clips what it draws.
//! Every cell of a group row therefore draws the whole line of the group, moved
//! by its own place in the table, and shows the part that falls in it: the row
//! reads as one line across the columns. The line is placed in the table's
//! visible area, so it stays where it is while the table scrolls sideways; the
//! track view draws the cells again when the table scrolls or its columns
//! change.

use gtk::prelude::*;
use gtk::{graphene, gsk, pango};

use super::TableCell;
use crate::models::GroupHeader;

/// Where the chevron starts, from the left of the table's visible area.
const INSET: f32 = 18.0;
/// The chevron's box.
const CHEVRON: f32 = 16.0;
/// The gap between the chevron, the value and the count.
const GAP: f32 = 10.0;
/// The room kept at the right.
const END: f32 = 28.0;

/// Marks `cell` as a group row's (its classes, accessible name and state), or
/// as a track row's for `None`.
pub(super) fn mark(cell: &TableCell, header: Option<&GroupHeader>) {
    match header {
        Some(header) => {
            cell.add_css_class("hk-group");
            if header.first {
                cell.add_css_class("hk-group-first");
            } else {
                cell.remove_css_class("hk-group-first");
            }
            cell.update_property(&[gtk::accessible::Property::Label(&format!(
                "{}, {}",
                header.value, header.count
            ))]);
            cell.update_state(&[gtk::accessible::State::Expanded(Some(header.open))]);
        }
        None => {
            if cell.has_css_class("hk-group") {
                cell.remove_css_class("hk-group");
                cell.remove_css_class("hk-group-first");
                cell.reset_state(gtk::AccessibleState::Expanded);
            }
        }
    }
}

/// A layout of `text` in `cell`'s font, with `attrs`.
fn layout(cell: &TableCell, text: &str, attrs: &pango::AttrList) -> pango::Layout {
    let layout = cell.create_pango_layout(Some(text));
    layout.set_single_paragraph_mode(true);
    layout.set_attributes(Some(attrs));
    layout
}

/// Draws the line of `header` into `cell`, where the cell lies in the table.
pub(super) fn draw(cell: &TableCell, snapshot: &gtk::Snapshot, header: &GroupHeader) {
    let Some(table) = cell.ancestor(gtk::ColumnView::static_type()) else {
        return;
    };
    let Some(origin) = cell.compute_point(&table, &graphene::Point::new(0.0, 0.0)) else {
        return;
    };
    let palette = crate::style::current().palette;
    let height = cell.height() as f32;
    let mut x = INSET - origin.x();
    let rtl = cell.direction() == gtk::TextDirection::Rtl;
    let room = table.width() as f32 - INSET - END;
    // Right to left, the line starts at the right of the visible area.
    let place = |x: f32, width: f32| {
        if rtl {
            table.width() as f32 - origin.x() - (x + origin.x()) - width
        } else {
            x
        }
    };

    let top = (height - CHEVRON) / 2.0;
    let path = gsk::PathBuilder::new();
    let left = place(x, CHEVRON);
    let (open, right) = (header.open, rtl);
    let points: [(f32, f32); 3] = if open {
        [(4.5, 6.5), (8.0, 10.0), (11.5, 6.5)]
    } else if right {
        [(9.5, 4.5), (6.0, 8.0), (9.5, 11.5)]
    } else {
        [(6.5, 4.5), (10.0, 8.0), (6.5, 11.5)]
    };
    path.move_to(left + points[0].0, top + points[0].1);
    path.line_to(left + points[1].0, top + points[1].1);
    path.line_to(left + points[2].0, top + points[2].1);
    let stroke = gsk::Stroke::new(1.5);
    stroke.set_line_cap(gsk::LineCap::Round);
    stroke.set_line_join(gsk::LineJoin::Round);
    snapshot.append_stroke(&path.to_path(), &stroke, &palette.t3.to_gdk());
    x += CHEVRON + GAP;

    let count_attrs = pango::AttrList::new();
    count_attrs.insert(pango::AttrSize::new_size_absolute(12 * pango::SCALE));
    count_attrs.insert(pango::AttrFontFeatures::new("\"tnum\" 1"));
    let count = layout(cell, &header.count, &count_attrs);
    let (count_width, count_height) = count.pixel_size();
    let value_attrs = pango::AttrList::new();
    value_attrs.insert(pango::AttrInt::new_weight(pango::Weight::Semibold));
    let value = layout(cell, &header.value, &value_attrs);
    let value_room = (room - (CHEVRON + GAP) - GAP - count_width as f32).max(24.0);
    value.set_width((value_room as i32) * pango::SCALE);
    value.set_ellipsize(pango::EllipsizeMode::End);
    let (value_width, value_height) = value.pixel_size();
    let color = if header.no_value {
        palette.t2
    } else {
        palette.t1
    };
    snapshot.save();
    snapshot.translate(&graphene::Point::new(
        place(x, value_width as f32).round(),
        ((height - value_height as f32) / 2.0).round(),
    ));
    snapshot.append_layout(&value, &color.to_gdk());
    snapshot.restore();
    x += value_width as f32 + GAP;
    snapshot.save();
    snapshot.translate(&graphene::Point::new(
        place(x, count_width as f32).round(),
        ((height - count_height as f32) / 2.0 + 1.0).round(),
    ));
    snapshot.append_layout(&count, &palette.t3.to_gdk());
    snapshot.restore();
}
