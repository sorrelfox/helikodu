// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The album list model of the grid: a `GListModel` over album keys.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::backend::{AlbumKey, AlbumQuery, AlbumRow, Backend};

/// How many neighbouring rows one fetch asks for at most.
const BATCH: usize = 120;
/// How many rows the model keeps; beyond that the cache starts again.
const CACHE: usize = 4096;

mod imp_item {
    use super::*;
    use std::sync::OnceLock;

    #[derive(Debug, Default)]
    pub(crate) struct AlbumItem {
        pub(super) key: Cell<u64>,
        pub(super) position: Cell<u32>,
        pub(super) row: RefCell<Option<Rc<AlbumRow>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumItem {
        const NAME: &'static str = "HkAlbumItem";
        type Type = super::AlbumItem;
    }

    impl ObjectImpl for AlbumItem {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![glib::subclass::Signal::builder("loaded").build()])
        }
    }
}

glib::wrapper! {
    /// One album of the model: its key and, once fetched, its row.
    pub(crate) struct AlbumItem(ObjectSubclass<imp_item::AlbumItem>);
}

impl AlbumItem {
    /// An item at `position`, with its row if it is known.
    pub(crate) fn new(key: AlbumKey, position: u32, row: Option<Rc<AlbumRow>>) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().key.set(key.0);
        obj.imp().position.set(position);
        obj.imp().row.replace(row);
        obj
    }

    /// The album.
    pub(crate) fn key(&self) -> AlbumKey {
        AlbumKey(self.imp().key.get())
    }

    /// Its position in the model.
    pub(crate) fn position(&self) -> u32 {
        self.imp().position.get()
    }

    /// Its row, if fetched.
    pub(crate) fn row(&self) -> Option<Rc<AlbumRow>> {
        self.imp().row.borrow().clone()
    }

    fn fill(&self, row: Rc<AlbumRow>) {
        self.imp().row.replace(Some(row));
        self.emit_by_name::<()>("loaded", &[]);
    }

    /// Calls `f` when the row arrives.
    pub(crate) fn connect_loaded(&self, f: impl Fn(&Self) + 'static) -> glib::SignalHandlerId {
        self.connect_local("loaded", false, move |args| {
            if let Some(item) = args.first().and_then(|v| v.get::<Self>().ok()) {
                f(&item);
            }
            None
        })
    }
}

/// Fetches rows in batches of neighbouring positions and caches them.
#[derive(Debug, Default)]
struct Fetcher {
    backend: RefCell<Option<Backend>>,
    query: Cell<AlbumQuery>,
    keys: Rc<RefCell<Vec<AlbumKey>>>,
    cache: RefCell<HashMap<AlbumKey, Rc<AlbumRow>>>,
    waiting: RefCell<BTreeMap<u32, Vec<glib::WeakRef<AlbumItem>>>>,
    scheduled: Cell<bool>,
    generation: Cell<u64>,
    fetches: Cell<u64>,
}

impl Fetcher {
    fn request(self: &Rc<Self>, item: &AlbumItem) {
        self.waiting
            .borrow_mut()
            .entry(item.position())
            .or_default()
            .push(item.downgrade());
        if !self.scheduled.replace(true) {
            let me = self.clone();
            glib::idle_add_local_once(move || me.fetch());
        }
    }

    /// Forgets what is waiting and cached, after the list changed.
    fn reset(&self, query: AlbumQuery) {
        self.query.set(query);
        self.waiting.borrow_mut().clear();
        self.cache.borrow_mut().clear();
        self.generation.set(self.generation.get() + 1);
    }

    fn fetch(self: Rc<Self>) {
        self.scheduled.set(false);
        let Some(backend) = self.backend.borrow().clone() else {
            return;
        };
        // The first waiting position and the waiting ones within a batch of it.
        let Some(first) = self.waiting.borrow().keys().next().copied() else {
            return;
        };
        let last = self
            .waiting
            .borrow()
            .range(first..first.saturating_add(BATCH as u32))
            .next_back()
            .map_or(first, |(p, _)| *p);
        let count = (last - first + 1) as usize;
        self.fetches.set(self.fetches.get() + 1);
        let generation = self.generation.get();
        let query = self.query.get();
        let me = self.clone();
        glib::spawn_future_local(async move {
            let rows = backend
                .query(move |library| library.album_rows(&query, first as usize, count))
                .await
                .unwrap_or_default();
            if me.generation.get() != generation {
                return;
            }
            {
                let keys = me.keys.borrow();
                let mut cache = me.cache.borrow_mut();
                if cache.len() + rows.len() > CACHE {
                    cache.clear();
                }
                for (offset, row) in rows.into_iter().enumerate() {
                    let position = first + offset as u32;
                    // A row that is not the key at its position means the library
                    // changed; a reload follows.
                    if keys.get(position as usize) != Some(&row.key) {
                        continue;
                    }
                    let row = Rc::new(row);
                    cache.insert(row.key, row.clone());
                    let items = me
                        .waiting
                        .borrow_mut()
                        .remove(&position)
                        .unwrap_or_default();
                    for item in items.iter().filter_map(glib::WeakRef::upgrade) {
                        item.fill(row.clone());
                    }
                }
            }
            // Positions the backend did not answer are dropped.
            me.waiting
                .borrow_mut()
                .retain(|p, _| *p < first || *p > last);
            if !me.waiting.borrow().is_empty() && !me.scheduled.replace(true) {
                let again = me.clone();
                glib::idle_add_local_once(move || again.fetch());
            }
        });
    }
}

mod imp_model {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct AlbumListModel {
        pub(super) fetcher: Rc<Fetcher>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumListModel {
        const NAME: &'static str = "HkAlbumListModel";
        type Type = super::AlbumListModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for AlbumListModel {}

    impl ListModelImpl for AlbumListModel {
        fn item_type(&self) -> glib::Type {
            AlbumItem::static_type()
        }

        fn n_items(&self) -> u32 {
            u32::try_from(self.fetcher.keys.borrow().len()).unwrap_or(u32::MAX)
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let key = *self.fetcher.keys.borrow().get(position as usize)?;
            let row = self.fetcher.cache.borrow().get(&key).cloned();
            Some(AlbumItem::new(key, position, row).upcast())
        }
    }
}

glib::wrapper! {
    /// Album keys in the grid's order; 8 bytes per album.
    pub(crate) struct AlbumListModel(ObjectSubclass<imp_model::AlbumListModel>)
        @implements gio::ListModel;
}

impl AlbumListModel {
    /// An empty model that fetches rows from `backend`.
    pub(crate) fn new(backend: &Backend) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().fetcher.backend.replace(Some(backend.clone()));
        obj
    }

    /// Replaces the keys with those `query` gave.
    pub(crate) fn set_keys(&self, query: AlbumQuery, keys: Vec<AlbumKey>) {
        let removed = self.n_items();
        let added = u32::try_from(keys.len()).unwrap_or(u32::MAX);
        let fetcher = &self.imp().fetcher;
        fetcher.reset(query);
        fetcher.keys.replace(keys);
        self.items_changed(0, removed, added);
    }

    /// Fetches the row of `item` unless it has one; it emits `loaded` then.
    pub(crate) fn ensure_row(&self, item: &AlbumItem) {
        if item.row().is_none() {
            self.imp().fetcher.request(item);
        }
    }

    /// How many batches were fetched (for tests).
    pub(crate) fn fetches(&self) -> u64 {
        self.imp().fetcher.fetches.get()
    }
}
