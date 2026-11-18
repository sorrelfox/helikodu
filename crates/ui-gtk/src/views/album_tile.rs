// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! One tile of the album grid: the cover, then title, artist and a format line.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::rc::Rc;

use gtk::glib;

use crate::backend::{AlbumKey, AlbumRow, CoverKey};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::covers::thumbnail_size;
use crate::models::{AlbumItem, CoverCache};

/// The logical size of a tile's cover the thumbnail is chosen for.
const TILE: i32 = 160;
use crate::widgets::{CoverArt, label};

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct AlbumTile {
        pub(super) cover: CoverArt,
        pub(super) title: RefCell<Option<gtk::Label>>,
        pub(super) artist: RefCell<Option<gtk::Label>>,
        pub(super) meta: RefCell<Option<gtk::Label>>,
        pub(super) text: RefCell<Option<gtk::Box>>,
        pub(super) bars: RefCell<Option<gtk::DrawingArea>>,
        pub(super) item: RefCell<Option<AlbumItem>>,
        pub(super) loaded: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) key: Cell<Option<u64>>,
        pub(super) covers: RefCell<Option<Rc<CoverCache>>>,
        /// Whether the cover loads only while the tile is in view (the grid).
        pub(super) lazy: Cell<bool>,
        pub(super) in_view: Cell<bool>,
        /// The cover and placeholder seed of the album shown.
        pub(super) art: Cell<Option<(Option<CoverKey>, u64)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumTile {
        const NAME: &'static str = "HkAlbumTile";
        type Type = super::AlbumTile;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for AlbumTile {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(gtk::Orientation::Vertical);
            obj.append(&self.cover);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
            text.set_margin_top(9);
            let title = label("", &["hk-medium"]);
            let artist = label("", &["hk-t2", "hk-secondary"]);
            let meta = label("", &["hk-t3", "hk-mono-meta"]);
            meta.set_margin_top(3);
            for l in [&title, &artist, &meta] {
                l.set_max_width_chars(1);
                l.set_hexpand(true);
                text.append(l);
            }
            obj.append(&text);
            let bars = placeholder_bars();
            bars.set_visible(false);
            obj.append(&bars);
            self.text.replace(Some(text));
            self.bars.replace(Some(bars));
            self.title.replace(Some(title));
            self.artist.replace(Some(artist));
            self.meta.replace(Some(meta));
        }
    }

    impl WidgetImpl for AlbumTile {}
    impl BoxImpl for AlbumTile {}
}

glib::wrapper! {
    /// An album tile.
    pub(crate) struct AlbumTile(ObjectSubclass<imp::AlbumTile>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for AlbumTile {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl AlbumTile {
    /// The album shown, if any.
    pub(crate) fn key(&self) -> Option<AlbumKey> {
        self.imp().key.get().map(AlbumKey)
    }

    /// The title shown.
    pub(crate) fn title(&self) -> String {
        self.imp()
            .title
            .borrow()
            .as_ref()
            .map(|l| l.text().to_string())
            .unwrap_or_default()
    }

    /// The cover widget.
    pub(crate) fn cover(&self) -> &CoverArt {
        &self.imp().cover
    }

    /// Shows `item`, now or when its row arrives, with its cover from `covers`.
    pub(crate) fn bind(&self, item: &AlbumItem, covers: &Rc<CoverCache>) {
        let imp = self.imp();
        imp.covers.replace(Some(covers.clone()));
        imp.key.set(Some(item.key().0));
        match item.row() {
            Some(row) => self.show(&row),
            None => {
                self.show_loading();
                let weak = self.downgrade();
                let id = item.connect_loaded(move |item| {
                    if let (Some(tile), Some(row)) = (weak.upgrade(), item.row())
                        && tile.key() == Some(item.key())
                    {
                        tile.show(&row);
                    }
                });
                imp.loaded.replace(Some(id));
            }
        }
        imp.item.replace(Some(item.clone()));
    }

    /// Forgets the item when the tile is recycled.
    pub(crate) fn unbind(&self) {
        let imp = self.imp();
        if let (Some(item), Some(id)) = (imp.item.take(), imp.loaded.take()) {
            item.disconnect(id);
        }
        if let (Some(cover), Some(covers)) = (imp.cover.wanted(), imp.covers.borrow().as_ref()) {
            covers.cancel(cover, self.thumbnail_size());
        }
        imp.cover.want(None);
        imp.key.set(None);
        imp.in_view.set(false);
        imp.art.set(None);
    }

    /// Loads the cover only while the tile is in view (see [`Self::set_in_view`]);
    /// otherwise it shows a cached cover or the placeholder.
    pub(crate) fn set_lazy_cover(&self) {
        self.imp().lazy.set(true);
    }

    /// Whether the tile is in view: a lazy tile loads its cover when it comes
    /// into view and lets go of it when it leaves, so that the tiles the grid
    /// keeps off screen hold no pictures.
    pub(crate) fn set_in_view(&self, on: bool) {
        let imp = self.imp();
        if !imp.lazy.get() || imp.in_view.replace(on) == on {
            return;
        }
        let (Some((cover, seed)), Some(covers)) = (imp.art.get(), imp.covers.borrow().clone())
        else {
            return;
        };
        if on {
            covers.show(&imp.cover, cover, seed, TILE);
        } else {
            if let Some(cover) = cover {
                covers.cancel(cover, self.thumbnail_size());
            }
            imp.cover.show(seed, None);
        }
    }

    /// The thumbnail size of the cover in device pixels.
    pub(crate) fn thumbnail_size(&self) -> u32 {
        thumbnail_size(TILE, self.scale_factor())
    }

    /// The cover this tile waits for, if it still shows the placeholder.
    pub(crate) fn waiting_cover(&self) -> Option<(CoverKey, u32)> {
        let imp = self.imp();
        let cover = imp.cover.wanted()?;
        let size = self.thumbnail_size();
        let covers = imp.covers.borrow();
        covers
            .as_ref()
            .filter(|c| c.is_waiting(cover, size))
            .map(|_| (cover, size))
    }

    fn show_loading(&self) {
        let imp = self.imp();
        imp.cover.want(None);
        imp.cover.set_loading(true);
        for l in [&imp.title, &imp.artist, &imp.meta] {
            if let Some(l) = l.borrow().as_ref() {
                l.set_text("");
            }
        }
        self.show_bars(true);
    }

    /// Shows the three placeholder bars in place of the text, or the text.
    fn show_bars(&self, on: bool) {
        let imp = self.imp();
        if let Some(text) = imp.text.borrow().as_ref() {
            text.set_visible(!on);
        }
        if let Some(bars) = imp.bars.borrow().as_ref() {
            if on {
                // Each tile its own widths, the same each time it is shown.
                let seed = imp.key.get().unwrap_or_default();
                bars.set_widget_name(&seed.to_string());
                bars.queue_draw();
            }
            bars.set_visible(on);
        }
    }

    fn show(&self, row: &AlbumRow) {
        let imp = self.imp();
        imp.cover.set_loading(false);
        self.show_bars(false);
        imp.art.set(Some((row.cover, row.cover_seed)));
        match imp.covers.borrow().as_ref() {
            Some(covers) if imp.lazy.get() && !imp.in_view.get() => {
                covers.show_cached(&imp.cover, row.cover, row.cover_seed, TILE);
            }
            Some(covers) => covers.show(&imp.cover, row.cover, row.cover_seed, TILE),
            None => imp.cover.show(row.cover_seed, None),
        }
        // The format if the list knows it; the track count stands in for it.
        let detail = if row.format.codec.is_some() {
            format::short(&row.format)
        } else {
            fill(
                &ntr("{} track", "{} tracks", row.track_count),
                &[&row.track_count.to_string()],
            )
        };
        let meta = match row.year {
            Some(year) => fill(&tr("{} · {}"), &[&year.to_string(), &detail]),
            None => detail,
        };
        if let Some(l) = imp.title.borrow().as_ref() {
            l.set_text(&row.title);
        }
        if let Some(l) = imp.artist.borrow().as_ref() {
            l.set_text(&row.artist);
        }
        if let Some(l) = imp.meta.borrow().as_ref() {
            l.set_text(&meta);
        }
        imp.cover
            .update_property(&[gtk::accessible::Property::Label(&fill(
                &tr("{} by {}"),
                &[&row.title, &row.artist],
            ))]);
    }
}

/// The three bars in `line` under a placeholder tile: 9, 8 and 7 high, 7
/// apart, radius 3, each between 34 % and 88 % of the tile's width. The
/// colour is the widget's CSS colour; the widths follow the widget name, which
/// holds the album's key.
fn placeholder_bars() -> gtk::DrawingArea {
    let bars = gtk::DrawingArea::new();
    bars.add_css_class("hk-placeholder-bars");
    bars.set_margin_top(13);
    bars.set_content_height(9 + 7 + 8 + 7 + 7);
    bars.set_can_target(false);
    bars.set_draw_func(|area, cr, width, _| {
        let seed: u64 = area.widget_name().parse().unwrap_or_default();
        let color = area.color();
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        let mut y = 0.0;
        for (i, height) in [9.0, 8.0, 7.0].into_iter().enumerate() {
            let w = f64::from(width) * bar_share(seed, i);
            rounded(cr, 0.0, y, w.round(), height, 3.0);
            y += height + 7.0;
        }
        let _ = cr.fill();
    });
    bars
}

/// A share of the width between 0.34 and 0.88 for bar `i` of the tile `seed`.
fn bar_share(seed: u64, i: usize) -> f64 {
    let mut x = seed
        .wrapping_add(i as u64 + 1)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    0.34 + (x % 1000) as f64 / 999.0 * (0.88 - 0.34)
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}
