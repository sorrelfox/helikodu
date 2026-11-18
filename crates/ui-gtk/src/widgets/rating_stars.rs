// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! RatingStars: five stars on a 13 px pitch, filled in the accent fill, empty
//! ones in `line2`. Shows a rating; an editable one sets it: a click on a star
//! sets that many, a click on the current value clears it, and the arrow keys
//! change it by one.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, gsk};

use crate::i18n::{fill, ntr, tr};

const STAR: f32 = 12.0;
const PITCH: f32 = 13.0;

fn star(x: f32, y: f32, size: f32) -> gsk::Path {
    let builder = gsk::PathBuilder::new();
    let (cx, cy) = (x + size / 2.0, y + size / 2.0 + size * 0.04);
    let outer = size / 2.0;
    let inner = outer * 0.45;
    for i in 0..10 {
        let r = if i % 2 == 0 { outer } else { inner };
        let a = std::f32::consts::PI * (i as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
        let (px, py) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            builder.move_to(px, py);
        } else {
            builder.line_to(px, py);
        }
    }
    builder.close();
    builder.to_path()
}

mod imp {
    use super::*;

    type Changed = Box<dyn Fn(Option<u8>)>;

    #[derive(Default)]
    pub(crate) struct RatingStars {
        pub(super) stars: Cell<Option<u8>>,
        pub(super) editable: Cell<bool>,
        pub(super) changed: RefCell<Option<Changed>>,
    }

    impl std::fmt::Debug for RatingStars {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("RatingStars")
                .field("stars", &self.stars)
                .finish_non_exhaustive()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RatingStars {
        const NAME: &'static str = "HkRatingStars";
        type Type = super::RatingStars;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for RatingStars {
        fn constructed(&self) {
            self.parent_constructed();
            super::super::redraw_on_theme_change(&*self.obj());
        }
    }

    impl WidgetImpl for RatingStars {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (64, 64, -1, -1),
                _ => (STAR as i32, STAR as i32, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let look = crate::style::current();
            let filled = look.accent.fill.to_gdk();
            let empty = look.palette.line2.to_gdk();
            let y = (self.obj().height() as f32 - STAR) / 2.0;
            let stars = self.stars.get().unwrap_or(0);
            for i in 0..5u8 {
                let color = if i < stars { &filled } else { &empty };
                snapshot.append_fill(
                    &star(f32::from(i) * PITCH, y, STAR - 1.0),
                    gsk::FillRule::Winding,
                    color,
                );
            }
        }
    }
}

glib::wrapper! {
    /// A rating of one to five stars, or none.
    pub(crate) struct RatingStars(ObjectSubclass<imp::RatingStars>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for RatingStars {
    fn default() -> Self {
        let obj: Self = glib::Object::new();
        obj.set_rating(None);
        obj
    }
}

impl RatingStars {
    /// Lets the user set the rating; `changed` gets each new value.
    pub(crate) fn set_editable(&self, changed: impl Fn(Option<u8>) + 'static) {
        let imp = self.imp();
        if imp.editable.replace(true) {
            imp.changed.replace(Some(Box::new(changed)));
            return;
        }
        imp.changed.replace(Some(Box::new(changed)));
        self.set_accessible_role(gtk::AccessibleRole::Slider);
        self.set_focusable(true);
        self.set_cursor_from_name(Some("pointer"));
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        let weak = self.downgrade();
        click.connect_pressed(move |gesture, _, x, _| {
            let Some(stars) = weak.upgrade() else { return };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let clicked = ((x as f32 / PITCH).floor() as u8 + 1).clamp(1, 5);
            let new = if stars.imp().stars.get() == Some(clicked) {
                None
            } else {
                Some(clicked)
            };
            stars.change(new);
        });
        self.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(stars) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let now = stars.imp().stars.get().unwrap_or(0);
            let new = match key {
                gtk::gdk::Key::Right | gtk::gdk::Key::Up => now.saturating_add(1).min(5),
                gtk::gdk::Key::Left | gtk::gdk::Key::Down => now.saturating_sub(1),
                gtk::gdk::Key::Home => 0,
                gtk::gdk::Key::End => 5,
                _ => return glib::Propagation::Proceed,
            };
            stars.change((new > 0).then_some(new));
            glib::Propagation::Stop
        });
        self.add_controller(keys);
        self.set_rating(self.imp().stars.get());
    }

    fn change(&self, stars: Option<u8>) {
        if self.imp().stars.get() == stars {
            return;
        }
        self.set_rating(stars);
        if let Some(changed) = self.imp().changed.borrow().as_ref() {
            changed(stars);
        }
    }

    /// Shows `stars` (1 to 5), or none.
    pub(crate) fn set_rating(&self, stars: Option<u8>) {
        self.imp().stars.set(stars.map(|s| s.min(5)));
        let text = match stars {
            Some(n) => fill(
                &ntr("{} of 5 stars", "{} of 5 stars", usize::from(n)),
                &[&n.to_string()],
            ),
            None => tr("Not rated"),
        };
        if self.imp().editable.get() {
            self.update_property(&[
                gtk::accessible::Property::Label(&tr("Rating")),
                gtk::accessible::Property::ValueMin(0.0),
                gtk::accessible::Property::ValueMax(5.0),
                gtk::accessible::Property::ValueNow(f64::from(stars.unwrap_or(0))),
                gtk::accessible::Property::ValueText(&text),
            ]);
            self.set_tooltip_text(Some(&text));
        } else {
            self.update_property(&[gtk::accessible::Property::Label(&text)]);
        }
        self.queue_draw();
    }
}
