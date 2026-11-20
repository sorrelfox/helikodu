// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! LoudnessCurve: how loud a track is over its length, from its stored
//! analysis, drawn like the waveform of the seek bar: one bar for every two
//! seconds, 1 px apart, in `t3`; silent stretches as bars of 2 px in `line2`,
//! and a dashed 1 px `t2` line where the fade-out starts. Bars 30 px high at
//! most; the line reaches 3 px above them.
//!
//! It is a picture: its accessible name says what it shows. It draws only when
//! its size or the look changes.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

/// The height of the tallest bar.
const BARS: f32 = 30.0;
/// How far the fade-out line reaches above the bars.
const OVER: f32 = 3.0;
/// The gap between bars.
const GAP: f32 = 1.0;
/// The narrowest bar; with more bars than fit, neighbours are joined.
const NARROWEST: f32 = 1.0;
/// The lowest bar of sound, and the bar of silence.
const LOWEST: f32 = 3.0;
const SILENT: f32 = 2.0;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct LoudnessCurve {
        /// Each bar's height as a share of the tallest; `None` for silence.
        pub(super) bars: RefCell<Vec<Option<f32>>>,
        /// Where the fade-out starts, as a share of the width.
        pub(super) fade: Cell<Option<f32>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LoudnessCurve {
        const NAME: &'static str = "HkLoudnessCurve";
        type Type = super::LoudnessCurve;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("loudnesscurve");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for LoudnessCurve {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_hexpand(true);
            super::super::redraw_on_theme_change(&*obj);
        }
    }

    impl WidgetImpl for LoudnessCurve {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (0, 0, -1, -1),
                _ => {
                    let h = (BARS + OVER) as i32;
                    (h, h, -1, -1)
                }
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            let bars = self.bars.borrow();
            let shown = joined(&bars, w);
            if shown.is_empty() || w <= 0.0 {
                return;
            }
            let palette = crate::style::current().palette;
            let sound = palette.t3.to_gdk();
            let silence = palette.line2.to_gdk();
            let pitch = (w + GAP) / shown.len() as f32;
            for (i, bar) in shown.iter().enumerate() {
                let x = (i as f32 * pitch).round();
                let width = (((i + 1) as f32 * pitch - GAP).round() - x).max(NARROWEST);
                let (height, color) = match bar {
                    Some(share) => ((share * BARS).round().clamp(LOWEST, BARS), &sound),
                    None => (SILENT, &silence),
                };
                let rect = graphene::Rect::new(x, h - height, width, height);
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(rect, 1.0));
                snapshot.append_color(color, &rect);
                snapshot.pop();
            }
            if let Some(fade) = self.fade.get() {
                let x = (fade * w).round().clamp(0.0, (w - 1.0).max(0.0));
                let line = palette.t2.to_gdk();
                let mut y = 0.0;
                while y < h {
                    let dash = 3.0_f32.min(h - y);
                    snapshot.append_color(&line, &graphene::Rect::new(x, y, 1.0, dash));
                    y += 5.0;
                }
            }
        }
    }
}

glib::wrapper! {
    /// The loudness curve of the File info tab.
    pub(crate) struct LoudnessCurve(ObjectSubclass<imp::LoudnessCurve>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for LoudnessCurve {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl LoudnessCurve {
    /// Shows `bars` (each a share of the tallest, `None` for silence) with the
    /// fade-out at `fade` (a share of the width); `name` says what it shows.
    pub(crate) fn set_curve(&self, bars: Vec<Option<f32>>, fade: Option<f32>, name: &str) {
        self.imp().bars.replace(bars);
        self.imp().fade.set(fade);
        self.update_property(&[gtk::accessible::Property::Label(name)]);
        self.queue_draw();
    }
}

/// `bars` joined in neighbours, the loudest of each, so that each is at least
/// [`NARROWEST`] wide in `width`; silence only where all were silent.
fn joined(bars: &[Option<f32>], width: f32) -> Vec<Option<f32>> {
    let fit = ((width + GAP) / (NARROWEST + GAP)).floor().max(1.0) as usize;
    if bars.len() <= fit {
        return bars.to_vec();
    }
    (0..fit)
        .map(|i| {
            let from = i * bars.len() / fit;
            let to = ((i + 1) * bars.len() / fit).max(from + 1);
            bars[from..to.min(bars.len())]
                .iter()
                .flatten()
                .copied()
                .reduce(f32::max)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_are_joined_only_when_they_do_not_fit() {
        let bars = vec![Some(0.5), None, Some(1.0), None];
        assert_eq!(joined(&bars, 100.0), bars);
        // Two fit in 3 px: the loudest of each pair, silence only if both are.
        assert_eq!(joined(&bars, 3.0), vec![Some(0.5), Some(1.0)]);
        assert_eq!(joined(&[None, None], 1.0), vec![None]);
    }
}
