// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library, Genres: the list of genres at the left, and the selected genre's
//! albums in a grid at the right. Selecting a genre runs a new album query;
//! the grid never filters in memory.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::{AlbumKey, AlbumScope, AlbumSort, Backend, GenreKey, GenreRow, PlayerRequest};
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::playback::Playback;
use crate::views::AlbumsView;
use crate::views::actions::{self, Target};
use crate::widgets::empty_state::empty_state;
use crate::widgets::label;

type Open = Box<dyn Fn(GenreKey)>;

/// The list pane shared by Genres and Artists: a header with the view title,
/// the count and a button, then a scrolled list.
pub(crate) fn list_pane(title: &str, button: &gtk::Widget) -> (gtk::Box, gtk::Label, gtk::ListBox) {
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.add_css_class("hk-list-pane");
    pane.set_size_request(264, -1);
    pane.set_hexpand(false);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("hk-pane-header");
    let heading = label(title, &["hk-view-title"]);
    heading.set_accessible_role(gtk::AccessibleRole::Heading);
    head.append(&heading);
    let count = label("", &["hk-t3", "hk-numeric"]);
    count.set_hexpand(true);
    count.set_valign(gtk::Align::Baseline);
    head.append(&count);
    button.set_valign(gtk::Align::Center);
    head.append(button);
    pane.append(&head);
    let list = gtk::ListBox::new();
    list.add_css_class("hk-list");
    list.add_css_class("hk-pane-list");
    list.update_property(&[gtk::accessible::Property::Label(title)]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&list));
    pane.append(&scroller);
    (pane, count, list)
}

/// The Genres view.
pub(crate) struct GenresView {
    pub(crate) root: gtk::Box,
    count: gtk::Label,
    list: gtk::ListBox,
    detail: gtk::Stack,
    name: gtk::Label,
    summary: gtk::Label,
    sort_button: gtk::MenuButton,
    albums: AlbumsView,
    backend: Backend,
    rows: RefCell<Vec<GenreRow>>,
    current: Cell<Option<GenreKey>>,
    wanted: Cell<Option<GenreKey>>,
    loaded: Cell<bool>,
    quiet: Cell<bool>,
    by_count: Cell<bool>,
    sort: Cell<AlbumSort>,
    open: RefCell<Option<Open>>,
}

impl std::fmt::Debug for GenresView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenresView").finish_non_exhaustive()
    }
}

/// A sort button with a menu of `items` (action target, label) in the group
/// `group`.`action`.
pub(crate) fn sort_button(label_text: &str, menu: &gio::Menu) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_label(label_text);
    button.add_css_class("hk-sort");
    button.set_menu_model(Some(menu));
    crate::widgets::menu_names::name_menus(&button);
    button.set_tooltip_text(Some(&tr("Sort")));
    button
}

impl GenresView {
    /// The view over `backend`; `open_album` shows an album page.
    pub(crate) fn new(
        backend: &Backend,
        playback: &Rc<Playback>,
        covers: &Rc<CoverCache>,
        open_album: impl Fn(AlbumKey) + 'static,
    ) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let order = gio::Menu::new();
        order.append(Some(&tr("Name")), Some("genres.order::name"));
        order.append(Some(&tr("Number of albums")), Some("genres.order::count"));
        let order_button = gtk::MenuButton::new();
        order_button.set_icon_name("view-sort-descending-symbolic");
        order_button.add_css_class("hk-menu");
        order_button.set_menu_model(Some(&order));
        crate::widgets::menu_names::name_menus(&order_button);
        order_button.set_tooltip_text(Some(&tr("Sort the genres")));
        order_button.update_property(&[gtk::accessible::Property::Label(&tr("Sort the genres"))]);
        let (pane, count, list) = list_pane(&tr("Genres"), order_button.upcast_ref());
        root.append(&pane);

        let detail = gtk::Stack::new();
        detail.set_hexpand(true);
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("hk-detail-header");
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        titles.append(&label(&tr("Genre"), &["hk-section-label"]));
        let name = label("", &["hk-hero-title", "small"]);
        name.set_accessible_role(gtk::AccessibleRole::Heading);
        titles.append(&name);
        let summary = label("", &["hk-t3"]);
        titles.append(&summary);
        header.append(&titles);
        let play = crate::views::artists::button(
            &tr("Play all"),
            Some("media-playback-start-symbolic"),
            true,
        );
        let shuffle = crate::views::artists::button(
            &tr("Shuffle"),
            Some("media-playlist-shuffle-symbolic"),
            false,
        );
        let sorts = gio::Menu::new();
        for (target, text) in [
            ("artist", tr("Album artist")),
            ("title", tr("Title")),
            ("year", tr("Year")),
            ("added", tr("Date added")),
        ] {
            sorts.append(Some(&text), Some(&format!("genres.sort::{target}")));
        }
        let sort_button = sort_button(&tr("Album artist"), &sorts);
        for w in [
            play.upcast_ref::<gtk::Widget>(),
            shuffle.upcast_ref(),
            sort_button.upcast_ref(),
        ] {
            w.set_valign(gtk::Align::End);
            header.append(w);
        }
        page.append(&header);
        let albums = AlbumsView::default();
        albums.bind(backend, playback, covers, open_album);
        albums.set_vexpand(true);
        page.append(&albums);
        detail.add_named(&page, Some("genre"));
        detail.add_named(
            &empty_state(
                "helikodu-tag-symbolic",
                &tr("No genre chosen"),
                &tr("Choose a genre to see its albums."),
                &[],
            ),
            Some("none"),
        );
        detail.set_visible_child_name("none");
        root.append(&detail);

        let view = Rc::new(Self {
            root,
            count,
            list,
            detail,
            name,
            summary,
            sort_button,
            albums,
            backend: backend.clone(),
            rows: RefCell::new(Vec::new()),
            current: Cell::new(None),
            wanted: Cell::new(None),
            loaded: Cell::new(false),
            quiet: Cell::new(false),
            by_count: Cell::new(false),
            sort: Cell::new(AlbumSort::AlbumArtist),
            open: RefCell::new(None),
        });
        view.connect(&play, &shuffle);
        view
    }

    fn connect(self: &Rc<Self>, play: &gtk::Button, shuffle: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_selected(move |_, row| {
            let (Some(view), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            if view.quiet.get() {
                return;
            }
            if let Ok(key) = row.widget_name().parse::<u64>()
                && let Some(open) = view.open.borrow().as_ref()
            {
                open(GenreKey(key));
            }
        });
        for (button, random) in [(play, false), (shuffle, true)] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade()
                    && let Some(genre) = view.current.get()
                {
                    view.backend.send(PlayerRequest::PlayGenre {
                        genre,
                        shuffle: random,
                    });
                }
            });
        }
        let group = gio::SimpleActionGroup::new();
        let order = gio::SimpleAction::new_stateful(
            "order",
            Some(glib::VariantTy::STRING),
            &"name".to_variant(),
        );
        let weak = Rc::downgrade(self);
        order.connect_activate(move |action, target| {
            let (Some(view), Some(name)) = (weak.upgrade(), target.and_then(|t| t.str())) else {
                return;
            };
            action.set_state(&name.to_variant());
            view.by_count.set(name == "count");
            view.fill();
        });
        group.add_action(&order);
        let sort = gio::SimpleAction::new_stateful(
            "sort",
            Some(glib::VariantTy::STRING),
            &"artist".to_variant(),
        );
        let weak = Rc::downgrade(self);
        sort.connect_activate(move |action, target| {
            let (Some(view), Some(name)) = (weak.upgrade(), target.and_then(|t| t.str())) else {
                return;
            };
            action.set_state(&name.to_variant());
            let (sort, text) = match name {
                "title" => (AlbumSort::Title, tr("Title")),
                "year" => (AlbumSort::Year, tr("Year")),
                "added" => (AlbumSort::DateAdded, tr("Date added")),
                _ => (AlbumSort::AlbumArtist, tr("Album artist")),
            };
            view.sort_button.set_label(&text);
            view.sort.set(sort);
            if let Some(genre) = view.current.get() {
                view.albums.set_embedded(AlbumScope::Genre(genre), sort);
            }
        });
        group.add_action(&sort);
        self.root.insert_action_group("genres", Some(&group));
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            if let Some(row) = view.list.row_at_y(y as i32)
                && let Ok(key) = row.widget_name().parse::<u64>()
            {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                actions::popup(&view.list, Target::Genre(GenreKey(key)), Some((x, y)));
            }
        });
        self.list.add_controller(click);
    }

    /// Calls `open` when the user picks a genre.
    pub(crate) fn connect_open(&self, open: impl Fn(GenreKey) + 'static) {
        self.open.replace(Some(Box::new(open)));
    }

    /// Shows the list, with `genre` selected (or the one shown before).
    pub(crate) fn show(self: &Rc<Self>, genre: Option<GenreKey>) {
        if genre.is_some() {
            self.wanted.set(genre);
        }
        if !self.loaded.get() {
            self.load();
        } else {
            self.select(self.wanted.get());
        }
    }

    fn load(self: &Rc<Self>) {
        self.loaded.set(true);
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let rows = backend.query(|l| l.genres()).await.unwrap_or_default();
            if let Some(view) = weak.upgrade() {
                view.rows.replace(rows);
                view.fill();
            }
        });
    }

    fn fill(self: &Rc<Self>) {
        self.quiet.set(true);
        crate::widgets::remove_rows(&self.list);
        let mut rows = self.rows.borrow().clone();
        if self.by_count.get() {
            rows.sort_by_key(|r| std::cmp::Reverse(r.album_count));
        }
        self.count.set_text(&crate::format::count(rows.len()));
        for genre in &rows {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&genre.key.0.to_string());
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let name = label(&genre.name, &[]);
            name.set_hexpand(true);
            line.append(&name);
            let count = label(&crate::format::count(genre.album_count), &["hk-count"]);
            line.append(&count);
            row.set_child(Some(&line));
            row.update_property(&[gtk::accessible::Property::Label(&fill(
                &ntr("{}, {} album", "{}, {} albums", genre.album_count),
                &[&genre.name, &genre.album_count.to_string()],
            ))]);
            self.list.append(&row);
        }
        self.quiet.set(false);
        let wanted = self.wanted.get().or_else(|| rows.first().map(|r| r.key));
        self.select(wanted);
    }

    fn select(self: &Rc<Self>, genre: Option<GenreKey>) {
        let Some(genre) = genre else {
            self.detail.set_visible_child_name("none");
            return;
        };
        self.wanted.set(Some(genre));
        self.quiet.set(true);
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && row.widget_name() == genre.0.to_string()
            {
                self.list.select_row(Some(row));
            }
            child = widget.next_sibling();
        }
        self.quiet.set(false);
        let Some(row) = self.rows.borrow().iter().find(|r| r.key == genre).cloned() else {
            return;
        };
        self.name.set_text(&row.name);
        let albums = fill(
            &ntr("{} album", "{} albums", row.album_count),
            &[&crate::format::count(row.album_count)],
        );
        let tracks = fill(
            &ntr("{} track", "{} tracks", row.track_count),
            &[&crate::format::count(row.track_count)],
        );
        self.summary
            .set_text(&fill(&tr("{} · {}"), &[&albums, &tracks]));
        self.detail.set_visible_child_name("genre");
        self.current.set(Some(genre));
        self.albums
            .set_embedded(AlbumScope::Genre(genre), self.sort.get());
    }

    /// Reloads after the library changed.
    pub(crate) fn library_changed(self: &Rc<Self>) {
        if self.loaded.get() {
            self.load();
            self.albums.reload();
        }
    }

    /// The list, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.list.clone().upcast()
    }

    /// The grid's tiles in view and those showing their cover (for tests).
    pub(crate) fn cover_stats(&self) -> (usize, usize) {
        self.albums.cover_stats()
    }

    /// The number of genres listed (for tests).
    pub(crate) fn count(&self) -> usize {
        self.rows.borrow().len()
    }
}
