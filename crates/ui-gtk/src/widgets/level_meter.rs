// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! LevelMeter: one row for each channel of the sound being played. The channel
//! letter; a 10 px bar filled from −60 dB up to the RMS level (300 ms) with a
//! 2 px line at the peak; the RMS value; "PEAK −3.1 dB", or "CLIP" for two
//! seconds after a peak reached full scale. A scale from −60 to 0 dB sits under
//! the last row. No colour carries meaning.
//!
//! Like [`SpectrumView`](super::spectrum_view::SpectrumView) it draws what it is
//! given and moves from the frame clock only while something moves.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk, pango};
use helikodu_model::audio::ChannelPosition;
use helikodu_model::player::AnalyzerFrame;

use super::spectrum_view::{
    Level, Motion, SCALE_FLOOR, animate, clock_seconds, mono, size_of, stop, text_at,
};
use crate::i18n::{fill, tr};
use crate::style::tokens::glass;
use crate::style::tokens::metric::analyzer;

/// How long "CLIP" stays, in seconds.
const CLIP_HOLD: f64 = 2.0;
/// The columns: the letter, the gap after the bar, the RMS value, the peak text.
const LETTER: f32 = 26.0;
const BAR_GAP: f32 = 24.0;
const RMS: f32 = 52.0;
const PEAK: f32 = 104.0;
/// The height of a row and of the scale under them.
const ROW: f32 = 22.0;
const SCALE: f32 = 22.0;
/// The labelled points of the scale, in dB.
const TICKS: [i32; 7] = [-60, -48, -36, -24, -12, -6, 0];
/// How often the accessible value text changes at most, in seconds.
const ACCESSIBLE_EVERY: f64 = 1.0;

/// A channel's row.
#[derive(Debug, Clone, Copy)]
struct Row {
    position: ChannelPosition,
    level: Level,
    /// Until when "CLIP" shows, in seconds of the frame clock.
    clip_until: f64,
}

/// The letter of a channel: L, R, C and LFE for the usual ones, the short
/// speaker name otherwise; M for a single channel.
fn letter(position: ChannelPosition, channels: usize) -> String {
    if channels == 1 {
        return tr("M");
    }
    match position {
        ChannelPosition::FrontLeft => tr("L"),
        ChannelPosition::FrontRight => tr("R"),
        ChannelPosition::FrontCenter => tr("C"),
        other => other.abbreviation().to_owned(),
    }
}

/// A level in dB with one decimal and a real minus sign; "−∞" at the floor.
pub(crate) fn db_text(db: f32) -> String {
    if db <= SCALE_FLOOR {
        return "\u{2212}\u{221e}".to_owned();
    }
    let text = format!("{db:.1}");
    if text == "-0.0" {
        return "0.0".to_owned();
    }
    text.replace('-', "\u{2212}")
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct LevelMeter {
        pub(super) rows: RefCell<Vec<Row>>,
        pub(super) motion: Motion,
        /// When the accessible value text was last set.
        pub(super) told: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LevelMeter {
        const NAME: &'static str = "HkLevelMeter";
        type Type = super::LevelMeter;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("levelmeter");
            klass.set_accessible_role(gtk::AccessibleRole::Meter);
        }
    }

    impl ObjectImpl for LevelMeter {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_hexpand(true);
            obj.update_property(&[
                gtk::accessible::Property::Label(&tr("Level")),
                gtk::accessible::Property::ValueMin(f64::from(SCALE_FLOOR)),
                gtk::accessible::Property::ValueMax(0.0),
            ]);
            obj.set_channels(&[ChannelPosition::FrontLeft, ChannelPosition::FrontRight]);
            super::super::redraw_on_theme_change(&*obj);
        }
    }

    impl WidgetImpl for LevelMeter {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => {
                    let w = (LETTER + 160.0 + BAR_GAP + RMS + PEAK) as i32;
                    (w, w, -1, -1)
                }
                _ => {
                    let rows = self.rows.borrow().len().max(1) as f32;
                    let h = (rows * ROW + SCALE) as i32;
                    (h, h, -1, -1)
                }
            }
        }

        fn unmap(&self) {
            stop(&self.motion);
            for row in self.rows.borrow_mut().iter_mut() {
                row.level = Level::default();
                row.clip_until = 0.0;
            }
            self.parent_unmap();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }
}

glib::wrapper! {
    /// The level of each channel; see the module documentation.
    pub(crate) struct LevelMeter(ObjectSubclass<imp::LevelMeter>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for LevelMeter {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl LevelMeter {
    /// Shows a row for each of `channels`, all silent.
    fn set_channels(&self, channels: &[ChannelPosition]) {
        self.imp().rows.replace(
            channels
                .iter()
                .map(|&position| Row {
                    position,
                    level: Level::default(),
                    clip_until: 0.0,
                })
                .collect(),
        );
        self.queue_resize();
    }

    /// Shows the channel levels of `frame`.
    pub(crate) fn show(&self, frame: &AnalyzerFrame) {
        let positions: Vec<ChannelPosition> = frame.channels.iter().map(|c| c.position).collect();
        let same = {
            let rows = self.imp().rows.borrow();
            rows.len() == positions.len()
                && rows.iter().zip(&positions).all(|(r, p)| r.position == *p)
        };
        if !same && !positions.is_empty() {
            self.set_channels(&positions);
        }
        let reduced = crate::style::current().reduced_motion;
        let now = clock_seconds(self);
        for (row, channel) in self.imp().rows.borrow_mut().iter_mut().zip(&frame.channels) {
            row.level.set(channel.rms_db, now, reduced);
            row.level.raise_peak(channel.peak_db, now);
            if channel.peak_db >= -0.001 {
                row.clip_until = now + CLIP_HOLD;
            }
        }
        self.tell(now, !frame.live);
        self.queue_draw();
        animate(
            self,
            |w: &Self| &w.imp().motion,
            |w: &Self, now: f64, dt: f32| {
                let reduced = crate::style::current().reduced_motion;
                let mut moving = false;
                for row in w.imp().rows.borrow_mut().iter_mut() {
                    moving |= row.level.step(now, dt, reduced);
                    moving |= row.clip_until > now;
                }
                moving
            },
        );
    }

    /// Sets the accessible value text, at most once a second unless `now_anyway`.
    fn tell(&self, now: f64, now_anyway: bool) {
        let imp = self.imp();
        if !now_anyway && now - imp.told.get() < ACCESSIBLE_EVERY {
            return;
        }
        imp.told.set(now);
        let count = imp.rows.borrow().len();
        let text = self
            .rows_text()
            .iter()
            .zip(imp.rows.borrow().iter())
            .map(|((rms, peak), row)| {
                fill(
                    &tr("{}: {} dB, {}"),
                    &[&letter(row.position, count), rms, peak],
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let loudest = imp
            .rows
            .borrow()
            .iter()
            .map(|r| r.level.shown)
            .fold(SCALE_FLOOR, f32::max);
        self.update_property(&[
            gtk::accessible::Property::ValueNow(f64::from(loudest)),
            gtk::accessible::Property::ValueText(&text),
        ]);
    }

    /// The RMS and peak texts of each row as shown.
    pub(crate) fn rows_text(&self) -> Vec<(String, String)> {
        let now = clock_seconds(self);
        self.imp()
            .rows
            .borrow()
            .iter()
            .map(|row| {
                let peak = if row.clip_until > now {
                    tr("CLIP")
                } else {
                    fill(&tr("PEAK {} dB"), &[&db_text(row.level.peak)])
                };
                (db_text(row.level.shown), peak)
            })
            .collect()
    }

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let widget = self.upcast_ref::<gtk::Widget>();
        let w = self.width() as f32;
        let bar_x = LETTER;
        let bar_w = w - LETTER - BAR_GAP - RMS - PEAK;
        if bar_w <= 0.0 {
            return;
        }
        let x_of = |db: f32| bar_x + (1.0 - db.clamp(SCALE_FLOOR, 0.0) / SCALE_FLOOR) * bar_w;
        let thick = analyzer::METER_BAR as f32;
        let texts = self.rows_text();
        let rows = self.imp().rows.borrow();
        let count = rows.len();
        let now = clock_seconds(self);
        for (i, (row, (rms, peak))) in rows.iter().zip(&texts).enumerate() {
            let y = i as f32 * ROW;
            let mid = y + ROW / 2.0;
            let letter = mono(
                widget,
                &letter(row.position, count),
                10.5,
                pango::Weight::Normal,
            );
            let (_, lh) = size_of(&letter);
            text_at(snapshot, &letter, 0.0, mid - lh / 2.0, glass::T2);

            let track = graphene::Rect::new(bar_x, mid - thick / 2.0, bar_w, thick);
            let rounded = gsk::RoundedRect::from_rect(track, 2.0);
            snapshot.push_rounded_clip(&rounded);
            snapshot.append_color(&glass::ANALYZER_GRID.to_gdk(), &track);
            if row.level.shown > SCALE_FLOOR {
                let end = x_of(row.level.shown);
                snapshot.append_color(
                    &glass::METER_FILL.to_gdk(),
                    &graphene::Rect::new(bar_x, track.y(), end - bar_x, thick),
                );
            }
            if row.level.peak > SCALE_FLOOR {
                let line = analyzer::PEAK_LINE as f32;
                let x = (x_of(row.level.peak) - line).max(bar_x);
                snapshot.append_color(
                    &glass::METER_PEAK.to_gdk(),
                    &graphene::Rect::new(x, track.y(), line, thick),
                );
            }
            snapshot.pop();

            let value = mono(widget, rms, 11.5, pango::Weight::Medium);
            let (vw, vh) = size_of(&value);
            text_at(
                snapshot,
                &value,
                bar_x + bar_w + BAR_GAP + RMS - vw,
                mid - vh / 2.0,
                glass::T1,
            );
            let clipped = row.clip_until > now;
            let peak_layout = mono(
                widget,
                peak,
                10.5,
                if clipped {
                    pango::Weight::Semibold
                } else {
                    pango::Weight::Normal
                },
            );
            let (pw, ph) = size_of(&peak_layout);
            text_at(
                snapshot,
                &peak_layout,
                w - pw,
                mid - ph / 2.0,
                if clipped { glass::T1 } else { glass::T2 },
            );
        }

        // The scale.
        let y = count as f32 * ROW + 4.0;
        for db in TICKS {
            let text = if db == 0 {
                "0".to_owned()
            } else {
                format!("\u{2212}{}", -db)
            };
            let layout = mono(widget, &text, 10.5, pango::Weight::Normal);
            let (tw, _) = size_of(&layout);
            let x = (x_of(db as f32) - tw / 2.0).clamp(bar_x, bar_x + bar_w - tw / 2.0);
            text_at(snapshot, &layout, x, y, glass::T4);
        }
    }

    /// Whether the frame clock drives the widget now, for tests.
    pub(crate) fn is_moving(&self) -> bool {
        self.imp().motion.is_running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_read_as_text() {
        assert_eq!(db_text(-14.24), "\u{2212}14.2");
        assert_eq!(db_text(-0.01), "0.0");
        assert_eq!(db_text(SCALE_FLOOR), "\u{2212}\u{221e}");
        assert_eq!(letter(ChannelPosition::LowFrequency, 6), "LFE");
        assert_eq!(letter(ChannelPosition::FrontCenter, 1), "M");
    }
}
