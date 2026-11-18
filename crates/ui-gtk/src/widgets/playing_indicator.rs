// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! PlayingIndicator: three bars in the accent fill that replace the track
//! number of the playing row. Static.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene};

const SIDE: i32 = 12;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct PlayingIndicator;

    #[glib::object_subclass]
    impl ObjectSubclass for PlayingIndicator {
        const NAME: &'static str = "HkPlayingIndicator";
        type Type = super::PlayingIndicator;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for PlayingIndicator {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.update_property(&[gtk::accessible::Property::Label(&crate::i18n::tr(
                "Playing",
            ))]);
            super::super::redraw_on_theme_change(&*obj);
        }
    }

    impl WidgetImpl for PlayingIndicator {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (SIDE, SIDE, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let look = crate::style::current();
            let color = look.accent.text.to_gdk();
            let x0 = (obj.width() - SIDE) as f32 / 2.0;
            let y0 = (obj.height() - SIDE) as f32 / 2.0;
            for (i, h) in [7.0_f32, 12.0, 9.0].into_iter().enumerate() {
                let x = x0 + i as f32 * 4.5;
                snapshot.append_color(&color, &graphene::Rect::new(x, y0 + 12.0 - h, 3.0, h));
            }
        }
    }
}

glib::wrapper! {
    /// Three bars that mark the playing track.
    pub(crate) struct PlayingIndicator(ObjectSubclass<imp::PlayingIndicator>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for PlayingIndicator {
    fn default() -> Self {
        glib::Object::new()
    }
}
