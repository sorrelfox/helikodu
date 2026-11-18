// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library, Albums: the toolbar (title, count, quick filter, sort) and the grid
//! of album tiles over an [`AlbumListModel`].
//!
//! A click on a tile opens the album page; Enter plays the album. The context
//! menu (right click, Shift+F10 or the Menu key) offers both.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{CompositeTemplate, gdk, gio, glib};

use crate::backend::{AlbumFilter, AlbumKey, AlbumQuery, AlbumSort, Backend, PlayerRequest};
use crate::i18n::tr;
use crate::models::{AlbumItem, AlbumListModel, CoverCache};
use crate::playback::{Change, Playback};
use crate::views::album_tile::AlbumTile;
use crate::widgets::empty_state::empty_state;

type OpenHandler = Rc<dyn Fn(AlbumKey)>;

mod imp {
    use super::*;

    #[derive(Default, CompositeTemplate)]
    #[template(resource = "/io/github/sorrelfox/Helikodu/ui/albums.ui")]
    pub(crate) struct AlbumsView {
        #[template_child]
        pub(super) count: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) filter_all: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(super) filter_lossless: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(super) filter_hires: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(super) sort_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub(super) content: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) empty_slot: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) grid: TemplateChild<gtk::GridView>,
        pub(super) backend: RefCell<Option<Backend>>,
        pub(super) model: RefCell<Option<AlbumListModel>>,
        pub(super) covers: RefCell<Option<Rc<CoverCache>>>,
        pub(super) query: Cell<AlbumQuery>,
        pub(super) generation: Cell<u64>,
        pub(super) playing: Cell<Option<AlbumKey>>,
        pub(super) tiles: RefCell<Vec<glib::WeakRef<AlbumTile>>>,
        pub(super) open: RefCell<Option<OpenHandler>>,
        pub(super) loaded: Cell<bool>,
        pub(super) click: Cell<u64>,
        pub(super) visibility_pass: Cell<bool>,
    }

    impl std::fmt::Debug for AlbumsView {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("AlbumsView")
                .field("query", &self.query)
                .finish()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumsView {
        const NAME: &'static str = "HkAlbumsView";
        type Type = super::AlbumsView;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumsView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            for (button, filter) in [
                (self.filter_all.get(), AlbumFilter::All),
                (self.filter_lossless.get(), AlbumFilter::Lossless),
                (self.filter_hires.get(), AlbumFilter::HiRes),
            ] {
                let weak = obj.downgrade();
                button.connect_toggled(move |button| {
                    if let Some(view) = weak.upgrade()
                        && button.is_active()
                    {
                        let mut query = view.imp().query.get();
                        query.filter = filter;
                        view.set_query(query);
                    }
                });
            }
            let group = gio::SimpleActionGroup::new();
            let sort = gio::SimpleAction::new_stateful(
                "sort",
                Some(glib::VariantTy::STRING),
                &"artist".to_variant(),
            );
            let weak = obj.downgrade();
            sort.connect_activate(move |action, target| {
                let (Some(view), Some(name)) = (weak.upgrade(), target.and_then(|t| t.str()))
                else {
                    return;
                };
                action.set_state(&name.to_variant());
                let (sort, label) = match name {
                    "title" => (AlbumSort::Title, tr("Title")),
                    "year" => (AlbumSort::Year, tr("Year")),
                    "added" => (AlbumSort::DateAdded, tr("Date added")),
                    _ => (AlbumSort::AlbumArtist, tr("Album artist")),
                };
                view.imp().sort_button.set_label(&label);
                let mut query = view.imp().query.get();
                query.sort = sort;
                view.set_query(query);
            });
            group.add_action(&sort);
            for (name, play) in [("play", true), ("open", false)] {
                let action = gio::SimpleAction::new(name, Some(glib::VariantTy::UINT64));
                let weak = obj.downgrade();
                action.connect_activate(move |_, target| {
                    let (Some(view), Some(key)) =
                        (weak.upgrade(), target.and_then(|t| t.get::<u64>()))
                    else {
                        return;
                    };
                    if play {
                        view.play(AlbumKey(key));
                    } else {
                        view.open(AlbumKey(key));
                    }
                });
                group.add_action(&action);
            }
            obj.insert_action_group("albums", Some(&group));
        }
    }

    impl WidgetImpl for AlbumsView {}
    impl BoxImpl for AlbumsView {}
}

glib::wrapper! {
    /// The album grid view.
    pub(crate) struct AlbumsView(ObjectSubclass<imp::AlbumsView>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for AlbumsView {
    fn default() -> Self {
        glib::Object::new()
    }
}

fn context_menu(key: AlbumKey) -> gio::Menu {
    let menu = gio::Menu::new();
    let target = key.0.to_variant();
    let play = gio::MenuItem::new(Some(&tr("Play")), None);
    play.set_action_and_target_value(Some("albums.play"), Some(&target));
    menu.append_item(&play);
    let queue = gio::MenuItem::new(Some(&tr("Add to queue")), Some("albums.add-to-queue"));
    menu.append_item(&queue);
    let section = gio::Menu::new();
    let open = gio::MenuItem::new(Some(&tr("Go to album")), None);
    open.set_action_and_target_value(Some("albums.open"), Some(&target));
    section.append_item(&open);
    section.append(Some(&tr("Go to artist")), Some("albums.go-to-artist"));
    menu.append_section(None, &section);
    menu
}

impl AlbumsView {
    /// Connects the view to `backend`; `open` shows an album page.
    pub(crate) fn bind(
        &self,
        backend: &Backend,
        playback: &Rc<Playback>,
        covers: &Rc<CoverCache>,
        open: impl Fn(AlbumKey) + 'static,
    ) {
        let imp = self.imp();
        imp.backend.replace(Some(backend.clone()));
        imp.covers.replace(Some(covers.clone()));
        imp.open.replace(Some(Rc::new(open)));
        let model = AlbumListModel::new(backend);
        imp.grid.set_factory(Some(&self.factory(&model)));
        imp.grid
            .set_model(Some(&gtk::NoSelection::new(Some(model.clone()))));
        let weak = self.downgrade();
        imp.grid.connect_activate(move |grid, position| {
            let Some(view) = weak.upgrade() else { return };
            if let Some(item) = grid
                .model()
                .and_then(|m| m.item(position))
                .and_downcast::<AlbumItem>()
            {
                view.play(item.key());
            }
        });
        imp.model.replace(Some(model));
        let quality = backend.library().supports_quality_filters();
        for button in [&imp.filter_lossless, &imp.filter_hires] {
            button.set_sensitive(quality);
            if !quality {
                button.set_tooltip_text(Some(&tr("The library does not tell formats apart yet")));
            }
        }
        let weak = self.downgrade();
        imp.grid.add_tick_callback(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.visibility_tick();
            }
            glib::ControlFlow::Continue
        });
        if let Some(adjustment) = imp.grid.vadjustment() {
            let weak = self.downgrade();
            adjustment.connect_value_changed(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.imp().visibility_pass.set(true);
                }
            });
        }
        let weak = self.downgrade();
        playback.connect(move |view, change| {
            if let Some(this) = weak.upgrade()
                && change == Change::Track
            {
                this.imp().playing.set(view.album());
                this.mark_playing();
            }
        });
        self.reload();
    }

    fn factory(&self, model: &AlbumListModel) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let weak = self.downgrade();
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let tile = AlbumTile::default();
            let Some(view) = weak.upgrade() else { return };
            view.attach_gestures(&tile);
            view.imp().tiles.borrow_mut().push(tile.downgrade());
            item.set_child(Some(&tile));
        });
        let weak = self.downgrade();
        let model = model.downgrade();
        factory.connect_bind(move |_, item| {
            let (Some(view), Some(model)) = (weak.upgrade(), model.upgrade()) else {
                return;
            };
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(tile), Some(album)) = (
                item.child().and_downcast::<AlbumTile>(),
                item.item().and_downcast::<AlbumItem>(),
            ) else {
                return;
            };
            if let Some(covers) = view.imp().covers.borrow().as_ref() {
                tile.bind(&album, covers);
            }
            model.ensure_row(&album);
            tile.cover()
                .set_playing(view.imp().playing.get() == Some(album.key()));
            view.imp().visibility_pass.set(true);
        });
        factory.connect_unbind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            if let Some(tile) = item.child().and_downcast::<AlbumTile>() {
                tile.unbind();
            }
        });
        factory
    }

    fn attach_gestures(&self, tile: &AlbumTile) {
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let weak_tile = tile.downgrade();
        let weak = self.downgrade();
        click.connect_released(move |gesture, presses, x, y| {
            let (Some(tile), Some(view)) = (weak_tile.upgrade(), weak.upgrade()) else {
                return;
            };
            let Some(key) = tile.key() else { return };
            match gesture.current_button() {
                // A click opens the album once it is clear that no second click
                // follows; a double click plays it instead.
                gdk::BUTTON_PRIMARY if presses == 1 => view.open_later(key),
                gdk::BUTTON_PRIMARY if presses == 2 => {
                    view.cancel_open();
                    view.play(key);
                }
                gdk::BUTTON_SECONDARY => view.show_menu(&tile, key, Some((x, y))),
                _ => {}
            }
        });
        tile.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak_tile = tile.downgrade();
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            let (Some(tile), Some(view)) = (weak_tile.upgrade(), weak.upgrade()) else {
                return glib::Propagation::Proceed;
            };
            let menu_key = key == gdk::Key::Menu
                || (key == gdk::Key::F10 && state.contains(gdk::ModifierType::SHIFT_MASK));
            match (menu_key, tile.key()) {
                (true, Some(album)) => {
                    view.show_menu(&tile, album, None);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        tile.add_controller(keys);
    }

    fn show_menu(&self, tile: &AlbumTile, key: AlbumKey, at: Option<(f64, f64)>) {
        let popover = gtk::PopoverMenu::from_model(Some(&context_menu(key)));
        popover.set_parent(tile);
        popover.set_has_arrow(false);
        if let Some((x, y)) = at {
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        }
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
    }

    fn open_later(&self, key: AlbumKey) {
        let imp = self.imp();
        let generation = imp.click.get() + 1;
        imp.click.set(generation);
        let delay = gtk::Settings::default()
            .map_or(400, |s| s.gtk_double_click_time())
            .max(100);
        let weak = self.downgrade();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(u64::try_from(delay).unwrap_or(400)),
            move || {
                if let Some(view) = weak.upgrade()
                    && view.imp().click.get() == generation
                {
                    view.open(key);
                }
            },
        );
    }

    fn cancel_open(&self) {
        let imp = self.imp();
        imp.click.set(imp.click.get() + 1);
    }

    /// The visibility pass of the covers: once per frame while tiles wait,
    /// the visible ones ask again so they are made first.
    fn visibility_tick(&self) {
        let imp = self.imp();
        if !imp.visibility_pass.replace(false) {
            return;
        }
        let Some(covers) = imp.covers.borrow().clone() else {
            return;
        };
        let viewport = imp.grid.get();
        let (w, h) = (f64::from(viewport.width()), f64::from(viewport.height()));
        let mut visible = Vec::new();
        imp.tiles.borrow_mut().retain(|weak| {
            let Some(tile) = weak.upgrade() else {
                return false;
            };
            if let Some(waiting) = tile.waiting_cover()
                && tile.is_mapped()
                && let Some(b) = tile.compute_bounds(&viewport)
            {
                let (x, y) = (f64::from(b.x()), f64::from(b.y()));
                let (bw, bh) = (f64::from(b.width()), f64::from(b.height()));
                if x + bw > 0.0 && y + bh > 0.0 && x < w && y < h {
                    visible.push(waiting);
                }
            }
            true
        });
        covers.prioritise(&visible);
    }

    fn mark_playing(&self) {
        let playing = self.imp().playing.get();
        self.imp().tiles.borrow_mut().retain(|w| {
            let Some(tile) = w.upgrade() else {
                return false;
            };
            tile.cover()
                .set_playing(tile.key().is_some() && tile.key() == playing);
            true
        });
    }

    fn set_query(&self, query: AlbumQuery) {
        if self.imp().query.replace(query) != query || !self.imp().loaded.get() {
            self.reload();
        }
    }

    /// Asks the backend for the album keys again.
    pub(crate) fn reload(&self) {
        let imp = self.imp();
        let Some(backend) = imp.backend.borrow().clone() else {
            return;
        };
        let generation = imp.generation.get() + 1;
        imp.generation.set(generation);
        let query = imp.query.get();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let keys = backend
                .query(move |library| library.album_keys(&query))
                .await;
            let Some(view) = weak.upgrade() else { return };
            let imp = view.imp();
            if imp.generation.get() != generation {
                return;
            }
            imp.loaded.set(true);
            let keys = keys.unwrap_or_default();
            imp.count.set_text(&keys.len().to_string());
            if keys.is_empty() {
                view.show_empty(query.filter == AlbumFilter::All);
            } else {
                imp.content.set_visible_child_name("grid");
            }
            if let Some(model) = imp.model.borrow().as_ref() {
                model.set_keys(query, keys);
            }
        });
    }

    fn show_empty(&self, library_empty: bool) {
        let imp = self.imp();
        while let Some(child) = imp.empty_slot.first_child() {
            imp.empty_slot.remove(&child);
        }
        let block = if library_empty {
            empty_state(
                "media-optical-symbolic",
                &tr("Your library is empty"),
                &tr(
                    "Scan a music folder with helikodu-cli, then start Helikodu again. Adding folders from here comes in a later version.",
                ),
                &[],
            )
        } else {
            empty_state(
                "media-optical-symbolic",
                &tr("No albums match"),
                &tr("No album in the library passes this filter."),
                &[],
            )
        };
        imp.empty_slot.append(&block);
        imp.content.set_visible_child_name("empty");
    }

    fn open(&self, key: AlbumKey) {
        let open = self.imp().open.borrow().clone();
        if let Some(open) = open {
            open(key);
        }
    }

    fn play(&self, key: AlbumKey) {
        if let Some(backend) = self.imp().backend.borrow().as_ref() {
            backend.send(PlayerRequest::PlayAlbum {
                album: key,
                start: 0,
                shuffle: false,
            });
        }
    }

    /// The grid, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.imp().grid.get().upcast()
    }

    /// The number of albums shown and the batches fetched (for tests).
    pub(crate) fn stats(&self) -> (u32, u64) {
        self.imp()
            .model
            .borrow()
            .as_ref()
            .map_or((0, 0), |m| (m.n_items(), m.fetches()))
    }
}
