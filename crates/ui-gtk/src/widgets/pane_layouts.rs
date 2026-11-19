// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Layouts that let a page fit a narrow content area without cutting it off.
//! As wide as the design draws a page, each is the plain row it replaces.
//!
//! - [`HeadLayout`]: a page's header, the titles and the buttons after them.
//!   The buttons stay beside the titles while the titles keep a useful width;
//!   otherwise they go under them, across the whole header.
//! - [`ListDetailLayout`]: a list pane and the detail beside it. The pane keeps
//!   its width while the detail has room, and gives way down to a least width
//!   when it does not.
//! - [`BesideLayout`]: the main content and a side column, with a line
//!   between. The column stays beside the content while the content keeps a
//!   least width; otherwise it goes under it.
//!
//! A layout replaces a box's own, so it is given the box's spacing when it is
//! installed (see the `install` functions).

use std::cell::Cell;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// The least width the titles of a header keep beside its buttons.
const TITLES_LEAST: i32 = 180;
/// The gap between the titles and the buttons under them.
const HEAD_ROW_GAP: i32 = 14;

/// The children that take part in the layout.
fn children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.first_child();
    while let Some(c) = child {
        if c.should_layout() {
            out.push(c.clone());
        }
        child = c.next_sibling();
    }
    out
}

fn width_of(widget: &gtk::Widget) -> (i32, i32) {
    let (min, nat, ..) = widget.measure(gtk::Orientation::Horizontal, -1);
    (min, nat.max(min))
}

fn height_of(widget: &gtk::Widget, width: i32) -> (i32, i32) {
    let (min, nat, ..) = widget.measure(gtk::Orientation::Vertical, width);
    (min, nat.max(min))
}

/// Places `child` at `x`, `y` in a parent `width` wide, mirrored in a
/// right-to-left language.
fn place(child: &gtk::Widget, rtl: bool, width: i32, (x, y, w, h): (i32, i32, i32, i32)) {
    let left = if rtl { width - x - w } else { x };
    child.size_allocate(&gtk::Allocation::new(left, y, w.max(0), h.max(0)), -1);
}

/// The widths of `kids` in a row `width` wide with `spacing` between them:
/// each its minimum, then towards its natural width in order, and what is
/// left to those that expand.
fn row_widths(kids: &[gtk::Widget], width: i32, spacing: i32) -> Vec<i32> {
    let sizes: Vec<(i32, i32)> = kids.iter().map(width_of).collect();
    let gaps = spacing * (kids.len() as i32 - 1).max(0);
    let mut widths: Vec<i32> = sizes.iter().map(|(min, _)| *min).collect();
    let mut extra = (width - gaps - widths.iter().sum::<i32>()).max(0);
    for (w, (min, nat)) in widths.iter_mut().zip(&sizes) {
        let more = (nat - min).min(extra);
        *w += more;
        extra -= more;
    }
    let expanding: Vec<usize> = (0..kids.len())
        .filter(|&i| kids[i].compute_expand(gtk::Orientation::Horizontal))
        .collect();
    let count = expanding.len() as i32;
    for (n, &i) in expanding.iter().enumerate() {
        // The first ones take the pixels that do not divide evenly.
        widths[i] += extra / count + i32::from((n as i32) < extra % count);
    }
    widths
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct HeadLayout {
        pub(super) spacing: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HeadLayout {
        const NAME: &'static str = "HkHeadLayout";
        type Type = super::HeadLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for HeadLayout {}

    impl LayoutManagerImpl for HeadLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let kids = children(widget);
            let Some((buttons, lead)) = kids.split_last() else {
                return (0, 0, -1, -1);
            };
            let spacing = self.spacing.get();
            let (lead_min, lead_nat) = self.lead_width(lead);
            let (buttons_min, buttons_nat) = width_of(buttons);
            if orientation == gtk::Orientation::Horizontal {
                // At least the titles or the buttons, one under the other; at
                // best side by side.
                let min = lead_min.max(buttons_min);
                let nat = if lead.is_empty() {
                    buttons_nat
                } else {
                    lead_nat + spacing + buttons_nat
                };
                return (min, nat.max(min), -1, -1);
            }
            let width = if for_size < 0 {
                lead_nat + spacing + buttons_nat
            } else {
                for_size
            };
            let (min, nat) = match self.buttons_beside(lead, buttons, width) {
                Some(beside) => {
                    let mut heights = self.lead_heights(lead, width - spacing - beside);
                    heights.push(height_of(buttons, beside));
                    heights
                        .into_iter()
                        .fold((0, 0), |(a, b), (min, nat)| (a.max(min), b.max(nat)))
                }
                None => {
                    let (lead_min, lead_nat) = self
                        .lead_heights(lead, width)
                        .into_iter()
                        .fold((0, 0), |(a, b), (min, nat)| (a.max(min), b.max(nat)));
                    let (min, nat) = height_of(buttons, width);
                    let gap = if lead.is_empty() { 0 } else { HEAD_ROW_GAP };
                    (lead_min + gap + min, lead_nat + gap + nat)
                }
            };
            (min, nat, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let kids = children(widget);
            let Some((buttons, lead)) = kids.split_last() else {
                return;
            };
            let rtl = widget.direction() == gtk::TextDirection::Rtl;
            let spacing = self.spacing.get();
            let (lead_width, lead_height) = match self.buttons_beside(lead, buttons, width) {
                Some(beside) => {
                    place(buttons, rtl, width, (width - beside, 0, beside, height));
                    (width - spacing - beside, height)
                }
                None => {
                    let (lead_height, _) = self
                        .lead_heights(lead, width)
                        .into_iter()
                        .fold((0, 0), |(a, b), (min, nat)| (a.max(min), b.max(nat)));
                    let gap = if lead.is_empty() { 0 } else { HEAD_ROW_GAP };
                    let y = lead_height + gap;
                    place(buttons, rtl, width, (0, y, width, height - y));
                    (width, lead_height)
                }
            };
            let mut x = 0;
            for (child, w) in lead.iter().zip(row_widths(lead, lead_width, spacing)) {
                place(child, rtl, width, (x, 0, w, lead_height));
                x += w + spacing;
            }
        }
    }

    impl HeadLayout {
        /// The least and natural widths of the leading children in a row.
        fn lead_width(&self, lead: &[gtk::Widget]) -> (i32, i32) {
            let gaps = self.spacing.get() * (lead.len() as i32 - 1).max(0);
            lead.iter()
                .map(width_of)
                .fold((gaps, gaps), |(a, b), (min, nat)| (a + min, b + nat))
        }

        /// The heights of the leading children in a row `width` wide.
        fn lead_heights(&self, lead: &[gtk::Widget], width: i32) -> Vec<(i32, i32)> {
            lead.iter()
                .zip(row_widths(lead, width, self.spacing.get()))
                .map(|(child, w)| height_of(child, w))
                .collect()
        }

        /// The width of the buttons beside the titles in `width`, if the
        /// titles keep their least width beside them.
        fn buttons_beside(
            &self,
            lead: &[gtk::Widget],
            buttons: &gtk::Widget,
            width: i32,
        ) -> Option<i32> {
            let (min, nat) = width_of(buttons);
            if lead.is_empty() {
                return Some(nat.min(width).max(min));
            }
            let (lead_min, _) = self.lead_width(lead);
            (width - self.spacing.get() - nat >= lead_min + TITLES_LEAST).then_some(nat)
        }
    }

    #[derive(Debug, Default)]
    pub(crate) struct ListDetailLayout {
        /// The pane's width as the design draws it.
        pub(super) pane: Cell<i32>,
        /// The least width the pane gives way to.
        pub(super) pane_least: Cell<i32>,
        /// The width the detail keeps at least before the pane gives way.
        pub(super) detail_least: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ListDetailLayout {
        const NAME: &'static str = "HkListDetailLayout";
        type Type = super::ListDetailLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for ListDetailLayout {}

    impl LayoutManagerImpl for ListDetailLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let kids = children(widget);
            let [pane, detail] = kids.as_slice() else {
                return fallback(&kids, orientation, for_size);
            };
            let (least, wide) = self.pane_range(pane);
            let (detail_min, detail_nat) = width_of(detail);
            if orientation == gtk::Orientation::Horizontal {
                let min = least + detail_min;
                return (min, (wide + detail_nat).max(min), -1, -1);
            }
            let width = if for_size < 0 {
                wide + detail_nat
            } else {
                for_size
            };
            let pane_width = self.pane_width(pane, detail, width);
            let (a, b) = height_of(pane, pane_width);
            let (c, d) = height_of(detail, (width - pane_width).max(detail_min));
            (a.max(c), b.max(d), -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let kids = children(widget);
            let [pane, detail] = kids.as_slice() else {
                if let Some(child) = kids.first() {
                    child.size_allocate(&gtk::Allocation::new(0, 0, width, height), -1);
                }
                return;
            };
            let rtl = widget.direction() == gtk::TextDirection::Rtl;
            let pane_width = self.pane_width(pane, detail, width);
            place(pane, rtl, width, (0, 0, pane_width, height));
            place(
                detail,
                rtl,
                width,
                (pane_width, 0, width - pane_width, height),
            );
        }
    }

    impl ListDetailLayout {
        /// The pane's least and widest width.
        fn pane_range(&self, pane: &gtk::Widget) -> (i32, i32) {
            let (min, _) = width_of(pane);
            let least = min.max(self.pane_least.get());
            (least, self.pane.get().max(least))
        }

        /// The pane's width in `width`: its own, or less down to its least so
        /// that the detail keeps its least width.
        fn pane_width(&self, pane: &gtk::Widget, detail: &gtk::Widget, width: i32) -> i32 {
            let (least, wide) = self.pane_range(pane);
            let (detail_min, _) = width_of(detail);
            let detail_least = detail_min.max(self.detail_least.get());
            (width - detail_least).clamp(least, wide)
        }
    }

    #[derive(Debug, Default)]
    pub(crate) struct BesideLayout {
        /// The width the main content keeps at least beside the column.
        pub(super) main_least: Cell<i32>,
        /// The share of the height the column takes at most under the
        /// content, in hundredths.
        pub(super) share: Cell<i32>,
        /// The column's width beside the content, or 0 for its natural
        /// width.
        pub(super) side_width: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BesideLayout {
        const NAME: &'static str = "HkBesideLayout";
        type Type = super::BesideLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for BesideLayout {}

    impl LayoutManagerImpl for BesideLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let kids = children(widget);
            let Some(parts) = Parts::of(&kids) else {
                return fallback(&kids, orientation, for_size);
            };
            let (main_min, main_nat) = width_of(parts.main);
            let (side_min, _) = width_of(parts.side);
            let side_nat = self.side_width(&parts);
            let line = parts.line();
            if orientation == gtk::Orientation::Horizontal {
                let min = main_min.max(side_min);
                return (min, (main_nat + line + side_nat).max(min), -1, -1);
            }
            let width = if for_size < 0 {
                main_nat + line + side_nat
            } else {
                for_size
            };
            if let Some(side) = self.side_beside(&parts, width) {
                let (a, b) = height_of(parts.main, width - line - side);
                let (c, d) = height_of(parts.side, side);
                (a.max(c), b.max(d), -1, -1)
            } else {
                let (a, b) = height_of(parts.main, width);
                let (c, _) = height_of(parts.side, width);
                let d = parts.side_content_height(width).max(c);
                (a + line + c, b + line + d, -1, -1)
            }
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let kids = children(widget);
            let Some(parts) = Parts::of(&kids) else {
                if let Some(child) = kids.first() {
                    child.size_allocate(&gtk::Allocation::new(0, 0, width, height), -1);
                }
                return;
            };
            let rtl = widget.direction() == gtk::TextDirection::Rtl;
            let line = parts.line();
            if let Some(side) = self.side_beside(&parts, width) {
                let main = width - line - side;
                place(parts.main, rtl, width, (0, 0, main, height));
                if let Some(divider) = parts.divider {
                    place(divider, rtl, width, (main, 0, line, height));
                }
                place(parts.side, rtl, width, (main + line, 0, side, height));
            } else {
                // The column as tall as its content, up to its share of the
                // height, while the content keeps its least height.
                let (main_min, _) = height_of(parts.main, width);
                let (side_min, _) = height_of(parts.side, width);
                let most = height * self.share.get() / 100;
                let side = parts
                    .side_content_height(width)
                    .min(most)
                    .min(height - line - main_min)
                    .max(side_min);
                let main = (height - line - side).max(0);
                place(parts.main, rtl, width, (0, 0, width, main));
                if let Some(divider) = parts.divider {
                    place(divider, rtl, width, (0, main, width, line));
                }
                place(parts.side, rtl, width, (0, main + line, width, side));
            }
        }
    }

    impl BesideLayout {
        /// The column's width beside the content in `width`, if the content
        /// keeps its least width beside it.
        fn side_beside(&self, parts: &Parts<'_>, width: i32) -> Option<i32> {
            let (main_min, _) = width_of(parts.main);
            let side = self.side_width(parts);
            (width - parts.line() - side >= main_min.max(self.main_least.get())).then_some(side)
        }

        /// The column's width beside the content.
        fn side_width(&self, parts: &Parts<'_>) -> i32 {
            let (min, nat) = width_of(parts.side);
            match self.side_width.get() {
                0 => nat,
                width => width.max(min),
            }
        }
    }

    /// The children: the content, the line between, the column.
    struct Parts<'a> {
        main: &'a gtk::Widget,
        divider: Option<&'a gtk::Widget>,
        side: &'a gtk::Widget,
    }

    impl<'a> Parts<'a> {
        fn of(kids: &'a [gtk::Widget]) -> Option<Self> {
            match kids {
                [main, side] => Some(Self {
                    main,
                    divider: None,
                    side,
                }),
                [main, divider, side] => Some(Self {
                    main,
                    divider: Some(divider),
                    side,
                }),
                _ => None,
            }
        }

        /// The width (or height) of the line between.
        fn line(&self) -> i32 {
            i32::from(self.divider.is_some())
        }

        /// The height the column's content asks for at `width`: what a
        /// scrolled column scrolls, otherwise the column itself.
        fn side_content_height(&self, width: i32) -> i32 {
            let content = self
                .side
                .downcast_ref::<gtk::ScrolledWindow>()
                .and_then(gtk::ScrolledWindow::child)
                .unwrap_or_else(|| self.side.clone());
            let (min, _) = width_of(&content);
            height_of(&content, width.max(min)).1
        }
    }

    /// The size of a layout with other children than it lays out: that of
    /// its first child.
    fn fallback(
        kids: &[gtk::Widget],
        orientation: gtk::Orientation,
        for_size: i32,
    ) -> (i32, i32, i32, i32) {
        kids.first().map_or((0, 0, -1, -1), |child| {
            let (min, nat, ..) = child.measure(orientation, for_size);
            (min, nat, -1, -1)
        })
    }
}

glib::wrapper! {
    /// Lays out a page's header: the titles, and the buttons beside them or
    /// under them when the header is narrow.
    pub(crate) struct HeadLayout(ObjectSubclass<imp::HeadLayout>)
        @extends gtk::LayoutManager;
}

impl HeadLayout {
    /// Lays out `header`, a row whose last child holds the buttons, with its
    /// spacing.
    pub(crate) fn install(header: &gtk::Box) {
        let layout: Self = glib::Object::new();
        layout.imp().spacing.set(header.spacing());
        header.set_layout_manager(Some(layout));
    }
}

glib::wrapper! {
    /// Lays out a list pane and the detail beside it; the pane gives way when
    /// the detail is short of room.
    pub(crate) struct ListDetailLayout(ObjectSubclass<imp::ListDetailLayout>)
        @extends gtk::LayoutManager;
}

impl ListDetailLayout {
    /// Lays out `row`, the pane and then the detail: the pane `width` wide,
    /// or less down to `least` so that the detail keeps `detail_least`.
    pub(crate) fn install(row: &gtk::Box, width: i32, least: i32, detail_least: i32) {
        let layout: Self = glib::Object::new();
        let imp = layout.imp();
        imp.pane.set(width);
        imp.pane_least.set(least);
        imp.detail_least.set(detail_least);
        row.set_layout_manager(Some(layout));
    }
}

glib::wrapper! {
    /// Lays out the main content and a side column, beside it or under it.
    pub(crate) struct BesideLayout(ObjectSubclass<imp::BesideLayout>)
        @extends gtk::LayoutManager;
}

impl BesideLayout {
    /// Lays out `row`: the content, an optional line and the column,
    /// `side_width` wide (0 for its natural width). Under `main_least` for
    /// the content, the column goes under it, taking at most `share`
    /// hundredths of the height.
    pub(crate) fn install(row: &gtk::Box, main_least: i32, share: i32, side_width: i32) {
        let layout: Self = glib::Object::new();
        let imp = layout.imp();
        imp.main_least.set(main_least);
        imp.share.set(share);
        imp.side_width.set(side_width);
        row.set_layout_manager(Some(layout));
    }
}

/// A line between two parts of a page, upright or across as its layout
/// places it.
pub(crate) fn divider() -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.add_css_class("hk-pane-divider");
    line
}
