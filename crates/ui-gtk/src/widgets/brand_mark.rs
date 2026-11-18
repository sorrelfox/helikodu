// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The logo in the brand strip: a roof chevron in the accent fill over three
//! level bars in the primary text colour, on the 16-unit grid of the
//! application icon.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

const ROOF: &str =
    "M8 0.69 15.146 6.737a1 1 0 0 1-1.292 1.526L8 3.31 2.146 8.263A1 1 0 0 1 0.854 6.737Z";
const BARS: &str = "M3 11a1 1 0 0 1 2 0v3a1 1 0 0 1-2 0ZM7 8.5a1 1 0 0 1 2 0v5.5a1 1 0 0 1-2 0ZM11 11a1 1 0 0 1 2 0v3a1 1 0 0 1-2 0Z";
const SIDE: i32 = 18;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct BrandMark;

    #[glib::object_subclass]
    impl ObjectSubclass for BrandMark {
        const NAME: &'static str = "HkBrandMark";
        type Type = super::BrandMark;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    impl ObjectImpl for BrandMark {
        fn constructed(&self) {
            self.parent_constructed();
            super::super::redraw_on_theme_change(&*self.obj());
        }
    }

    impl WidgetImpl for BrandMark {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (SIDE, SIDE, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let look = crate::style::current();
            let obj = self.obj();
            let scale = SIDE as f32 / 16.0;
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (obj.width() - SIDE) as f32 / 2.0,
                (obj.height() - SIDE) as f32 / 2.0,
            ));
            snapshot.scale(scale, scale);
            for (data, color) in [(ROOF, look.accent.fill), (BARS, look.palette.t1)] {
                if let Ok(path) = gsk::Path::parse(data) {
                    snapshot.append_fill(&path, gsk::FillRule::Winding, &color.to_gdk());
                }
            }
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    /// The Helikodu mark.
    pub(crate) struct BrandMark(ObjectSubclass<imp::BrandMark>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for BrandMark {
    fn default() -> Self {
        glib::Object::new()
    }
}
