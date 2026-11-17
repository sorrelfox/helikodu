// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A lazy list model over a vector of IDs. GTK asks for items by position;
//! an item object is created only then, and its row data only when a widget
//! is bound to it.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::catalog;

thread_local! {
    /// How many item objects GTK asked for, per model kind.
    pub static ITEMS_MATERIALISED: Cell<u64> = const { Cell::new(0) };
    /// How many rows had their data built (a database fetch in the real player).
    pub static ROWS_FETCHED: Cell<u64> = const { Cell::new(0) };
}

mod imp_row {
    use super::*;

    #[derive(Default)]
    pub struct RowItem {
        pub id: Cell<u32>,
        pub position: Cell<u32>,
        pub track: OnceCell<Rc<catalog::Track>>,
        pub album: OnceCell<Rc<catalog::Album>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RowItem {
        const NAME: &'static str = "HkSpikeRowItem";
        type Type = super::RowItem;
    }

    impl ObjectImpl for RowItem {}
}

glib::wrapper! {
    /// One item of a lazy model: an ID and, once bound, its row data.
    pub struct RowItem(ObjectSubclass<imp_row::RowItem>);
}

impl RowItem {
    fn new(id: u32, position: u32) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().id.set(id);
        obj.imp().position.set(position);
        obj
    }

    pub fn id(&self) -> u32 {
        self.imp().id.get()
    }

    pub fn position(&self) -> u32 {
        self.imp().position.get()
    }

    pub fn track(&self) -> Rc<catalog::Track> {
        self.imp()
            .track
            .get_or_init(|| {
                ROWS_FETCHED.with(|c| c.set(c.get() + 1));
                Rc::new(catalog::track(self.id()))
            })
            .clone()
    }

    pub fn album(&self) -> Rc<catalog::Album> {
        self.imp()
            .album
            .get_or_init(|| {
                ROWS_FETCHED.with(|c| c.set(c.get() + 1));
                Rc::new(catalog::album(self.id()))
            })
            .clone()
    }
}

mod imp_model {
    use super::*;

    #[derive(Default)]
    pub struct IdModel {
        pub ids: RefCell<Vec<u32>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IdModel {
        const NAME: &'static str = "HkSpikeIdModel";
        type Type = super::IdModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for IdModel {}

    impl ListModelImpl for IdModel {
        fn item_type(&self) -> glib::Type {
            super::RowItem::static_type()
        }

        fn n_items(&self) -> u32 {
            self.ids.borrow().len() as u32
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let id = *self.ids.borrow().get(position as usize)?;
            ITEMS_MATERIALISED.with(|c| c.set(c.get() + 1));
            Some(super::RowItem::new(id, position).upcast())
        }
    }
}

glib::wrapper! {
    /// List model that holds only IDs (4 bytes per row).
    pub struct IdModel(ObjectSubclass<imp_model::IdModel>) @implements gio::ListModel;
}

impl IdModel {
    pub fn new(ids: Vec<u32>) -> Self {
        let obj: Self = glib::Object::new();
        *obj.imp().ids.borrow_mut() = ids;
        obj
    }
}
