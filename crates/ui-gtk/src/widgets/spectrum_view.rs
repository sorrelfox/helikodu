// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! SpectrumView: the analyzer's spectrum on glass. Bars on a logarithmic axis
//! from 25 Hz to 20 kHz, 3 px apart, in the accent fill, each with a 2 px line at
//! its recent peak; lines at 0, −20, −40 and −60 dB with their labels at the
//! left, and the frequencies under the plot.
//!
//! The widget draws the band levels it is given ([`SpectrumView::show`]); it
//! never sees samples. A bar rises at once and falls back smoothly; a peak line
//! holds for 1.5 s and then falls. Both run from the frame clock and only while
//! something moves: once everything has come to rest no frame is drawn. With
//! reduced motion the bars and the peak lines jump instead of falling.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, pango};
use helikodu_model::player::{AnalyzerFrame, SPECTRUM_BANDS, axis_fraction};

use crate::i18n::tr;
use crate::style::color::Rgba;
use crate::style::tokens::glass;
use crate::style::tokens::metric::analyzer;

/// The bottom of the scales, in dB.
pub(crate) const SCALE_FLOOR: f32 = -(analyzer::DB_RANGE as f32);
/// How fast a bar falls back, in dB a second.
const FALL: f32 = 40.0;
/// How fast a peak line falls once its hold is over, in dB a second.
const PEAK_FALL: f32 = 24.0;
/// How long a peak line holds, in seconds.
const PEAK_HOLD: f64 = analyzer::PEAK_HOLD_SECONDS;

/// One bar or meter: what it shows, where it is going and its peak line.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Level {
    pub(crate) shown: f32,
    target: f32,
    pub(crate) peak: f32,
    /// When the peak was reached, in seconds of the frame clock.
    peak_at: f64,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            shown: SCALE_FLOOR,
            target: SCALE_FLOOR,
            peak: SCALE_FLOOR,
            peak_at: 0.0,
        }
    }
}

impl Level {
    /// Takes a new value: the bar rises to it at once (with reduced motion it
    /// also falls at once), the peak line follows it up.
    pub(crate) fn set(&mut self, value: f32, now: f64, reduced: bool) {
        let value = value.max(SCALE_FLOOR);
        self.target = value;
        if value >= self.shown || reduced {
            self.shown = value;
        }
        if value >= self.peak {
            self.peak = value;
            self.peak_at = now;
        }
    }

    /// Raises the peak line to `value` if it is higher (a meter's peak is not its
    /// bar's value).
    pub(crate) fn raise_peak(&mut self, value: f32, now: f64) {
        if value >= self.peak {
            self.peak = value.max(SCALE_FLOOR);
            self.peak_at = now;
        }
    }

    /// Moves on by `dt` seconds; returns whether it still moves.
    pub(crate) fn step(&mut self, now: f64, dt: f32, reduced: bool) -> bool {
        if self.shown > self.target {
            self.shown = if reduced {
                self.target
            } else {
                (self.shown - FALL * dt).max(self.target)
            };
        }
        if self.peak > self.shown && now - self.peak_at >= PEAK_HOLD {
            self.peak = if reduced {
                self.shown
            } else {
                (self.peak - PEAK_FALL * dt).max(self.shown)
            };
        }
        self.shown > self.target || self.peak > self.shown
    }
}

/// Animates a widget's levels from its frame clock, only while they move.
#[derive(Default)]
pub(crate) struct Motion {
    tick: RefCell<Option<gtk::TickCallbackId>>,
    last: Cell<Option<f64>>,
}

impl std::fmt::Debug for Motion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Motion")
            .field("running", &self.tick.borrow().is_some())
            .finish()
    }
}

impl Motion {
    /// Whether the frame clock drives the widget now.
    pub(crate) fn is_running(&self) -> bool {
        self.tick.borrow().is_some()
    }
}

/// The frame clock's time of `widget` in seconds.
pub(crate) fn clock_seconds(widget: &impl IsA<gtk::Widget>) -> f64 {
    let micros = widget
        .frame_clock()
        .map_or_else(glib::monotonic_time, |clock| clock.frame_time());
    micros as f64 / 1e6
}

/// Starts the frame-clock callback of `widget` unless it runs; `step` is called
/// with the time and the seconds since the last frame, and returns whether
/// anything still moves.
pub(crate) fn animate<W: IsA<gtk::Widget>>(
    widget: &W,
    motion: impl Fn(&W) -> &Motion + 'static,
    step: impl Fn(&W, f64, f32) -> bool + 'static,
) {
    let held = motion(widget);
    if held.tick.borrow().is_some() || !widget.is_mapped() {
        return;
    }
    held.last.set(None);
    let id = widget.add_tick_callback(move |widget, clock| {
        let now = clock.frame_time() as f64 / 1e6;
        let held = motion(widget);
        let dt = held
            .last
            .replace(Some(now))
            .map_or(0.0, |last| (now - last) as f32);
        let moving = step(widget, now, dt);
        widget.queue_draw();
        if moving {
            glib::ControlFlow::Continue
        } else {
            held.tick.replace(None);
            glib::ControlFlow::Break
        }
    });
    held.tick.replace(Some(id));
}

/// Stops the frame-clock callback, if it runs.
pub(crate) fn stop(motion: &Motion) {
    if let Some(id) = motion.tick.take() {
        id.remove();
    }
}

/// A monospace layout of `text` at `px` pixels.
pub(crate) fn mono(
    widget: &gtk::Widget,
    text: &str,
    px: f64,
    weight: pango::Weight,
) -> pango::Layout {
    let layout = widget.create_pango_layout(Some(text));
    let mut font = pango::FontDescription::new();
    font.set_family("monospace");
    font.set_weight(weight);
    font.set_absolute_size(px * f64::from(pango::SCALE));
    layout.set_font_description(Some(&font));
    layout
}

/// Draws `layout` with its top left at `x`, `y`.
pub(crate) fn text_at(
    snapshot: &gtk::Snapshot,
    layout: &pango::Layout,
    x: f32,
    y: f32,
    color: Rgba,
) {
    snapshot.save();
    snapshot.translate(&graphene::Point::new(x, y));
    snapshot.append_layout(layout, &color.to_gdk());
    snapshot.restore();
}

/// The width and height of `layout` in pixels.
pub(crate) fn size_of(layout: &pango::Layout) -> (f32, f32) {
    let (_, logical) = layout.pixel_extents();
    (logical.width() as f32, logical.height() as f32)
}

/// The frequencies labelled under the plot, in Hz.
const LABELS: [(f32, &str); 10] = [
    (31.5, "31"),
    (63.0, "63"),
    (125.0, "125"),
    (250.0, "250"),
    (500.0, "500"),
    (1_000.0, "1k"),
    (2_000.0, "2k"),
    (4_000.0, "4k"),
    (8_000.0, "8k"),
    (16_000.0, "16k"),
];

/// The space above the plot (for the top label) and under it (for the
/// frequencies).
const TOP: f32 = 7.0;
const BOTTOM: f32 = 22.0;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct SpectrumView {
        pub(super) bands: RefCell<Vec<Level>>,
        pub(super) motion: Motion,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SpectrumView {
        const NAME: &'static str = "HkSpectrumView";
        type Type = super::SpectrumView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("spectrumview");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for SpectrumView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_hexpand(true);
            obj.update_property(&[gtk::accessible::Property::Label(&tr(
                "Spectrum of the sound being played",
            ))]);
            self.bands.replace(vec![Level::default(); SPECTRUM_BANDS]);
            super::super::redraw_on_theme_change(&*obj);
        }
    }

    impl WidgetImpl for SpectrumView {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (analyzer::AXIS_COLUMN + 240, 600, -1, -1),
                _ => {
                    let h = analyzer::SPECTRUM_HEIGHT;
                    (h, h, -1, -1)
                }
            }
        }

        fn unmap(&self) {
            // Hidden, it starts again from silence.
            stop(&self.motion);
            self.bands.replace(vec![Level::default(); SPECTRUM_BANDS]);
            self.parent_unmap();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }
}

glib::wrapper! {
    /// The analyzer's spectrum; see the module documentation.
    pub(crate) struct SpectrumView(ObjectSubclass<imp::SpectrumView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for SpectrumView {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SpectrumView {
    /// Shows the band levels of `frame`.
    pub(crate) fn show(&self, frame: &AnalyzerFrame) {
        let reduced = crate::style::current().reduced_motion;
        let now = clock_seconds(self);
        {
            let mut bands = self.imp().bands.borrow_mut();
            for (level, &value) in bands.iter_mut().zip(&frame.bands) {
                level.set(value, now, reduced);
            }
        }
        self.queue_draw();
        animate(
            self,
            |w: &Self| &w.imp().motion,
            move |w: &Self, now: f64, dt: f32| {
                let reduced = crate::style::current().reduced_motion;
                let mut moving = false;
                for level in w.imp().bands.borrow_mut().iter_mut() {
                    moving |= level.step(now, dt, reduced);
                }
                moving
            },
        );
    }

    /// The bars and peak lines as drawn, in dB, for tests.
    pub(crate) fn levels(&self) -> Vec<(f32, f32)> {
        self.imp()
            .bands
            .borrow()
            .iter()
            .map(|l| (l.shown, l.peak))
            .collect()
    }

    /// Whether the frame clock drives the widget now, for tests.
    pub(crate) fn is_moving(&self) -> bool {
        self.imp().motion.is_running()
    }

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let widget = self.upcast_ref::<gtk::Widget>();
        let (w, h) = (self.width() as f32, self.height() as f32);
        let left = analyzer::AXIS_COLUMN as f32;
        let (top, bottom) = (TOP, h - BOTTOM);
        let plot_w = w - left;
        let plot_h = bottom - top;
        if plot_w <= 0.0 || plot_h <= 0.0 {
            return;
        }
        let y_of = |db: f32| top + (db.clamp(SCALE_FLOOR, 0.0) / SCALE_FLOOR) * plot_h;

        // The lines and their labels.
        for db in [0, -20, -40, -60] {
            let y = y_of(db as f32).round();
            snapshot.append_color(
                &glass::ANALYZER_GRID.to_gdk(),
                &graphene::Rect::new(left, y, plot_w, 1.0),
            );
            let text = if db == 0 {
                tr("0 dB")
            } else {
                format!("\u{2212}{}", -db)
            };
            let layout = mono(widget, &text, 10.5, pango::Weight::Normal);
            let (_, th) = size_of(&layout);
            text_at(snapshot, &layout, 10.0, y - th / 2.0, glass::T4);
        }

        // The bars and their peak lines.
        let gap = analyzer::BAND_GAP as f32;
        let count = SPECTRUM_BANDS as f32;
        let bar = ((plot_w - gap * (count - 1.0)) / count).max(1.0);
        let accent = crate::style::current().accent.glass_fill.to_gdk();
        let peak_color = glass::METER_PEAK.to_gdk();
        let line = analyzer::PEAK_LINE as f32;
        for (i, level) in self.imp().bands.borrow().iter().enumerate() {
            let x = left + i as f32 * (bar + gap);
            if level.shown > SCALE_FLOOR {
                let y = y_of(level.shown);
                snapshot.append_color(&accent, &graphene::Rect::new(x, y, bar, bottom - y));
            }
            if level.peak > SCALE_FLOOR {
                let y = y_of(level.peak);
                snapshot.append_color(
                    &peak_color,
                    &graphene::Rect::new(x, (y - line - 1.0).max(top - line), bar, line),
                );
            }
        }

        // The frequencies.
        for (hz, text) in LABELS {
            let layout = mono(widget, text, 10.5, pango::Weight::Normal);
            let (tw, _) = size_of(&layout);
            let x = left + axis_fraction(hz) * plot_w - tw / 2.0;
            let x = x.clamp(left, w - tw);
            text_at(snapshot, &layout, x, bottom + 6.0, glass::T4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_rise_at_once_and_fall_back() {
        let mut level = Level::default();
        level.set(-10.0, 0.0, false);
        assert_eq!((level.shown, level.peak), (-10.0, -10.0));
        level.set(-40.0, 0.1, false);
        assert_eq!(level.shown, -10.0, "falls back, not at once");
        assert!(level.step(0.2, 0.25, false));
        assert_eq!(level.shown, -20.0);
        assert_eq!(level.peak, -10.0, "the peak holds");
        // Once the hold is over, the peak falls to the bar; then nothing moves.
        let mut now = 0.2;
        while level.step(now, 0.1, false) {
            now += 0.1;
            assert!(now < 10.0);
        }
        assert_eq!(level.shown, -40.0);
        assert_eq!(level.peak, -40.0);
        assert!(now > PEAK_HOLD);

        // With reduced motion, the bar jumps; the peak jumps after its hold.
        let mut level = Level::default();
        level.set(-10.0, 0.0, true);
        level.set(-40.0, 0.1, true);
        assert_eq!(level.shown, -40.0);
        assert!(level.step(1.0, 0.1, true));
        assert_eq!(level.peak, -10.0);
        assert!(!level.step(1.6, 0.1, true));
        assert_eq!(level.peak, -40.0);
    }
}
