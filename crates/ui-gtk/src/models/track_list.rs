// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track list model of the track table: a `GListModel` over track keys in
//! the order a query gave. Rows are fetched by key, a batch of neighbours at a
//! time, on a worker thread when a cell is bound to them, and kept in a cache
//! that starts again when it is full.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::backend::{Backend, TrackKey, TrackRow};

/// How many neighbouring rows one fetch asks for at most.
const BATCH: usize = 96;
/// How many rows the model keeps; beyond that the cache starts again.
const CACHE: usize = 4096;

mod imp_item {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct TrackItem {
        pub(super) key: Cell<u64>,
        pub(super) position: Cell<u32>,
        pub(super) row: RefCell<Option<Rc<TrackRow>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackItem {
        const NAME: &'static str = "HkTrackItem";
        type Type = super::TrackItem;
    }

    impl ObjectImpl for TrackItem {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![glib::subclass::Signal::builder("loaded").build()])
        }
    }
}

glib::wrapper! {
    /// One track of a list: its key, its position and, once fetched, its row.
    pub(crate) struct TrackItem(ObjectSubclass<imp_item::TrackItem>);
}

impl TrackItem {
    /// An item at `position`, with its row if it is known.
    pub(crate) fn new(key: TrackKey, position: u32, row: Option<Rc<TrackRow>>) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().key.set(key.0);
        obj.imp().position.set(position);
        obj.imp().row.replace(row);
        obj
    }

    /// The track.
    pub(crate) fn key(&self) -> TrackKey {
        TrackKey(self.imp().key.get())
    }

    /// Its position in the list.
    pub(crate) fn position(&self) -> u32 {
        self.imp().position.get()
    }

    /// Its row, if fetched.
    pub(crate) fn row(&self) -> Option<Rc<TrackRow>> {
        self.imp().row.borrow().clone()
    }

    /// Replaces the row and tells the cells.
    pub(crate) fn fill(&self, row: Rc<TrackRow>) {
        self.imp().row.replace(Some(row));
        self.emit_by_name::<()>("loaded", &[]);
    }

    /// Calls `f` when the row arrives or changes.
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
    keys: RefCell<Rc<[TrackKey]>>,
    cache: RefCell<HashMap<TrackKey, Rc<TrackRow>>>,
    waiting: RefCell<BTreeMap<u32, Vec<glib::WeakRef<TrackItem>>>>,
    scheduled: Cell<bool>,
    generation: Cell<u64>,
    fetches: Cell<u64>,
}

impl Fetcher {
    fn request(self: &Rc<Self>, item: &TrackItem) {
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

    fn fetch(self: Rc<Self>) {
        self.scheduled.set(false);
        let Some(backend) = self.backend.borrow().clone() else {
            return;
        };
        // The first waiting position and the waiting ones within a batch of it;
        // the batch covers the positions in between too, which scrolling
        // usually asks for next.
        let Some(first) = self.waiting.borrow().keys().next().copied() else {
            return;
        };
        let last = self
            .waiting
            .borrow()
            .range(first..first.saturating_add(BATCH as u32))
            .next_back()
            .map_or(first, |(p, _)| *p);
        let keys: Vec<TrackKey> = {
            let all = self.keys.borrow();
            let end = (last as usize + 1).min(all.len());
            all.get(first as usize..end).unwrap_or_default().to_vec()
        };
        self.fetches.set(self.fetches.get() + 1);
        let generation = self.generation.get();
        let me = self.clone();
        glib::spawn_future_local(async move {
            let rows = backend
                .query(move |library| library.track_rows(&keys))
                .await
                .unwrap_or_default();
            if me.generation.get() != generation {
                return;
            }
            let rows: HashMap<TrackKey, Rc<TrackRow>> =
                rows.into_iter().map(|r| (r.key, Rc::new(r))).collect();
            {
                let mut cache = me.cache.borrow_mut();
                if cache.len() + rows.len() > CACHE {
                    cache.clear();
                }
                cache.extend(rows.iter().map(|(k, r)| (*k, r.clone())));
            }
            let done: Vec<(u32, Vec<glib::WeakRef<TrackItem>>)> = {
                let mut waiting = me.waiting.borrow_mut();
                let positions: Vec<u32> = waiting.range(first..=last).map(|(p, _)| *p).collect();
                positions
                    .into_iter()
                    .filter_map(|p| waiting.remove(&p).map(|items| (p, items)))
                    .collect()
            };
            for (_, items) in done {
                for item in items.iter().filter_map(glib::WeakRef::upgrade) {
                    if let Some(row) = rows.get(&item.key()) {
                        item.fill(row.clone());
                    }
                }
            }
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
    pub(crate) struct TrackListModel {
        pub(super) fetcher: Rc<Fetcher>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackListModel {
        const NAME: &'static str = "HkTrackListModel";
        type Type = super::TrackListModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for TrackListModel {}

    impl ListModelImpl for TrackListModel {
        fn item_type(&self) -> glib::Type {
            TrackItem::static_type()
        }

        fn n_items(&self) -> u32 {
            u32::try_from(self.fetcher.keys.borrow().len()).unwrap_or(u32::MAX)
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let key = *self.fetcher.keys.borrow().get(position as usize)?;
            let row = self.fetcher.cache.borrow().get(&key).cloned();
            Some(TrackItem::new(key, position, row).upcast())
        }
    }
}

glib::wrapper! {
    /// Track keys in a list's order; 8 bytes per track.
    pub(crate) struct TrackListModel(ObjectSubclass<imp_model::TrackListModel>)
        @implements gio::ListModel;
}

impl TrackListModel {
    /// An empty model that fetches rows from `backend`.
    pub(crate) fn new(backend: &Backend) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().fetcher.backend.replace(Some(backend.clone()));
        obj
    }

    /// Replaces the keys. The cached rows stay, since a key's row does not
    /// depend on its position.
    pub(crate) fn set_keys(&self, keys: Vec<TrackKey>) {
        let removed = self.n_items();
        let added = u32::try_from(keys.len()).unwrap_or(u32::MAX);
        let fetcher = &self.imp().fetcher;
        fetcher.waiting.borrow_mut().clear();
        fetcher.generation.set(fetcher.generation.get() + 1);
        fetcher.keys.replace(keys.into());
        self.items_changed(0, removed, added);
    }

    /// Forgets the cached rows, after the library changed.
    pub(crate) fn forget_rows(&self) {
        self.imp().fetcher.cache.borrow_mut().clear();
    }

    /// The keys, in order.
    pub(crate) fn keys(&self) -> Rc<[TrackKey]> {
        self.imp().fetcher.keys.borrow().clone()
    }

    /// The cached row of `key`.
    pub(crate) fn cached(&self, key: TrackKey) -> Option<Rc<TrackRow>> {
        self.imp().fetcher.cache.borrow().get(&key).cloned()
    }

    /// Stores a changed row, such as a new rating.
    pub(crate) fn update_row(&self, row: TrackRow) {
        self.imp()
            .fetcher
            .cache
            .borrow_mut()
            .insert(row.key, Rc::new(row));
    }

    /// Fetches the row of `item` unless it has one; it emits `loaded` then.
    pub(crate) fn ensure_row(&self, item: &TrackItem) {
        if item.row().is_none() {
            self.imp().fetcher.request(item);
        }
    }

    /// Fetches the row of `item` again, as after an edit; the row it has stays
    /// until the new one comes and `loaded` is emitted.
    pub(crate) fn refetch(&self, item: &TrackItem) {
        self.imp().fetcher.request(item);
    }

    /// How many batches were fetched (for tests).
    pub(crate) fn fetches(&self) -> u64 {
        self.imp().fetcher.fetches.get()
    }
}
