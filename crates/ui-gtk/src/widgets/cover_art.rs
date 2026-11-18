// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! CoverArt: a square cover with rounded corners and a hairline ring. Without
//! art it draws the generated placeholder: one of the palettes and patterns of
//! the design tokens, picked by a stable hash of the album.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use crate::backend::CoverKey;
use crate::style::color::Rgba;
use crate::style::tokens::{DARK, cover_placeholder};

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct CoverArt {
        pub(super) seed: Cell<u64>,
        pub(super) texture: RefCell<Option<gdk::Texture>>,
        pub(super) radius: Cell<f32>,
        /// A fixed side, or 0 to be as tall as the width allows.
        pub(super) side: Cell<i32>,
        /// Draw nothing: the placeholder tile's CSS shows through.
        pub(super) loading: Cell<bool>,
        /// The cover this widget waits for or shows.
        pub(super) wanted: Cell<Option<CoverKey>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverArt {
        const NAME: &'static str = "HkCoverArt";
        type Type = super::CoverArt;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("coverart");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for CoverArt {
        fn constructed(&self) {
            self.parent_constructed();
            self.radius.set(6.0);
            self.obj().add_css_class("hk-cover");
        }
    }

    impl WidgetImpl for CoverArt {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            if self.side.get() > 0 {
                gtk::SizeRequestMode::ConstantSize
            } else {
                gtk::SizeRequestMode::HeightForWidth
            }
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let side = self.side.get();
            if side > 0 {
                return (side, side, -1, -1);
            }
            // In the grid: at least the design's tile width, then as tall as wide.
            match orientation {
                gtk::Orientation::Horizontal => (148, 148, -1, -1),
                _ if for_size > 0 => (for_size, for_size, -1, -1),
                _ => (148, 148, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            let s = w.min(h);
            if s <= 0.0 {
                return;
            }
            if self.loading.get() {
                return;
            }
            let bounds = graphene::Rect::new((w - s) / 2.0, 0.0, s, s);
            let r = self.radius.get();
            let rounded = gsk::RoundedRect::from_rect(bounds, r);
            snapshot.push_rounded_clip(&rounded);
            match self.texture.borrow().as_ref() {
                Some(texture) => {
                    snapshot.append_scaled_texture(texture, gsk::ScalingFilter::Trilinear, &bounds)
                }
                None => {
                    snapshot.save();
                    snapshot.translate(&graphene::Point::new(bounds.x(), 0.0));
                    placeholder(snapshot, s, self.seed.get());
                    snapshot.restore();
                }
            }
            snapshot.pop();
            let ring = DARK.cover_ring.to_gdk();
            snapshot.append_border(&rounded, &[1.0; 4], &[ring, ring, ring, ring]);
        }
    }
}

glib::wrapper! {
    /// A cover image or its generated placeholder.
    pub(crate) struct CoverArt(ObjectSubclass<imp::CoverArt>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for CoverArt {
    fn default() -> Self {
        Self::new(0, 6.0)
    }
}

impl CoverArt {
    /// A cover of `side` pixels (0: square at whatever width it gets) with
    /// corner radius `radius`.
    pub(crate) fn new(side: i32, radius: f32) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().side.set(side);
        obj.imp().radius.set(radius);
        obj
    }

    /// Gives the cover a fixed side (0: square at its width) and corner radius.
    pub(crate) fn set_shape(&self, side: i32, radius: f32) {
        self.imp().side.set(side);
        self.imp().radius.set(radius);
        self.queue_resize();
    }

    /// Shows the placeholder for `seed`, or `texture` when there is art.
    pub(crate) fn show(&self, seed: u64, texture: Option<gdk::Texture>) {
        let imp = self.imp();
        if imp.seed.get() == seed && *imp.texture.borrow() == texture {
            return;
        }
        imp.seed.set(seed);
        imp.texture.replace(texture);
        self.queue_draw();
    }

    /// Notes which cover the widget should show, so that late thumbnails of
    /// another cover are ignored.
    pub(crate) fn want(&self, cover: Option<CoverKey>) {
        self.imp().wanted.set(cover);
    }

    /// Whether a cover image is shown rather than the placeholder.
    pub(crate) fn has_image(&self) -> bool {
        self.imp().texture.borrow().is_some()
    }

    /// The cover the widget should show.
    pub(crate) fn wanted(&self) -> Option<CoverKey> {
        self.imp().wanted.get()
    }

    /// Shows the loading tile instead of a cover.
    pub(crate) fn set_loading(&self, loading: bool) {
        if self.imp().loading.replace(loading) != loading {
            if loading {
                self.add_css_class("hk-placeholder");
            } else {
                self.remove_css_class("hk-placeholder");
            }
            self.queue_draw();
        }
    }

    /// Marks the cover of the album that plays (the accent ring).
    pub(crate) fn set_playing(&self, playing: bool) {
        if playing {
            self.add_css_class("playing");
        } else {
            self.remove_css_class("playing");
        }
    }
}

fn color(c: Rgba) -> gdk::RGBA {
    c.to_gdk()
}

fn circle(snapshot: &gtk::Snapshot, x: f32, y: f32, r: f32, c: Rgba) {
    let builder = gsk::PathBuilder::new();
    builder.add_circle(&graphene::Point::new(x, y), r);
    snapshot.append_fill(&builder.to_path(), gsk::FillRule::Winding, &color(c));
}

fn rect(snapshot: &gtk::Snapshot, x: f32, y: f32, w: f32, h: f32, c: Rgba) {
    snapshot.append_color(&color(c), &graphene::Rect::new(x, y, w, h));
}

/// Which palette and pattern `seed` picks.
pub(crate) fn pick(seed: u64) -> (usize, usize) {
    let palettes = cover_placeholder::PALETTES.len() as u64;
    let patterns = cover_placeholder::PATTERNS.len() as u64;
    (
        (seed % palettes) as usize,
        (seed / palettes % patterns) as usize,
    )
}

/// Draws the generated cover for `seed` into a square of side `s` at the origin.
fn placeholder(snapshot: &gtk::Snapshot, s: f32, seed: u64) {
    let (palette, pattern) = pick(seed);
    let [ground, shape, detail] = cover_placeholder::PALETTES[palette];
    rect(snapshot, 0.0, 0.0, s, s, ground);
    match pattern {
        // Disc.
        0 => circle(snapshot, 0.68 * s, 0.34 * s, 0.21 * s, shape),
        // Diagonal: the shape below-right of the 135 degree diagonal.
        1 => {
            let builder = gsk::PathBuilder::new();
            builder.move_to(s, 0.0);
            builder.line_to(s, s);
            builder.line_to(0.0, s);
            builder.close();
            snapshot.append_fill(&builder.to_path(), gsk::FillRule::Winding, &color(shape));
        }
        // Arch: a shape ring around a detail half-disc, both at the bottom centre.
        2 => {
            circle(snapshot, 0.5 * s, s, 0.56 * s, shape);
            circle(snapshot, 0.5 * s, s, 0.38 * s, detail);
        }
        // Stripes.
        3 => {
            for i in 0..5 {
                let x = (0.07 + 0.2 * i as f32) * s;
                rect(snapshot, x, 0.0, 0.06 * s, s, shape);
            }
        }
        // Horizon.
        4 => {
            rect(snapshot, 0.0, 0.62 * s, s, 0.04 * s, detail);
            rect(snapshot, 0.0, 0.66 * s, s, 0.34 * s, shape);
        }
        // Two discs.
        _ => {
            circle(snapshot, 0.30 * s, 0.70 * s, 0.12 * s, detail);
            circle(snapshot, 0.62 * s, 0.40 * s, 0.30 * s, shape);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pick;

    #[test]
    fn every_palette_and_pattern_is_reachable() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..60 {
            seen.insert(pick(seed));
        }
        assert_eq!(seen.len(), 60);
    }
}
