// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! BipolarScale: a slider that fills from its centre, for gains around 0 dB.
//! A 3 px trough in `line2`, the part between the centre and the thumb in the
//! accent fill, a 1 × 12 tick in `t3` at zero and a 14 px thumb in `t1` with a
//! 3 px ring in the pane colour. Click or drag sets the value; the arrow keys
//! move it by one step, Page Up and Page Down by six, Home and End to the ends.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use crate::i18n::{fill, tr};

const THUMB: f32 = 14.0;
const RING: f32 = 3.0;
const TROUGH: f32 = 3.0;
const TICK: f32 = 12.0;

type Listener = Box<dyn Fn(f64)>;
type ValueText = Box<dyn Fn(f64) -> String>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub(crate) struct BipolarScale {
        pub(super) value: Cell<f64>,
        pub(super) min: Cell<f64>,
        pub(super) max: Cell<f64>,
        pub(super) step: Cell<f64>,
        pub(super) listeners: RefCell<Vec<Listener>>,
        pub(super) text: RefCell<Option<ValueText>>,
    }

    impl std::fmt::Debug for BipolarScale {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("BipolarScale")
                .field("value", &self.value.get())
                .finish_non_exhaustive()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BipolarScale {
        const NAME: &'static str = "HkBipolarScale";
        type Type = super::BipolarScale;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Slider);
        }
    }

    impl ObjectImpl for BipolarScale {
        fn constructed(&self) {
            self.parent_constructed();
            self.min.set(-12.0);
            self.max.set(12.0);
            self.step.set(0.5);
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_focus_on_click(true);
            super::super::redraw_on_theme_change(&*obj);

            let drag = gtk::GestureDrag::new();
            drag.connect_drag_begin(|gesture, x, _| {
                if let Some(scale) = gesture.widget().and_downcast::<super::BipolarScale>() {
                    scale.grab_focus();
                    scale.set_from_x(x);
                }
            });
            drag.connect_drag_update(|gesture, dx, _| {
                if let (Some(scale), Some((x, _))) = (
                    gesture.widget().and_downcast::<super::BipolarScale>(),
                    gesture.start_point(),
                ) {
                    scale.set_from_x(x + dx);
                }
            });
            obj.add_controller(drag);

            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(|controller, key, _, _| {
                let Some(scale) = controller.widget().and_downcast::<super::BipolarScale>() else {
                    return glib::Propagation::Proceed;
                };
                let imp = scale.imp();
                let step = imp.step.get();
                let value = imp.value.get();
                let to = match key {
                    gdk::Key::Left | gdk::Key::Down | gdk::Key::KP_Left | gdk::Key::KP_Down => {
                        value - step
                    }
                    gdk::Key::Right | gdk::Key::Up | gdk::Key::KP_Right | gdk::Key::KP_Up => {
                        value + step
                    }
                    gdk::Key::Page_Down => value - step * 6.0,
                    gdk::Key::Page_Up => value + step * 6.0,
                    gdk::Key::Home => imp.min.get(),
                    gdk::Key::End => imp.max.get(),
                    _ => return glib::Propagation::Proceed,
                };
                scale.change(to);
                glib::Propagation::Stop
            });
            obj.add_controller(keys);
        }
    }

    impl WidgetImpl for BipolarScale {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (120, 200, -1, -1),
                _ => (20, 20, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let look = crate::style::current();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            let left = THUMB / 2.0 + RING;
            let span = (w - 2.0 * left).max(1.0);
            let x_of = |v: f64| {
                let (min, max) = (self.min.get(), self.max.get());
                left + span * ((v - min) / (max - min)).clamp(0.0, 1.0) as f32
            };
            let cy = h / 2.0;
            let trough = graphene::Rect::new(left, cy - TROUGH / 2.0, span, TROUGH);
            let rounded = gsk::RoundedRect::from_rect(trough, TROUGH / 2.0);
            snapshot.push_rounded_clip(&rounded);
            snapshot.append_color(&look.palette.line2.to_gdk(), &trough);
            snapshot.pop();
            let zero = x_of(0.0);
            let at = x_of(self.value.get());
            let (from, to) = if at < zero { (at, zero) } else { (zero, at) };
            if to > from {
                snapshot.append_color(
                    &look.accent.fill.to_gdk(),
                    &graphene::Rect::new(from, cy - TROUGH / 2.0, to - from, TROUGH),
                );
            }
            snapshot.append_color(
                &look.palette.t3.to_gdk(),
                &graphene::Rect::new(zero - 0.5, cy - TICK / 2.0, 1.0, TICK),
            );
            let outer = THUMB / 2.0 + RING;
            let ring = graphene::Rect::new(at - outer, cy - outer, outer * 2.0, outer * 2.0);
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(ring, outer));
            snapshot.append_color(&look.palette.pane.to_gdk(), &ring);
            snapshot.pop();
            let thumb = graphene::Rect::new(at - THUMB / 2.0, cy - THUMB / 2.0, THUMB, THUMB);
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(thumb, THUMB / 2.0));
            snapshot.append_color(&look.palette.t1.to_gdk(), &thumb);
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    /// A slider that fills from its centre.
    pub(crate) struct BipolarScale(ObjectSubclass<imp::BipolarScale>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for BipolarScale {
    fn default() -> Self {
        let scale: Self = glib::Object::new();
        scale.set_value(0.0);
        scale
    }
}

/// A gain as the scale reads it: `−6.0 dB`, `+1.5 dB`, `0.0 dB`.
pub(crate) fn decibels(value: f64) -> String {
    let text = if value > 0.0 {
        format!("+{value:.1}")
    } else if value < 0.0 {
        format!("−{:.1}", -value)
    } else {
        "0.0".into()
    };
    fill(&tr("{} dB"), &[&text])
}

impl BipolarScale {
    /// Shows `value` without telling the listeners.
    pub(crate) fn set_value(&self, value: f64) {
        let imp = self.imp();
        let value = value.clamp(imp.min.get(), imp.max.get());
        imp.value.set(value);
        self.update_property(&[
            gtk::accessible::Property::ValueMin(imp.min.get()),
            gtk::accessible::Property::ValueMax(imp.max.get()),
            gtk::accessible::Property::ValueNow(value),
            gtk::accessible::Property::ValueText(&self.value_text(value)),
        ]);
        self.queue_draw();
    }

    /// Reads values with `text` instead of in decibels, for assistive
    /// technology.
    pub(crate) fn set_value_text(&self, text: impl Fn(f64) -> String + 'static) {
        self.imp().text.replace(Some(Box::new(text)));
        self.set_value(self.imp().value.get());
    }

    fn value_text(&self, value: f64) -> String {
        match self.imp().text.borrow().as_ref() {
            Some(text) => text(value),
            None => decibels(value),
        }
    }

    /// Calls `listener` with each value the user sets.
    pub(crate) fn connect_changed(&self, listener: impl Fn(f64) + 'static) {
        self.imp().listeners.borrow_mut().push(Box::new(listener));
    }

    fn change(&self, value: f64) {
        let imp = self.imp();
        let step = imp.step.get();
        let value = ((value / step).round() * step).clamp(imp.min.get(), imp.max.get());
        if (value - imp.value.get()).abs() < f64::EPSILON {
            return;
        }
        self.set_value(value);
        for listener in imp.listeners.borrow().iter() {
            listener(value);
        }
    }

    fn set_from_x(&self, x: f64) {
        let imp = self.imp();
        let left = f64::from(THUMB / 2.0 + RING);
        let span = (f64::from(self.width()) - 2.0 * left).max(1.0);
        let share = ((x - left) / span).clamp(0.0, 1.0);
        self.change(imp.min.get() + share * (imp.max.get() - imp.min.get()));
    }
}

#[cfg(test)]
mod tests {
    use super::decibels;

    #[test]
    fn values_carry_their_sign() {
        assert_eq!(decibels(-6.0), "−6.0 dB");
        assert_eq!(decibels(1.5), "+1.5 dB");
        assert_eq!(decibels(0.0), "0.0 dB");
    }
}
