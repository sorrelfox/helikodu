// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! WaveformSeekBar: the seek bar drawn as a band of bars. Bars before the
//! position are in the glass accent fill, the rest in `waveRest`; a bar is
//! either played or not. Until the library has peaks for a track every bar is
//! 28 % high (a flat band).
//!
//! Click or drag seeks; the arrow keys move by the seek step, Home and End jump
//! to the ends. A seek is requested on release, and at most ten times a second
//! while dragging.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use crate::i18n::{fill, tr};
use crate::style::tokens::glass;

const BAR: f32 = 5.0;
const GAP: f32 = 2.0;
const FLAT: f32 = 0.28;
const MIN: f32 = 0.08;
/// How far the arrow keys move.
const STEP: Duration = Duration::from_secs(5);

type SeekHandler = Box<dyn Fn(Duration)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub(crate) struct WaveformSeekBar {
        pub(super) elapsed: Cell<Duration>,
        pub(super) length: Cell<Option<Duration>>,
        /// The position shown while dragging.
        pub(super) dragging: Cell<Option<Duration>>,
        pub(super) last_seek: Cell<Option<Instant>>,
        pub(super) peaks: RefCell<Vec<f32>>,
        pub(super) handler: RefCell<Option<SeekHandler>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WaveformSeekBar {
        const NAME: &'static str = "HkWaveformSeekBar";
        type Type = super::WaveformSeekBar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("waveformseekbar");
            klass.set_accessible_role(gtk::AccessibleRole::Slider);
        }
    }

    impl ObjectImpl for WaveformSeekBar {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_hexpand(true);
            obj.update_property(&[gtk::accessible::Property::Label(&tr("Seek"))]);
            super::super::redraw_on_theme_change(&*obj);

            let drag = gtk::GestureDrag::new();
            drag.connect_drag_begin(glib::clone!(
                #[weak]
                obj,
                move |_, x, _| obj.drag_to(x, false)
            ));
            drag.connect_drag_update(glib::clone!(
                #[weak]
                obj,
                move |gesture, dx, _| {
                    if let Some((x, _)) = gesture.start_point() {
                        obj.drag_to(x + dx, false);
                    }
                }
            ));
            drag.connect_drag_end(glib::clone!(
                #[weak]
                obj,
                move |gesture, dx, _| {
                    if let Some((x, _)) = gesture.start_point() {
                        obj.drag_to(x + dx, true);
                    }
                }
            ));
            obj.add_controller(drag);

            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, key, _, _| obj.key(key)
            ));
            obj.add_controller(keys);
        }
    }

    impl WidgetImpl for WaveformSeekBar {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (BAR as i32 * 10, BAR as i32 * 40, -1, -1),
                _ => (34, 34, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            let count = ((w + GAP) / (BAR + GAP)).floor().max(0.0) as usize;
            if count == 0 {
                return;
            }
            let fraction = obj.fraction();
            let played = (fraction * count as f64).floor() as usize;
            let accent = crate::style::current().accent.glass_fill.to_gdk();
            let rest = glass::WAVE_REST.to_gdk();
            let peaks = self.peaks.borrow();
            for i in 0..count {
                let peak = if peaks.is_empty() {
                    FLAT
                } else {
                    peaks[i * peaks.len() / count].max(MIN)
                };
                let bar_h = (peak * h).round().max(2.0);
                let x = i as f32 * (BAR + GAP);
                let rect = graphene::Rect::new(x, ((h - bar_h) / 2.0).round(), BAR, bar_h);
                let color = if i < played { &accent } else { &rest };
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(rect, 1.0));
                snapshot.append_color(color, &rect);
                snapshot.pop();
            }
        }
    }
}

glib::wrapper! {
    /// The seek bar of the deck.
    pub(crate) struct WaveformSeekBar(ObjectSubclass<imp::WaveformSeekBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for WaveformSeekBar {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl WaveformSeekBar {
    /// Calls `handler` with the position the user seeks to.
    pub(crate) fn connect_seek(&self, handler: impl Fn(Duration) + 'static) {
        self.imp().handler.replace(Some(Box::new(handler)));
    }

    /// Shows a position. Ignored while the user drags.
    pub(crate) fn set_position(&self, elapsed: Duration, length: Option<Duration>) {
        let imp = self.imp();
        imp.elapsed.set(elapsed);
        imp.length.set(length);
        self.set_sensitive(length.is_some_and(|l| !l.is_zero()));
        self.update_accessible();
        self.queue_draw();
    }

    fn shown(&self) -> Duration {
        let imp = self.imp();
        imp.dragging.get().unwrap_or(imp.elapsed.get())
    }

    fn fraction(&self) -> f64 {
        match self.imp().length.get() {
            Some(length) if !length.is_zero() => {
                (self.shown().as_secs_f64() / length.as_secs_f64()).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }

    fn update_accessible(&self) {
        let imp = self.imp();
        let length = imp.length.get().unwrap_or_default();
        let shown = self.shown();
        let text = fill(
            &tr("{} of {}"),
            &[
                &crate::format::duration(shown),
                &crate::format::duration(length),
            ],
        );
        self.update_property(&[
            gtk::accessible::Property::ValueMin(0.0),
            gtk::accessible::Property::ValueMax(length.as_secs_f64()),
            gtk::accessible::Property::ValueNow(shown.as_secs_f64()),
            gtk::accessible::Property::ValueText(&text),
        ]);
    }

    fn request(&self, to: Duration) {
        if let Some(handler) = self.imp().handler.borrow().as_ref() {
            handler(to);
        }
    }

    fn drag_to(&self, x: f64, done: bool) {
        let imp = self.imp();
        let Some(length) = imp.length.get() else {
            return;
        };
        let width = f64::from(self.width().max(1));
        let to = length.mul_f64((x / width).clamp(0.0, 1.0));
        if done {
            imp.dragging.set(None);
            imp.elapsed.set(to);
            imp.last_seek.set(Some(Instant::now()));
            self.request(to);
        } else {
            imp.dragging.set(Some(to));
            let due = imp
                .last_seek
                .get()
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(100));
            if due {
                imp.last_seek.set(Some(Instant::now()));
                self.request(to);
            }
        }
        self.update_accessible();
        self.queue_draw();
    }

    fn key(&self, key: gdk::Key) -> glib::Propagation {
        let imp = self.imp();
        let Some(length) = imp.length.get() else {
            return glib::Propagation::Proceed;
        };
        let now = imp.elapsed.get();
        let to = match key {
            gdk::Key::Left | gdk::Key::KP_Left => now.saturating_sub(STEP),
            gdk::Key::Right | gdk::Key::KP_Right => (now + STEP).min(length),
            gdk::Key::Home => Duration::ZERO,
            gdk::Key::End => length,
            _ => return glib::Propagation::Proceed,
        };
        imp.elapsed.set(to);
        self.request(to);
        self.update_accessible();
        self.queue_draw();
        glib::Propagation::Stop
    }
}
