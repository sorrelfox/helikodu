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
        pub(super) item: RefCell<Option<AlbumItem>>,
        pub(super) loaded: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) key: Cell<Option<u64>>,
        pub(super) covers: RefCell<Option<Rc<CoverCache>>>,
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
    }

    fn show(&self, row: &AlbumRow) {
        let imp = self.imp();
        imp.cover.set_loading(false);
        match imp.covers.borrow().as_ref() {
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
