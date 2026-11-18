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
//!
//! An A-B loop shows as two flags: a 1 px line at each end with a 12 px tab at
//! the top that carries "A" or "B". Bars outside the loop are dimmed; inside
//! it, played bars keep the accent and the others are lighter. Dragging a flag
//! moves that end of the loop (requested on release).
//!
//! For a chapter of a file the bar covers the whole file: the deck gives the
//! positions in the file, the start of the playing chapter (the loop's points
//! are within the chapter) and the chapter boundaries, each a 3 px gap in the
//! bars.

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
/// The width of the gap at a chapter boundary.
const CHAPTER_GAP: f32 = 3.0;
/// How far the arrow keys move.
const STEP: Duration = Duration::from_secs(5);

type SeekHandler = Box<dyn Fn(Duration)>;
type LoopHandler = Box<dyn Fn(Duration, Duration)>;

/// How near a flag's line a press takes the flag rather than seeking.
const FLAG_REACH: f64 = 6.0;

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
        /// The A-B loop's points, as far as they are set.
        pub(super) ab: Cell<(Option<Duration>, Option<Duration>)>,
        /// The flag being dragged (`true` for B) and where it is now.
        pub(super) flag_drag: Cell<Option<(bool, Duration)>>,
        pub(super) loop_handler: RefCell<Option<LoopHandler>>,
        /// Where the playing chapter starts in the file; the loop's points are
        /// within the chapter.
        pub(super) offset: Cell<Duration>,
        /// The chapter boundaries, as fractions of the file.
        pub(super) gaps: RefCell<Vec<f64>>,
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
                move |_, x, y| obj.press(x, y)
            ));
            drag.connect_drag_update(glib::clone!(
                #[weak]
                obj,
                move |gesture, dx, _| {
                    if let Some((x, _)) = gesture.start_point() {
                        if obj.imp().flag_drag.get().is_some() {
                            obj.move_flag(x + dx, false);
                        } else {
                            obj.drag_to(x + dx, false);
                        }
                    }
                }
            ));
            drag.connect_drag_end(glib::clone!(
                #[weak]
                obj,
                move |gesture, dx, _| {
                    if let Some((x, _)) = gesture.start_point() {
                        if obj.imp().flag_drag.get().is_some() {
                            obj.move_flag(x + dx, true);
                        } else {
                            obj.drag_to(x + dx, true);
                        }
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
            let pitch = (w + GAP) / count as f32;
            let fraction = obj.fraction();
            let played = (fraction * count as f64).floor() as usize;
            let accent = crate::style::current().accent.glass_fill.to_gdk();
            let rest = glass::WAVE_REST.to_gdk();
            let peaks = self.peaks.borrow();
            let (a, b) = obj.shown_loop();
            let section = obj.fractions(a, b);
            let outside = glass::LOOP_OUTSIDE.to_gdk();
            let inside = glass::LOOP_INSIDE.to_gdk();
            let gaps: Vec<(f32, f32)> = self
                .gaps
                .borrow()
                .iter()
                .map(|g| {
                    let x = (*g as f32 * w).round();
                    (x - CHAPTER_GAP / 2.0, x + CHAPTER_GAP / 2.0)
                })
                .collect();
            for i in 0..count {
                let peak = if peaks.is_empty() {
                    FLAT
                } else {
                    peaks[i * peaks.len() / count].max(MIN)
                };
                let bar_h = (peak * h).round().max(2.0);
                // The bars share the whole width, as in the design: the space
                // a whole bar would not fill is spread over all of them.
                let x = (i as f32 * pitch).round();
                let width = ((i + 1) as f32 * pitch - GAP).round() - x;
                let rect = graphene::Rect::new(x, ((h - bar_h) / 2.0).round(), width, bar_h);
                let middle = (i as f64 + 0.5) / count as f64;
                let color = match section {
                    Some((from, to)) if middle < from || middle > to => &outside,
                    Some(_) if i < played => &accent,
                    Some(_) => &inside,
                    None if i < played => &accent,
                    None => &rest,
                };
                for piece in without_gaps(rect, &gaps) {
                    snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(piece, 1.0));
                    snapshot.append_color(color, &piece);
                    snapshot.pop();
                }
            }
            drop(peaks);
            obj.draw_flags(snapshot, a, b);
        }
    }
}

glib::wrapper! {
    /// The seek bar of the deck.
    pub(crate) struct WaveformSeekBar(ObjectSubclass<imp::WaveformSeekBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// The parts of `rect` outside the horizontal ranges `gaps`.
fn without_gaps(rect: graphene::Rect, gaps: &[(f32, f32)]) -> Vec<graphene::Rect> {
    let mut pieces = vec![(rect.x(), rect.x() + rect.width())];
    for &(from, to) in gaps {
        pieces = pieces
            .into_iter()
            .flat_map(|(left, right)| {
                if to <= left || from >= right {
                    vec![(left, right)]
                } else {
                    [(left, from.max(left)), (to.min(right), right)]
                        .into_iter()
                        .filter(|(l, r)| r - l >= 1.0)
                        .collect()
                }
            })
            .collect();
    }
    pieces
        .into_iter()
        .map(|(left, right)| graphene::Rect::new(left, rect.y(), right - left, rect.height()))
        .collect()
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

    /// Calls `handler` with the loop's start and end when a flag is dragged.
    pub(crate) fn connect_loop_moved(&self, handler: impl Fn(Duration, Duration) + 'static) {
        self.imp().loop_handler.replace(Some(Box::new(handler)));
    }

    /// Covers a whole file of chapters: the playing chapter starts at `offset`,
    /// and `gaps` are the chapter boundaries as fractions of the file. Positions
    /// given to [`set_position`](Self::set_position) and asked for by a seek are
    /// then in the file. No offset and no gaps for a track of its own.
    pub(crate) fn set_chapters(&self, offset: Duration, gaps: Vec<f64>) {
        let imp = self.imp();
        imp.offset.set(offset);
        imp.gaps.replace(gaps);
        self.queue_draw();
    }

    /// Shows the A-B loop's points, as far as they are set.
    pub(crate) fn set_loop(&self, start: Option<Duration>, end: Option<Duration>) {
        let imp = self.imp();
        imp.ab.set((start, end));
        if start.is_none() && end.is_none() {
            imp.flag_drag.set(None);
        }
        self.queue_draw();
    }

    /// The loop's points in the bar's positions (in the file for a chapter).
    fn loop_points(&self) -> (Option<Duration>, Option<Duration>) {
        let imp = self.imp();
        let offset = imp.offset.get();
        let (a, b) = imp.ab.get();
        (a.map(|a| a + offset), b.map(|b| b + offset))
    }

    /// The loop's points, with a dragged flag where it is now.
    fn shown_loop(&self) -> (Option<Duration>, Option<Duration>) {
        let imp = self.imp();
        let (a, b) = self.loop_points();
        match imp.flag_drag.get() {
            Some((false, at)) => (Some(at), b),
            Some((true, at)) => (a, Some(at)),
            None => (a, b),
        }
    }

    /// The loop as fractions of the length, once both points are set.
    fn fractions(&self, a: Option<Duration>, b: Option<Duration>) -> Option<(f64, f64)> {
        let length = self.imp().length.get().filter(|l| !l.is_zero())?;
        let share = |d: Duration| (d.as_secs_f64() / length.as_secs_f64()).clamp(0.0, 1.0);
        let (a, b) = (a?, b?);
        (a < b).then(|| (share(a), share(b)))
    }

    fn flag_x(&self, at: Duration) -> Option<f64> {
        let length = self.imp().length.get().filter(|l| !l.is_zero())?;
        Some((at.as_secs_f64() / length.as_secs_f64()).clamp(0.0, 1.0) * f64::from(self.width()))
    }

    fn draw_flags(&self, snapshot: &gtk::Snapshot, a: Option<Duration>, b: Option<Duration>) {
        let h = self.height() as f32;
        let w = self.width() as f32;
        let tab = crate::style::tokens::metric::r#loop::FLAG_TAB as f32;
        let line = crate::style::tokens::metric::r#loop::FLAG_LINE as f32;
        let flag = glass::LOOP_FLAG.to_gdk();
        let ink = glass::BG.to_gdk();
        for (at, text, after) in [(a, "A", false), (b, "B", true)] {
            let Some(x) = at.and_then(|at| self.flag_x(at)) else {
                continue;
            };
            let x = (x as f32).round().clamp(0.0, (w - line).max(0.0));
            snapshot.append_color(&flag, &graphene::Rect::new(x, 0.0, line, h));
            // A's tab hangs left of its line, B's right; both stay inside.
            let left = if after { x } else { x + line - tab };
            let left = left.clamp(0.0, (w - tab).max(0.0));
            snapshot.append_color(&flag, &graphene::Rect::new(left, 0.0, tab, tab));
            let layout = self.create_pango_layout(Some(text));
            let mut font = gtk::pango::FontDescription::from_string("Monospace Medium");
            font.set_absolute_size(9.0 * f64::from(gtk::pango::SCALE));
            layout.set_font_description(Some(&font));
            let (_, logical) = layout.pixel_extents();
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                left + (tab - logical.width() as f32) / 2.0,
                (tab - logical.height() as f32) / 2.0,
            ));
            snapshot.append_layout(&layout, &ink);
            snapshot.restore();
        }
    }

    /// A press: on a flag it takes the flag, elsewhere it seeks.
    fn press(&self, x: f64, _y: f64) {
        let imp = self.imp();
        let (a, b) = self.loop_points();
        let near = |at: Option<Duration>| {
            at.and_then(|at| self.flag_x(at))
                .map(|fx| (fx - x).abs())
                .filter(|d| *d <= FLAG_REACH)
        };
        let flag = match (near(a), near(b)) {
            (Some(da), Some(db)) => Some(if db < da { (true, b) } else { (false, a) }),
            (Some(_), None) => Some((false, a)),
            (None, Some(_)) => Some((true, b)),
            (None, None) => None,
        };
        match flag {
            Some((is_b, Some(at))) if a.is_some() && b.is_some() => {
                imp.flag_drag.set(Some((is_b, at)));
                self.queue_draw();
            }
            _ => self.drag_to(x, false),
        }
    }

    /// Moves the dragged flag to `x`; on release, asks for the new loop.
    fn move_flag(&self, x: f64, done: bool) {
        let imp = self.imp();
        let (Some((is_b, _)), Some(length)) = (imp.flag_drag.get(), imp.length.get()) else {
            return;
        };
        let width = f64::from(self.width().max(1));
        let at = length.mul_f64((x / width).clamp(0.0, 1.0));
        imp.flag_drag.set(Some((is_b, at)));
        if done {
            let (a, b) = self.shown_loop();
            imp.flag_drag.set(None);
            if let (Some(a), Some(b)) = (a, b) {
                let (start, end) = if a <= b { (a, b) } else { (b, a) };
                let offset = imp.offset.get();
                let (start, end) = (start.saturating_sub(offset), end.saturating_sub(offset));
                if start < end {
                    imp.ab.set((Some(start), Some(end)));
                    if let Some(handler) = imp.loop_handler.borrow().as_ref() {
                        handler(start, end);
                    }
                }
            }
        }
        self.queue_draw();
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
