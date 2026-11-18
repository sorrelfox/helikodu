// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! EqGraph: the response of the equalizer on glass.
//!
//! X is logarithmic from 20 Hz to 20 kHz, Y is ±12 dB. The curve is the summed
//! response of the equalizer's sections, computed with the same functions the
//! engine builds its filters with (`helikodu_model::eq`), 2 px in the accent with
//! a 14 % fill to the zero line.
//!
//! In parametric mode (232 high) each enabled or disabled filter has an 18 px
//! numbered handle on the curve at its frequency; disabled ones are grey.
//!
//! In graphic mode (372 high) each band has a vertical track and a round handle
//! that is its fader: a focusable child of role slider, named by its frequency.
//! Up and Down change it by 0.5 dB, Page Up and Page Down by 3 dB, 0 or a
//! double-click return it to 0 dB, Left and Right move to the neighbours.
//! Dragging sets the band under the pointer; with 31 bands a drag across the
//! tracks sets each band it passes.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk, pango};
use helikodu_model::eq::{self, Coefficients};
use helikodu_model::settings::{
    EqMode, EqualizerSettings, GRAPHIC_10_BANDS_HZ, GRAPHIC_31_BANDS_HZ,
};

use crate::i18n::{fill, tr};
use crate::style::color::Rgba;
use crate::style::tokens::glass;

/// The rate the response is drawn for.
const RATE: f64 = 48_000.0;
/// The lowest and highest frequency of the X axis.
const LOW_HZ: f64 = 20.0;
const HIGH_HZ: f64 = 20_000.0;
/// The range of the Y axis, in dB either way.
const RANGE_DB: f64 = 12.0;
/// The step of a band's gain, in dB.
const STEP_DB: f64 = 0.5;
/// The grid lines of the X axis.
const GRID_HZ: [f64; 10] = [
    20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0,
];

/// A filter handle in parametric mode.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FilterMark {
    hz: f64,
    enabled: bool,
}

type BandListener = Box<dyn Fn(usize, f64)>;

/// A frequency as the graph labels it: `31.5`, `500`, `1.25k`, `16k`.
pub(crate) fn short_hz(hz: f64) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        let text = format!("{k:.2}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        format!("{text}k")
    } else {
        let text = format!("{hz:.1}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// A frequency in words for people and assistive technology: `500 Hz`,
/// `1.25 kHz`.
pub(crate) fn hz_text(hz: f64) -> String {
    if hz >= 1000.0 {
        let k = format!("{:.2}", hz / 1000.0);
        let k = k.trim_end_matches('0').trim_end_matches('.');
        fill(&tr("{} kHz"), &[k])
    } else {
        let text = format!("{hz:.1}");
        fill(
            &tr("{} Hz"),
            &[text.trim_end_matches('0').trim_end_matches('.')],
        )
    }
}

/// A gain with its sign, without a unit: `+4.5`, `−2.0`, `0.0`.
pub(crate) fn signed(db: f64) -> String {
    if db > 0.0 {
        format!("+{db:.1}")
    } else if db < 0.0 {
        format!("−{:.1}", -db)
    } else {
        "0.0".into()
    }
}

mod handle {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct BandHandle {
        pub(super) index: Cell<usize>,
        pub(super) size: Cell<f32>,
        pub(super) selected: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BandHandle {
        const NAME: &'static str = "HkEqBandHandle";
        type Type = super::BandHandle;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Slider);
        }
    }

    impl ObjectImpl for BandHandle {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_focus_on_click(true);
            obj.add_css_class("hk-eq-handle");
        }
    }

    impl WidgetImpl for BandHandle {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            let side = self.size.get().ceil() as i32 + 6;
            (side, side, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let look = crate::style::current();
            let accent = look.accent.raw;
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            let side = self.size.get();
            let rect = graphene::Rect::new((w - side) / 2.0, (h - side) / 2.0, side, side);
            let round = gsk::RoundedRect::from_rect(rect, side / 2.0);
            let fill = if self.selected.get() {
                accent
            } else {
                glass::BG
            };
            snapshot.push_rounded_clip(&round);
            snapshot.append_color(&fill.to_gdk(), &rect);
            snapshot.pop();
            snapshot.append_border(&round, &[1.5; 4], &[accent.to_gdk(); 4]);
            if obj.has_visible_focus() {
                let outer = rect.inset_r(-3.0, -3.0);
                snapshot.append_border(
                    &gsk::RoundedRect::from_rect(outer, side / 2.0 + 3.0),
                    &[2.0; 4],
                    &[look.accent.text.to_gdk(); 4],
                );
            }
        }
    }
}

glib::wrapper! {
    /// The round handle of one band: its fader.
    pub(crate) struct BandHandle(ObjectSubclass<handle::BandHandle>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub(crate) struct EqGraph {
        /// The equalizer shown.
        pub(super) settings: RefCell<EqualizerSettings>,
        /// Its sections at [`RATE`].
        pub(super) sections: RefCell<Vec<Coefficients>>,
        /// The parametric filters' handles.
        pub(super) marks: RefCell<Vec<FilterMark>>,
        /// The graphic bands' handles.
        pub(super) handles: RefCell<Vec<super::BandHandle>>,
        pub(super) selected: Cell<usize>,
        /// The band the last drag step set, for strokes across the tracks.
        pub(super) dragged: Cell<Option<usize>>,
        pub(super) listeners: RefCell<Vec<BandListener>>,
    }

    impl std::fmt::Debug for EqGraph {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("EqGraph")
                .field("mode", &self.settings.borrow().mode)
                .finish_non_exhaustive()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EqGraph {
        const NAME: &'static str = "HkEqGraph";
        type Type = super::EqGraph;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Group);
        }
    }

    impl ObjectImpl for EqGraph {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("hk-eq-graph");
            obj.set_overflow(gtk::Overflow::Hidden);
            super::super::redraw_on_theme_change(&*obj);

            let drag = gtk::GestureDrag::new();
            drag.connect_drag_begin(|gesture, x, y| {
                let Some(graph) = gesture.widget().and_downcast::<super::EqGraph>() else {
                    return;
                };
                if !graph.is_graphic() {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                let band = graph.band_at(x);
                graph.select(band, true);
                graph.imp().dragged.set(Some(band));
                graph.set_band_from_y(band, y);
            });
            drag.connect_drag_update(|gesture, dx, dy| {
                let (Some(graph), Some((x, y))) = (
                    gesture.widget().and_downcast::<super::EqGraph>(),
                    gesture.start_point(),
                ) else {
                    return;
                };
                let imp = graph.imp();
                let Some(last) = imp.dragged.get() else {
                    return;
                };
                let (x, y) = (x + dx, y + dy);
                if imp.handles.borrow().len() > 10 {
                    // A stroke across the tracks sets every band it passes.
                    let band = graph.band_at(x);
                    let (from, to) = if band < last {
                        (band, last)
                    } else {
                        (last, band)
                    };
                    for b in from..=to {
                        graph.set_band_from_y(b, y);
                    }
                    imp.dragged.set(Some(band));
                    graph.select(band, true);
                } else {
                    graph.set_band_from_y(last, y);
                }
            });
            drag.connect_drag_end(|gesture, _, _| {
                if let Some(graph) = gesture.widget().and_downcast::<super::EqGraph>() {
                    graph.imp().dragged.set(None);
                }
            });
            obj.add_controller(drag);

            let click = gtk::GestureClick::new();
            click.connect_pressed(|gesture, n, x, _| {
                let Some(graph) = gesture.widget().and_downcast::<super::EqGraph>() else {
                    return;
                };
                if n == 2 && graph.is_graphic() {
                    let band = graph.band_at(x);
                    graph.change_band(band, 0.0);
                }
            });
            obj.add_controller(click);
        }

        fn dispose(&self) {
            for handle in self.handles.borrow_mut().drain(..) {
                handle.unparent();
            }
        }
    }

    impl WidgetImpl for EqGraph {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (360, 760, -1, -1),
                _ => {
                    let h = if self.obj().is_graphic() { 372 } else { 232 };
                    (h, h, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            let obj = self.obj();
            let settings = self.settings.borrow();
            let Some(gains) = settings.graphic_gains() else {
                return;
            };
            let centres = obj.centres();
            for (i, handle) in self.handles.borrow().iter().enumerate() {
                let (side, _, _, _) = handle.measure(gtk::Orientation::Horizontal, -1);
                let hz = centres.get(i).copied().unwrap_or(1000.0);
                let gain = gains.get(i).copied().unwrap_or(0.0);
                let x = obj.x_of(hz) - side as f32 / 2.0;
                let y = obj.y_of(gain) - side as f32 / 2.0;
                handle.size_allocate(
                    &gdk::Rectangle::new(x.round() as i32, y.round() as i32, side, side),
                    -1,
                );
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            obj.draw(snapshot);
            for handle in self.handles.borrow().iter() {
                obj.snapshot_child(handle, snapshot);
            }
        }
    }
}

glib::wrapper! {
    /// The equalizer's response with its handles.
    pub(crate) struct EqGraph(ObjectSubclass<imp::EqGraph>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for EqGraph {
    fn default() -> Self {
        glib::Object::new()
    }
}

/// The plot area's insets: left, right, top, bottom.
fn insets(graphic: bool) -> (f32, f32, f32, f32) {
    if graphic {
        (44.0, 16.0, 18.0, 54.0)
    } else {
        (44.0, 16.0, 14.0, 26.0)
    }
}

fn mono_layout(widget: &gtk::Widget, text: &str, px: f64, weight: pango::Weight) -> pango::Layout {
    let layout = widget.create_pango_layout(Some(text));
    let mut font = pango::FontDescription::new();
    font.set_family("monospace");
    font.set_weight(weight);
    font.set_absolute_size(px * f64::from(pango::SCALE));
    layout.set_font_description(Some(&font));
    layout
}

impl EqGraph {
    /// Shows `settings`: the mode, the filters or band gains, the curve.
    pub(crate) fn set_settings(&self, settings: &EqualizerSettings) {
        let imp = self.imp();
        let was_graphic = self.is_graphic();
        let bands_before = imp.handles.borrow().len();
        *imp.settings.borrow_mut() = settings.clone();
        *imp.sections.borrow_mut() = eq::sections(settings, RATE);
        *imp.marks.borrow_mut() = settings
            .parametric
            .iter()
            .map(|f| FilterMark {
                hz: f.frequency_hz,
                enabled: f.enabled,
            })
            .collect();
        let bands = settings.graphic_gains().map_or(0, <[f64]>::len);
        if bands != bands_before {
            for handle in imp.handles.borrow_mut().drain(..) {
                handle.unparent();
            }
            let centres = self.centres();
            let mut handles = Vec::with_capacity(bands);
            for (i, hz) in centres.iter().enumerate() {
                let handle: BandHandle = glib::Object::new();
                handle.imp().index.set(i);
                handle.set_widget_name(&format!("eq-band-{i}"));
                handle.imp().size.set(if bands > 10 { 12.0 } else { 14.0 });
                handle.set_parent(self);
                handle.update_property(&[gtk::accessible::Property::Label(&fill(
                    &tr("Band {}"),
                    &[&hz_text(*hz)],
                ))]);
                self.connect_handle(&handle);
                handles.push(handle);
            }
            *imp.handles.borrow_mut() = handles;
            // The band at 500 Hz is selected first.
            let first = centres.iter().position(|hz| *hz >= 500.0).unwrap_or(0);
            imp.selected.set(first.min(bands.saturating_sub(1)));
        }
        if let Some(gains) = settings.graphic_gains() {
            let selected = imp.selected.get();
            for (i, handle) in imp.handles.borrow().iter().enumerate() {
                let gain = gains.get(i).copied().unwrap_or(0.0);
                handle.imp().selected.set(i == selected);
                handle.update_property(&[
                    gtk::accessible::Property::ValueMin(-RANGE_DB),
                    gtk::accessible::Property::ValueMax(RANGE_DB),
                    gtk::accessible::Property::ValueNow(gain),
                    gtk::accessible::Property::ValueText(&fill(&tr("{} dB"), &[&signed(gain)])),
                ]);
                handle.queue_draw();
            }
        }
        if was_graphic != self.is_graphic() {
            self.queue_resize();
        } else {
            self.queue_allocate();
        }
        self.queue_draw();
    }

    /// Calls `listener` with each band and gain the user sets.
    pub(crate) fn connect_band_changed(&self, listener: impl Fn(usize, f64) + 'static) {
        self.imp().listeners.borrow_mut().push(Box::new(listener));
    }

    /// The equalizer as the graph shows it, with the user's latest changes.
    pub(crate) fn settings(&self) -> EqualizerSettings {
        self.imp().settings.borrow().clone()
    }

    fn is_graphic(&self) -> bool {
        self.imp().settings.borrow().mode != EqMode::Parametric
    }

    fn centres(&self) -> &'static [f64] {
        match self.imp().settings.borrow().mode {
            EqMode::Graphic31 => &GRAPHIC_31_BANDS_HZ,
            _ => &GRAPHIC_10_BANDS_HZ,
        }
    }

    fn gain(&self, band: usize) -> f64 {
        self.imp()
            .settings
            .borrow()
            .graphic_gains()
            .and_then(|g| g.get(band).copied())
            .unwrap_or(0.0)
    }

    fn plot(&self) -> (f32, f32, f32, f32) {
        let (l, r, t, b) = insets(self.is_graphic());
        let (w, h) = (self.width() as f32, self.height() as f32);
        (l, t, (w - l - r).max(1.0), (h - t - b).max(1.0))
    }

    fn x_of(&self, hz: f64) -> f32 {
        let (left, _, width, _) = self.plot();
        let share = (hz / LOW_HZ).log10() / (HIGH_HZ / LOW_HZ).log10();
        left + width * share.clamp(0.0, 1.0) as f32
    }

    fn y_of(&self, db: f64) -> f32 {
        let (_, top, _, height) = self.plot();
        let share = (RANGE_DB - db.clamp(-RANGE_DB, RANGE_DB)) / (2.0 * RANGE_DB);
        top + height * share as f32
    }

    fn db_at(&self, y: f64) -> f64 {
        let (_, top, _, height) = self.plot();
        let share = (y - f64::from(top)) / f64::from(height);
        RANGE_DB - share.clamp(0.0, 1.0) * 2.0 * RANGE_DB
    }

    /// The band whose track is nearest to `x`.
    fn band_at(&self, x: f64) -> usize {
        self.centres()
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = (f64::from(self.x_of(**a)) - x).abs();
                let db = (f64::from(self.x_of(**b)) - x).abs();
                da.total_cmp(&db)
            })
            .map_or(0, |(i, _)| i)
    }

    fn set_band_from_y(&self, band: usize, y: f64) {
        self.change_band(band, self.db_at(y));
    }

    /// Selects `band`, giving its handle the focus when `focus` is set.
    pub(crate) fn select(&self, band: usize, focus: bool) {
        let imp = self.imp();
        let handles = imp.handles.borrow().clone();
        let Some(handle) = handles.get(band) else {
            return;
        };
        imp.selected.set(band);
        for (i, h) in handles.iter().enumerate() {
            h.imp().selected.set(i == band);
            h.queue_draw();
        }
        if focus && !handle.has_focus() {
            handle.grab_focus();
        }
        self.queue_draw();
    }

    /// Sets `band` to `gain` (rounded to the step) as the user would, and
    /// tells the listeners.
    pub(crate) fn change_band(&self, band: usize, gain: f64) {
        let gain = ((gain / STEP_DB).round() * STEP_DB).clamp(-RANGE_DB, RANGE_DB);
        if (gain - self.gain(band)).abs() < f64::EPSILON {
            return;
        }
        let mut settings = self.imp().settings.borrow().clone();
        match settings.mode {
            EqMode::Graphic10 if band < 10 => settings.graphic_10[band] = gain,
            EqMode::Graphic31 if band < 31 => settings.graphic_31[band] = gain,
            _ => return,
        }
        self.set_settings(&settings);
        let listeners = self.imp().listeners.borrow();
        for listener in listeners.iter() {
            listener(band, gain);
        }
    }

    fn connect_handle(&self, handle: &BandHandle) {
        let focus = gtk::EventControllerFocus::new();
        let weak = self.downgrade();
        let weak_handle = handle.downgrade();
        focus.connect_enter(move |_| {
            if let (Some(graph), Some(handle)) = (weak.upgrade(), weak_handle.upgrade()) {
                graph.select(handle.imp().index.get(), false);
            }
        });
        handle.add_controller(focus);
        let keys = gtk::EventControllerKey::new();
        let weak = self.downgrade();
        let weak_handle = handle.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            let (Some(graph), Some(handle)) = (weak.upgrade(), weak_handle.upgrade()) else {
                return glib::Propagation::Proceed;
            };
            let band = handle.imp().index.get();
            let gain = graph.gain(band);
            match key {
                gdk::Key::Up | gdk::Key::KP_Up => graph.change_band(band, gain + STEP_DB),
                gdk::Key::Down | gdk::Key::KP_Down => graph.change_band(band, gain - STEP_DB),
                gdk::Key::Page_Up => graph.change_band(band, gain + 3.0),
                gdk::Key::Page_Down => graph.change_band(band, gain - 3.0),
                gdk::Key::_0 | gdk::Key::KP_0 => graph.change_band(band, 0.0),
                gdk::Key::Left | gdk::Key::KP_Left => {
                    let rtl = graph.direction() == gtk::TextDirection::Rtl;
                    let next = if rtl { band + 1 } else { band.wrapping_sub(1) };
                    graph.select(next, true);
                }
                gdk::Key::Right | gdk::Key::KP_Right => {
                    let rtl = graph.direction() == gtk::TextDirection::Rtl;
                    let next = if rtl { band.wrapping_sub(1) } else { band + 1 };
                    graph.select(next, true);
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        handle.add_controller(keys);
    }

    fn label_at(
        &self,
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

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let look = crate::style::current();
        let accent = look.accent.raw;
        let graphic = self.is_graphic();
        let widget = self.upcast_ref::<gtk::Widget>();
        let (w, h) = (self.width() as f32, self.height() as f32);
        let bounds = graphene::Rect::new(0.0, 0.0, w, h);
        snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, 8.0));
        snapshot.append_color(&glass::BG.to_gdk(), &bounds);
        let (left, top, width, height) = self.plot();
        let line = |snapshot: &gtk::Snapshot, rect: graphene::Rect, color: Rgba| {
            snapshot.append_color(&color.to_gdk(), &rect);
        };

        // The grid: frequencies, or the band tracks in graphic mode.
        if graphic {
            let selected = self.imp().selected.get();
            for (i, hz) in self.centres().iter().enumerate() {
                let x = self.x_of(*hz).round();
                let color = if i == selected {
                    glass::GRAPH_ZERO
                } else {
                    glass::GRAPH_GRID
                };
                line(
                    snapshot,
                    graphene::Rect::new(x - 0.5, top, 1.0, height),
                    color,
                );
            }
        } else {
            for hz in GRID_HZ {
                let x = self.x_of(hz).round();
                line(
                    snapshot,
                    graphene::Rect::new(x - 0.5, top, 1.0, height),
                    glass::GRAPH_GRID,
                );
            }
        }
        for db in [12.0, 6.0, -6.0, -12.0] {
            let y = self.y_of(db).round();
            line(
                snapshot,
                graphene::Rect::new(left, y - 0.5, width, 1.0),
                glass::GRAPH_GRID,
            );
        }
        let zero = self.y_of(0.0).round();
        line(
            snapshot,
            graphene::Rect::new(left, zero - 0.5, width, 1.0),
            glass::GRAPH_ZERO,
        );

        // The dB labels at the left.
        for db in [12.0, 6.0, 0.0, -6.0, -12.0] {
            let text = match db {
                d if d > 0.0 => format!("+{d:.0}"),
                d if d < 0.0 => format!("−{:.0}", -d),
                _ => "0".into(),
            };
            let layout = mono_layout(widget, &text, 9.5, pango::Weight::Normal);
            let (_, logical) = layout.pixel_extents();
            self.label_at(
                snapshot,
                &layout,
                10.0,
                self.y_of(db) - logical.height() as f32 / 2.0,
                glass::T3,
            );
        }

        // The curve, with its fill to the zero line.
        let sections = self.imp().sections.borrow();
        let steps = (width / 2.0).ceil().max(2.0) as usize;
        let points: Vec<(f32, f32)> = (0..=steps)
            .map(|i| {
                let x = left + width * i as f32 / steps as f32;
                let share = f64::from((x - left) / width);
                let hz = LOW_HZ * (HIGH_HZ / LOW_HZ).powf(share);
                (x, self.y_of(eq::response_db(&sections, hz, RATE)))
            })
            .collect();
        drop(sections);
        let curve = gsk::PathBuilder::new();
        let area = gsk::PathBuilder::new();
        area.move_to(left, zero);
        for (i, (x, y)) in points.iter().enumerate() {
            if i == 0 {
                curve.move_to(*x, *y);
            } else {
                curve.line_to(*x, *y);
            }
            area.line_to(*x, *y);
        }
        area.line_to(left + width, zero);
        area.close();
        snapshot.append_fill(
            &area.to_path(),
            gsk::FillRule::Winding,
            &accent.with_alpha(0.14).to_gdk(),
        );
        let stroke = gsk::Stroke::new(2.0);
        stroke.set_line_join(gsk::LineJoin::Round);
        snapshot.append_stroke(&curve.to_path(), &stroke, &accent.to_gdk());

        if graphic {
            self.draw_graphic_labels(snapshot, widget, top + height);
        } else {
            self.draw_parametric(snapshot, widget, top + height);
        }
        snapshot.pop();
    }

    fn draw_parametric(&self, snapshot: &gtk::Snapshot, widget: &gtk::Widget, bottom: f32) {
        let look = crate::style::current();
        let accent = look.accent.raw;
        for hz in GRID_HZ {
            let layout = mono_layout(widget, &short_hz(hz), 9.5, pango::Weight::Normal);
            let (_, logical) = layout.pixel_extents();
            let x = (self.x_of(hz) - logical.width() as f32 / 2.0)
                .clamp(0.0, self.width() as f32 - logical.width() as f32 - 2.0);
            self.label_at(snapshot, &layout, x, bottom + 7.0, glass::T3);
        }
        let sections = self.imp().sections.borrow();
        for (i, mark) in self.imp().marks.borrow().iter().enumerate() {
            let x = self.x_of(mark.hz);
            let y = self.y_of(eq::response_db(&sections, mark.hz, RATE));
            let side = 18.0;
            let rect = graphene::Rect::new(x - side / 2.0, y - side / 2.0, side, side);
            let round = gsk::RoundedRect::from_rect(rect, side / 2.0);
            snapshot.push_rounded_clip(&round);
            snapshot.append_color(&glass::BG.to_gdk(), &rect);
            snapshot.pop();
            let (ring, text) = if mark.enabled {
                (accent, glass::T1)
            } else {
                (glass::T3, glass::T3)
            };
            snapshot.append_border(&round, &[1.5; 4], &[ring.to_gdk(); 4]);
            let layout = mono_layout(widget, &(i + 1).to_string(), 9.5, pango::Weight::Semibold);
            let (_, logical) = layout.pixel_extents();
            self.label_at(
                snapshot,
                &layout,
                x - logical.width() as f32 / 2.0,
                y - logical.height() as f32 / 2.0,
                text,
            );
        }
    }

    fn draw_graphic_labels(&self, snapshot: &gtk::Snapshot, widget: &gtk::Widget, bottom: f32) {
        let selected = self.imp().selected.get();
        let centres = self.centres();
        let ten = centres.len() <= 10;
        for (i, hz) in centres.iter().enumerate() {
            if !ten && i % 3 != 0 {
                continue;
            }
            let x = self.x_of(*hz);
            let layout = mono_layout(widget, &short_hz(*hz), 9.5, pango::Weight::Normal);
            let (_, logical) = layout.pixel_extents();
            let lx = (x - logical.width() as f32 / 2.0)
                .clamp(0.0, self.width() as f32 - logical.width() as f32 - 2.0);
            self.label_at(snapshot, &layout, lx, bottom + 10.0, glass::T3);
            if ten {
                let gain = self.gain(i);
                let (color, weight) = if i == selected {
                    (glass::T1, pango::Weight::Semibold)
                } else {
                    (glass::T2, pango::Weight::Normal)
                };
                let layout = mono_layout(widget, &signed(gain), 10.0, weight);
                let (_, logical) = layout.pixel_extents();
                self.label_at(
                    snapshot,
                    &layout,
                    x - logical.width() as f32 / 2.0,
                    bottom + 28.0,
                    color,
                );
            }
        }
        // The selected band at the top right.
        if let Some(hz) = centres.get(selected) {
            let value = fill(
                &tr("{} · {} dB"),
                &[&hz_text(*hz), &signed(self.gain(selected))],
            );
            let value = mono_layout(widget, &value, 11.0, pango::Weight::Semibold);
            let (_, vl) = value.pixel_extents();
            let right = self.width() as f32 - 24.0;
            let vx = right - vl.width() as f32;
            self.label_at(snapshot, &value, vx, 12.0, glass::T1);
            let micro = widget.create_pango_layout(Some(&tr("Band").to_uppercase()));
            let mut font = pango::FontDescription::new();
            font.set_weight(pango::Weight::Semibold);
            font.set_absolute_size(9.5 * f64::from(pango::SCALE));
            micro.set_font_description(Some(&font));
            let (_, ml) = micro.pixel_extents();
            self.label_at(
                snapshot,
                &micro,
                vx - ml.width() as f32 - 8.0,
                12.0 + (vl.height() - ml.height()) as f32 / 2.0,
                glass::T3,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequencies_read_as_on_the_graph() {
        assert_eq!(short_hz(31.5), "31.5");
        assert_eq!(short_hz(500.0), "500");
        assert_eq!(short_hz(1250.0), "1.25k");
        assert_eq!(short_hz(16_000.0), "16k");
        assert_eq!(signed(-2.0), "−2.0");
        assert_eq!(signed(4.5), "+4.5");
        assert_eq!(signed(0.0), "0.0");
    }
}
