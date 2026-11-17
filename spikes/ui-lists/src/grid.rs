// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The album grid: a GtkGridView of fixed-size tiles over the lazy ID model,
//! with covers decoded and scaled on worker threads.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::covers::{Loader, Lru, Thumb};
use crate::model::{IdModel, RowItem};

pub const TILE: i32 = 160;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AlbumTile {
        pub picture: OnceCell<gtk::Picture>,
        pub title: OnceCell<gtk::Label>,
        pub artist: OnceCell<gtk::Label>,
        pub album: Cell<Option<u32>>,
        pub requested: Cell<Option<Instant>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumTile {
        const NAME: &'static str = "HkSpikeAlbumTile";
        type Type = super::AlbumTile;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for AlbumTile {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(gtk::Orientation::Vertical);
            obj.set_spacing(4);
            obj.set_margin_start(10);
            obj.set_margin_end(10);
            obj.set_margin_top(10);
            obj.set_margin_bottom(10);
            obj.set_halign(gtk::Align::Center);
            let pic = gtk::Picture::new();
            pic.set_size_request(TILE, TILE);
            pic.set_halign(gtk::Align::Center);
            pic.set_hexpand(false);
            pic.set_content_fit(gtk::ContentFit::Cover);
            pic.set_can_shrink(true);
            pic.add_css_class("hk-cover");
            pic.set_overflow(gtk::Overflow::Hidden);
            let title = gtk::Label::new(None);
            let artist = gtk::Label::new(None);
            for l in [&title, &artist] {
                l.set_xalign(0.0);
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                l.set_max_width_chars(1);
                l.set_width_request(TILE);
            }
            artist.add_css_class("hk-t3");
            artist.add_css_class("hk-secondary");
            obj.append(&pic);
            obj.append(&title);
            obj.append(&artist);
            let _ = self.picture.set(pic);
            let _ = self.title.set(title);
            let _ = self.artist.set(artist);
        }
    }

    impl WidgetImpl for AlbumTile {}
    impl BoxImpl for AlbumTile {}
}

glib::wrapper! {
    pub struct AlbumTile(ObjectSubclass<imp::AlbumTile>) @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl AlbumTile {
    fn new() -> Self {
        glib::Object::new()
    }

    fn picture(&self) -> &gtk::Picture {
        self.imp().picture.get().expect("constructed")
    }
}

thread_local! {
    pub static TILES_CREATED: Cell<u64> = const { Cell::new(0) };
    pub static TILES_BOUND: Cell<u64> = const { Cell::new(0) };
    pub static TILES_BOUND_MAX: Cell<u64> = const { Cell::new(0) };
}

/// Main-thread side of the thumbnail pipeline.
#[derive(Debug)]
pub struct Thumbs {
    pub loader: Loader,
    pub cache: RefCell<Lru<gdk::Texture>>,
    pub waiting: RefCell<HashMap<u32, glib::WeakRef<AlbumTile>>>,
    /// Bind-to-visible latency of covers that had to be decoded, in ms.
    pub latencies: RefCell<Vec<f64>>,
    pub decode_ms: RefCell<Vec<f64>>,
    pub cache_hits: Cell<u64>,
    pub stale_results: Cell<u64>,
    pub px: Cell<u32>,
    pub covers: u32,
}

impl Thumbs {
    /// Tiles that are bound but still show the placeholder.
    pub fn unfilled(&self) -> usize {
        self.waiting
            .borrow()
            .values()
            .filter(|w| w.upgrade().is_some())
            .count()
    }

    /// Once per frame: finds bound tiles without a cover that intersect the
    /// viewport, and moves their requests to the front of the line. Returns
    /// how many visible tiles still show the placeholder.
    pub fn prioritise_visible(&self, viewport: &gtk::Widget) -> usize {
        let vw = f64::from(viewport.width());
        let vh = f64::from(viewport.height());
        let px = self.px.get();
        let mut visible = Vec::new();
        for (album, w) in self.waiting.borrow().iter() {
            let Some(tile) = w.upgrade() else { continue };
            if !tile.is_mapped() {
                continue;
            }
            let Some(b) = tile.compute_bounds(viewport) else {
                continue;
            };
            let (x, y, w, h) = (
                f64::from(b.x()),
                f64::from(b.y()),
                f64::from(b.width()),
                f64::from(b.height()),
            );
            if x + w > 0.0 && y + h > 0.0 && x < vw && y < vh {
                visible.push((*album, *album % self.covers, px));
            }
        }
        self.loader.promote(&visible);
        visible.len()
    }

    fn bind(&self, tile: &AlbumTile, album: u32) {
        TILES_BOUND.with(|c| {
            c.set(c.get() + 1);
            TILES_BOUND_MAX.with(|m| m.set(m.get().max(c.get())));
        });
        let imp = tile.imp();
        imp.album.set(Some(album));
        let pic = tile.picture();
        if let Some(tex) = self.cache.borrow_mut().get(album) {
            self.cache_hits.set(self.cache_hits.get() + 1);
            pic.set_paintable(Some(&tex));
            pic.remove_css_class("hk-placeholder");
            return;
        }
        pic.set_paintable(None::<&gdk::Paintable>);
        pic.add_css_class("hk-placeholder");
        imp.requested.set(Some(Instant::now()));
        let w = glib::WeakRef::new();
        w.set(Some(tile));
        self.waiting.borrow_mut().insert(album, w);
        self.loader
            .request(album, album % self.covers, self.px.get());
    }

    fn unbind(&self, tile: &AlbumTile) {
        TILES_BOUND.with(|c| c.set(c.get().saturating_sub(1)));
        if let Some(album) = tile.imp().album.take()
            && self.waiting.borrow_mut().remove(&album).is_some()
        {
            self.loader.cancel(album);
        }
    }

    fn deliver(&self, t: Thumb) {
        self.decode_ms.borrow_mut().push(t.decode_ms);
        let bytes = glib::Bytes::from_owned(t.rgba);
        let tex = gdk::MemoryTexture::new(
            t.px as i32,
            t.px as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            t.px as usize * 4,
        )
        .upcast::<gdk::Texture>();
        self.cache.borrow_mut().put(t.album, tex.clone());
        let tile = self
            .waiting
            .borrow_mut()
            .remove(&t.album)
            .and_then(|w| w.upgrade());
        match tile {
            Some(tile) if tile.imp().album.get() == Some(t.album) => {
                let pic = tile.picture();
                pic.set_paintable(Some(&tex));
                pic.remove_css_class("hk-placeholder");
                if let Some(at) = tile.imp().requested.take() {
                    self.latencies
                        .borrow_mut()
                        .push(at.elapsed().as_secs_f64() * 1000.0);
                }
            }
            _ => self.stale_results.set(self.stale_results.get() + 1),
        }
    }
}

pub fn build(
    albums: u32,
    covers_dir: PathBuf,
    covers: u32,
    workers: usize,
    cache: usize,
    max_columns: u32,
) -> (gtk::ScrolledWindow, Rc<Thumbs>) {
    let (tx, rx) = async_channel::unbounded::<Thumb>();
    let thumbs = Rc::new(Thumbs {
        loader: Loader::start(covers_dir, workers, 256, tx),
        cache: RefCell::new(Lru::new(cache)),
        waiting: RefCell::new(HashMap::new()),
        latencies: RefCell::new(Vec::new()),
        decode_ms: RefCell::new(Vec::new()),
        cache_hits: Cell::new(0),
        stale_results: Cell::new(0),
        px: Cell::new(TILE as u32),
        covers,
    });
    let weak = Rc::downgrade(&thumbs);
    glib::spawn_future_local(async move {
        while let Ok(t) = rx.recv().await {
            let Some(th) = weak.upgrade() else { break };
            th.deliver(t);
        }
    });

    let model = IdModel::new((0..albums).collect());
    let selection = gtk::SingleSelection::new(Some(model));
    let f = gtk::SignalListItemFactory::new();
    f.connect_setup(|_, li| {
        let Some(li) = li.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        TILES_CREATED.with(|c| c.set(c.get() + 1));
        li.set_child(Some(&AlbumTile::new()));
    });
    let th = Rc::downgrade(&thumbs);
    f.connect_bind(move |_, li| {
        let Some(li) = li.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(tile), Some(item)) = (
            li.child().and_downcast::<AlbumTile>(),
            li.item().and_downcast::<RowItem>(),
        ) else {
            return;
        };
        let a = item.album();
        let imp = tile.imp();
        if let Some(l) = imp.title.get() {
            l.set_text(&a.title);
        }
        if let Some(l) = imp.artist.get() {
            l.set_text(&format!("{} · {}", a.artist, a.year));
        }
        if let Some(th) = th.upgrade() {
            // Covers at device pixels: the surface scale may be fractional.
            let scale = tile
                .native()
                .and_then(|n| n.surface())
                .map_or(1.0, |s| s.scale());
            th.px.set((f64::from(TILE) * scale.max(1.0)).ceil() as u32);
            th.bind(&tile, item.id());
        }
    });
    let th = Rc::downgrade(&thumbs);
    f.connect_unbind(move |_, li| {
        let Some(li) = li.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let (Some(tile), Some(th)) = (li.child().and_downcast::<AlbumTile>(), th.upgrade()) {
            th.unbind(&tile);
        }
    });
    let grid = gtk::GridView::new(Some(selection), Some(f));
    grid.add_css_class("hk-grid");
    // GtkGridView keeps about max-columns x 30 tiles bound, so this is a
    // memory and bind-cost setting, not only a layout one.
    grid.set_max_columns(max_columns);
    grid.set_min_columns(2);
    let sw = gtk::ScrolledWindow::new();
    sw.set_vexpand(true);
    sw.set_child(Some(&grid));
    (sw, thumbs)
}
