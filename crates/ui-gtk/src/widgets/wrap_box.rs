// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A row of widgets that continues on a new line when it is narrower than its
//! children together: the buttons of a page header, which in a longer language
//! can be wider than the page. Each child keeps its natural width (or the
//! whole row's, when that is less); a line is as tall as its tallest child.
//! As wide as there is room for, it is a plain row, as the design draws it.

use std::cell::Cell;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct WrapBox {
        /// The gap between children on a line.
        pub(super) spacing: Cell<i32>,
        /// The gap between lines.
        pub(super) row_spacing: Cell<i32>,
        /// Whether one line reports its children's baseline, so that the row
        /// around it lines them up with its labels as it would the children
        /// themselves.
        pub(super) baseline: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WrapBox {
        const NAME: &'static str = "HkWrapBox";
        type Type = super::WrapBox;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for WrapBox {
        fn properties() -> &'static [glib::ParamSpec] {
            use std::sync::OnceLock;
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecInt::builder("spacing")
                        .minimum(0)
                        .default_value(0)
                        .build(),
                    glib::ParamSpecInt::builder("row-spacing")
                        .minimum(0)
                        .default_value(0)
                        .build(),
                    glib::ParamSpecBoolean::builder("baseline").build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "spacing" => self.spacing.set(value.get().unwrap_or(0)),
                "baseline" => self.baseline.set(value.get().unwrap_or(false)),
                _ => self.row_spacing.set(value.get().unwrap_or(0)),
            }
            self.obj().queue_resize();
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "spacing" => self.spacing.get().to_value(),
                "baseline" => self.baseline.get().to_value(),
                _ => self.row_spacing.get().to_value(),
            }
        }

        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for WrapBox {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let children = self.children();
            if orientation == gtk::Orientation::Horizontal {
                // At least the widest child; at best all on one line.
                let gaps = self.spacing.get() * (children.len() as i32 - 1).max(0);
                let min = children.iter().map(|(_, min, _)| *min).max().unwrap_or(0);
                let nat = children.iter().map(|(_, _, nat)| *nat).sum::<i32>() + gaps;
                (min, nat.max(min), -1, -1)
            } else {
                let width = if for_size < 0 { i32::MAX } else { for_size };
                let lines = self.lines(&children, width);
                let height = lines.iter().map(|line| line.height).sum::<i32>();
                let total = height + self.row_spacing.get() * (lines.len() as i32 - 1).max(0);
                let baseline = match lines.as_slice() {
                    [line] if self.baseline.get() => line.baseline,
                    _ => -1,
                };
                (total, total, baseline, baseline)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let children = self.children();
            let rtl = self.obj().direction() == gtk::TextDirection::Rtl;
            let mut y = 0;
            let mut lines = self.lines(&children, width);
            if let [line] = lines.as_mut_slice()
                && self.baseline.get()
            {
                // One line has the whole height, where the children align
                // themselves as they would in the row around it.
                line.height = line.height.max(height);
            }
            for line in lines {
                let mut x = 0;
                for (child, w) in &line.items {
                    let left = if rtl { width - x - w } else { x };
                    child.size_allocate(&gtk::Allocation::new(left, y, *w, line.height), -1);
                    x += w + self.spacing.get();
                }
                y += line.height + self.row_spacing.get();
            }
        }
    }

    /// The children of a line with their widths, and the line's height.
    pub(super) struct Line {
        pub(super) items: Vec<(gtk::Widget, i32)>,
        pub(super) height: i32,
        /// The lowest baseline of its children, or -1.
        pub(super) baseline: i32,
    }

    impl WrapBox {
        /// The visible children with their minimum and natural widths.
        pub(super) fn children(&self) -> Vec<(gtk::Widget, i32, i32)> {
            let mut out = Vec::new();
            let mut child = self.obj().first_child();
            while let Some(c) = child {
                if c.should_layout() {
                    let (min, nat, ..) = c.measure(gtk::Orientation::Horizontal, -1);
                    out.push((c.clone(), min, nat));
                }
                child = c.next_sibling();
            }
            out
        }

        /// The children split into lines `width` wide.
        pub(super) fn lines(&self, children: &[(gtk::Widget, i32, i32)], width: i32) -> Vec<Line> {
            let spacing = self.spacing.get();
            let mut lines: Vec<Line> = Vec::new();
            let mut used = 0;
            for (child, min, nat) in children {
                let w = (*nat).min(width).max(*min);
                let fits = lines
                    .last()
                    .is_some_and(|l| !l.items.is_empty() && used + spacing + w <= width);
                if fits {
                    used += spacing + w;
                } else {
                    lines.push(Line {
                        items: Vec::new(),
                        height: 0,
                        baseline: -1,
                    });
                    used = w;
                }
                let (_, h, _, baseline) = child.measure(gtk::Orientation::Vertical, w);
                let line = lines.last_mut().expect("a line was pushed");
                line.height = line.height.max(h);
                line.baseline = line.baseline.max(baseline);
                line.items.push((child.clone(), w));
            }
            lines
        }
    }
}

glib::wrapper! {
    /// A row of widgets that wraps onto further lines when narrow.
    pub(crate) struct WrapBox(ObjectSubclass<imp::WrapBox>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl WrapBox {
    /// An empty row with `spacing` between children and between lines.
    pub(crate) fn new(spacing: i32) -> Self {
        glib::Object::builder()
            .property("spacing", spacing)
            .property("row-spacing", spacing)
            .build()
    }

    /// Adds `child` at the end of the row.
    pub(crate) fn append(&self, child: &impl IsA<gtk::Widget>) {
        child.set_parent(self);
    }

    /// Takes `child` out of the row.
    pub(crate) fn remove(&self, child: &impl IsA<gtk::Widget>) {
        if child.parent().as_ref() == Some(self.upcast_ref()) {
            child.unparent();
        }
    }
}
