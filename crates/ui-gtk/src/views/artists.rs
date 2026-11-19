// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library, Artists: the album artists at the left, under letter headers in
//! the order of their sort names, and the artist page at the right: a hero
//! with Play all and Shuffle, then the tabs Releases (albums, EPs and singles,
//! appears on, and the most played tracks) and All tracks (the track table).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::backend::{
    AlbumKey, AlbumRow, ArtistDetail, ArtistKey, ArtistRow, Backend, PlayerRequest, TrackKey,
    TrackRow, TrackScope,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::{AlbumItem, CoverCache};
use crate::playback::{Change, Playback, PlayingKeys};
use crate::views::actions::{self, Target};
use crate::views::album_tile::AlbumTile;
use crate::views::artist_photo;
use crate::views::drag::DraggedTracks;
use crate::views::genres::list_pane;
use crate::views::tracks::TracksView;
use crate::widgets::avatar::Avatar;
use crate::widgets::empty_state::empty_state;
use crate::widgets::pane_layouts::HeadLayout;
use crate::widgets::{TitleLine, WrapBox, label};

type Open = Box<dyn Fn(ArtistKey)>;
type OpenAlbum = Rc<dyn Fn(AlbumKey)>;

/// A button with an optional icon; `primary` takes the accent.
pub(crate) fn button(text: &str, icon: Option<&str>, primary: bool) -> gtk::Button {
    let button = gtk::Button::new();
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if let Some(icon) = icon {
        line.append(&gtk::Image::from_icon_name(icon));
    }
    line.append(&gtk::Label::new(Some(text)));
    button.set_child(Some(&line));
    button.add_css_class("hk-btn");
    if primary {
        button.add_css_class("primary");
    }
    button.update_property(&[gtk::accessible::Property::Label(text)]);
    button
}

/// The letter an artist is listed under.
fn letter(row: &ArtistRow) -> String {
    row.sort_name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| {
            if c.is_alphabetic() {
                c.to_uppercase().collect()
            } else {
                "#".to_owned()
            }
        })
        .unwrap_or_else(|| "#".to_owned())
}

/// The Artists view.
pub(crate) struct ArtistsView {
    pub(crate) root: gtk::Box,
    count: gtk::Label,
    list: gtk::ListBox,
    detail: gtk::Stack,
    avatar: Avatar,
    name: gtk::Label,
    summary: gtk::Label,
    pages: gtk::Stack,
    releases: gtk::FlowBox,
    most_played: gtk::ListBox,
    most_played_box: gtk::Box,
    /// The Plays and Length columns: as wide as their header or their widest
    /// value, whichever is wider, in the header and in every row.
    figures: [gtk::SizeGroup; 2],
    tracks: Rc<TracksView>,
    backend: Backend,
    covers: Rc<CoverCache>,
    rows: RefCell<Vec<ArtistRow>>,
    detail_data: RefCell<Option<ArtistDetail>>,
    current: Cell<Option<ArtistKey>>,
    wanted: Cell<Option<ArtistKey>>,
    loaded: Cell<bool>,
    quiet: Cell<bool>,
    generation: Cell<u64>,
    playing_album: Cell<Option<AlbumKey>>,
    playing_tracks: Cell<PlayingKeys>,
    tiles: RefCell<Vec<AlbumTile>>,
    letters: Rc<RefCell<std::collections::HashMap<String, String>>>,
    /// The avatar of each list row, by artist.
    row_avatars: RefCell<HashMap<ArtistKey, Avatar>>,
    /// The photo items of the hero's more menu, for the artist shown.
    photo_menu: gio::Menu,
    /// Whether a pass over the visible rows' photos is scheduled.
    photo_pass: Cell<bool>,
    open: RefCell<Option<Open>>,
    open_album: OpenAlbum,
}

impl std::fmt::Debug for ArtistsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtistsView").finish_non_exhaustive()
    }
}

impl ArtistsView {
    /// The view over `backend`; `open_album` shows an album page.
    pub(crate) fn new(
        backend: &Backend,
        playback: &Rc<Playback>,
        covers: &Rc<CoverCache>,
        open_album: impl Fn(AlbumKey) + 'static,
    ) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let filter = gtk::MenuButton::new();
        filter.set_icon_name("view-sort-descending-symbolic");
        filter.add_css_class("hk-menu");
        let order = gio::Menu::new();
        order.append(Some(&tr("Name")), Some("artists.order::name"));
        order.append(Some(&tr("Number of albums")), Some("artists.order::count"));
        crate::widgets::menu_names::set_menu_later(&filter, &order);
        filter.set_tooltip_text(Some(&tr("Sort the artists")));
        filter.update_property(&[gtk::accessible::Property::Label(&tr("Sort the artists"))]);
        let (pane, count, list) = list_pane(&tr("Artists"), filter.upcast_ref());
        list.add_css_class("hk-artist-list");
        root.append(&pane);

        let detail = gtk::Stack::new();
        detail.set_hexpand(true);
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let hero = gtk::Box::new(gtk::Orientation::Horizontal, 26);
        hero.add_css_class("hk-artist-hero");
        let avatar = Avatar::new(104);
        hero.append(&avatar);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        titles.set_valign(gtk::Align::Center);
        titles.append(&label(&tr("Album artist"), &["hk-section-label"]));
        let name = label("", &["hk-hero-title"]);
        name.set_accessible_role(gtk::AccessibleRole::Heading);
        // A long name continues on a second line before it is cut short.
        name.set_single_line_mode(false);
        name.set_wrap(true);
        name.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        name.set_lines(2);
        titles.append(&name);
        let summary = label("", &["hk-t3"]);
        summary.set_wrap(true);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::None);
        summary.set_single_line_mode(false);
        titles.append(&summary);
        hero.append(&titles);
        let play = button(&tr("Play all"), Some("media-playback-start-symbolic"), true);
        let shuffle = button(
            &tr("Shuffle"),
            Some("media-playlist-shuffle-symbolic"),
            false,
        );
        let more = gtk::MenuButton::new();
        more.set_icon_name("view-more-symbolic");
        more.add_css_class("hk-more");
        more.set_tooltip_text(Some(&tr("More")));
        more.update_property(&[gtk::accessible::Property::Label(&tr("More"))]);
        let more_menu = gio::Menu::new();
        more_menu.append(Some(&tr("Play next")), Some("artists.play-next"));
        more_menu.append(Some(&tr("Add to queue")), Some("artists.enqueue"));
        let photo_menu = gio::Menu::new();
        more_menu.append_section(None, &photo_menu);
        crate::widgets::menu_names::set_menu_later(&more, &more_menu);
        // Continues on a second line when the page is too narrow for it.
        let buttons = WrapBox::new(8);
        buttons.set_valign(gtk::Align::Center);
        buttons.append(&play);
        buttons.append(&shuffle);
        buttons.append(&more);
        hero.append(&buttons);
        HeadLayout::install(&hero);
        page.append(&hero);

        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 22);
        tabs.add_css_class("hk-tabs");
        tabs.add_css_class("hk-page-tabs");
        let releases_tab = gtk::ToggleButton::with_label(&tr("Releases"));
        let tracks_tab = gtk::ToggleButton::with_label(&tr("All tracks"));
        tracks_tab.set_group(Some(&releases_tab));
        releases_tab.set_active(true);
        tabs.append(&releases_tab);
        tabs.append(&tracks_tab);
        page.append(&tabs);

        let pages = gtk::Stack::new();
        pages.set_vexpand(true);
        pages.set_hhomogeneous(false);
        let releases_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        releases_page.add_css_class("hk-releases");
        let releases = gtk::FlowBox::new();
        releases.set_selection_mode(gtk::SelectionMode::None);
        releases.set_column_spacing(36);
        releases.set_row_spacing(24);
        releases.set_homogeneous(false);
        releases.set_valign(gtk::Align::Start);
        releases.set_max_children_per_line(8);
        releases_page.append(&releases);
        let most_played_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        most_played_box.add_css_class("hk-most-played");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        header.add_css_class("hk-table-header");
        let figures = [
            gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
        ];
        for (i, (text, width, expand)) in [
            (tr("Most played"), 0, true),
            (tr("Album"), 0, true),
            (tr("Plays"), 44, false),
            (tr("Length"), 44, false),
        ]
        .into_iter()
        .enumerate()
        {
            let l = label(&text, &["hk-section-label"]);
            l.set_hexpand(expand);
            if expand {
                // As in the rows, so the columns line up.
                l.set_max_width_chars(1);
            }
            if width > 0 {
                l.set_size_request(width, -1);
                l.set_xalign(1.0);
                // The header is never cut: it sets the column's width.
                l.set_ellipsize(gtk::pango::EllipsizeMode::None);
                figures[i - 2].add_widget(&l);
            }
            header.append(&l);
        }
        most_played_box.append(&header);
        let most_played = gtk::ListBox::new();
        most_played.add_css_class("hk-tracks");
        most_played.add_css_class("hk-compact");
        most_played.set_activate_on_single_click(false);
        most_played.update_property(&[gtk::accessible::Property::Label(&tr("Most played"))]);
        most_played_box.append(&most_played);
        releases_page.append(&most_played_box);
        let releases_scroller = gtk::ScrolledWindow::new();
        releases_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        releases_scroller.set_child(Some(&releases_page));
        pages.add_named(&releases_scroller, Some("releases"));
        let tracks = TracksView::new(backend, playback, true);
        pages.add_named(&tracks.root, Some("tracks"));
        page.append(&pages);
        detail.add_named(&page, Some("artist"));
        detail.add_named(
            &empty_state(
                "avatar-default-symbolic",
                &tr("No artist chosen"),
                &tr("Choose an artist to see their releases."),
                &[],
            ),
            Some("none"),
        );
        detail.set_visible_child_name("none");
        root.append(&detail);
        crate::views::genres::list_and_detail(&root);

        let view = Rc::new(Self {
            root,
            count,
            list,
            detail,
            avatar,
            name,
            summary,
            pages,
            releases,
            most_played,
            most_played_box,
            figures,
            tracks,
            backend: backend.clone(),
            covers: covers.clone(),
            rows: RefCell::new(Vec::new()),
            detail_data: RefCell::new(None),
            current: Cell::new(None),
            wanted: Cell::new(None),
            loaded: Cell::new(false),
            quiet: Cell::new(false),
            generation: Cell::new(0),
            playing_album: Cell::new(None),
            playing_tracks: Cell::new(PlayingKeys::default()),
            tiles: RefCell::new(Vec::new()),
            letters: Rc::default(),
            row_avatars: RefCell::default(),
            photo_menu,
            photo_pass: Cell::new(false),
            open: RefCell::new(None),
            open_album: Rc::new(open_album),
        });
        view.connect(playback, &play, &shuffle, &releases_tab, &tracks_tab);
        view
    }

    fn connect(
        self: &Rc<Self>,
        playback: &Rc<Playback>,
        play: &gtk::Button,
        shuffle: &gtk::Button,
        releases_tab: &gtk::ToggleButton,
        tracks_tab: &gtk::ToggleButton,
    ) {
        let letters = self.letters.clone();
        self.list.set_header_func(move |row, before| {
            let letters = letters.borrow();
            let letter_of = |r: &gtk::ListBoxRow| letters.get(r.widget_name().as_str()).cloned();
            let this = letter_of(row);
            let show = this.is_some() && before.is_none_or(|b| letter_of(b) != this);
            let header = this.filter(|_| show).map(|l| label(&l, &["hk-letter"]));
            row.set_header(header.as_ref());
        });
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
                open(ArtistKey(key));
            }
        });
        for (button, random) in [(play, false), (shuffle, true)] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade()
                    && let Some(artist) = view.current.get()
                {
                    view.backend.send(PlayerRequest::PlayArtist {
                        artist,
                        shuffle: random,
                    });
                }
            });
        }
        let weak = Rc::downgrade(self);
        releases_tab.connect_toggled(move |tab| {
            if let Some(view) = weak.upgrade()
                && tab.is_active()
            {
                view.pages.set_visible_child_name("releases");
            }
        });
        let weak = Rc::downgrade(self);
        tracks_tab.connect_toggled(move |tab| {
            if let Some(view) = weak.upgrade()
                && tab.is_active()
            {
                view.pages.set_visible_child_name("tracks");
                view.show_tracks();
            }
        });
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
            let mut rows = view.rows.borrow().clone();
            if name == "count" {
                rows.sort_by_key(|r| std::cmp::Reverse(r.album_count));
            } else {
                rows.sort_by_key(|r| r.sort_name.to_lowercase());
            }
            view.rows.replace(rows);
            view.fill(name != "count");
        });
        group.add_action(&order);
        for (name, next) in [("play-next", true), ("enqueue", false)] {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                let Some(view) = weak.upgrade() else { return };
                let (Some(artist), Some(window)) = (
                    view.current.get(),
                    view.root.root().and_downcast::<crate::window::MainWindow>(),
                ) else {
                    return;
                };
                actions::play(
                    &window,
                    view.backend.clone(),
                    Target::Artist(artist),
                    Some(next),
                );
            });
            group.add_action(&action);
        }
        for (name, set) in [("set-photo", true), ("remove-photo", false)] {
            let action = gio::SimpleAction::new(name, Some(glib::VariantTy::UINT64));
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, target| {
                let (Some(view), Some(key)) = (weak.upgrade(), target.and_then(|t| t.get::<u64>()))
                else {
                    return;
                };
                if set {
                    view.choose_photo(ArtistKey(key));
                } else {
                    view.change_photo(ArtistKey(key), None);
                }
            });
            group.add_action(&action);
        }
        self.root.insert_action_group("artists", Some(&group));
        self.connect_avatar();
        if let Some(scroller) = self
            .list
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>()
        {
            let weak = Rc::downgrade(self);
            scroller.vadjustment().connect_value_changed(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.schedule_photo_pass();
                }
            });
        }
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            if let Some(row) = view.list.row_at_y(y as i32)
                && let Ok(key) = row.widget_name().parse::<u64>()
            {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                let key = ArtistKey(key);
                if let Some(menu) = actions::popup(&view.list, Target::Artist(key), Some((x, y)))
                    .and_then(|p| p.menu_model())
                    .and_downcast::<gio::Menu>()
                {
                    menu.append_section(
                        None,
                        &artist_photo::menu_section(key, view.own_photo(key)),
                    );
                }
            }
        });
        self.list.add_controller(click);
        let weak = Rc::downgrade(self);
        self.most_played.connect_row_activated(move |_, row| {
            let Some(view) = weak.upgrade() else { return };
            let detail = view.detail_data.borrow();
            let Some(detail) = detail.as_ref() else {
                return;
            };
            let index = row.index().max(0) as usize;
            view.backend.send(PlayerRequest::PlayTracks {
                tracks: detail.most_played.iter().map(|t| t.key).collect(),
                start: index,
                title: detail.row.name.clone(),
                shuffle: false,
            });
        });
        let weak = Rc::downgrade(self);
        playback.connect(move |player, change| {
            let Some(view) = weak.upgrade() else { return };
            match change {
                Change::Track => {
                    view.playing_album.set(player.album());
                    for tile in view.tiles.borrow().iter() {
                        tile.cover()
                            .set_playing(tile.key().is_some() && tile.key() == player.album());
                    }
                }
                Change::Playing
                    if view.playing_tracks.replace(player.playing) != player.playing =>
                {
                    view.mark_most_played();
                }
                _ => {}
            }
        });
    }

    /// Calls `open` when the user picks an artist.
    pub(crate) fn connect_open(&self, open: impl Fn(ArtistKey) + 'static) {
        self.open.replace(Some(Box::new(open)));
    }

    /// Shows the list, with `artist` selected (or the one shown before).
    pub(crate) fn show(self: &Rc<Self>, artist: Option<ArtistKey>) {
        if artist.is_some() {
            self.wanted.set(artist);
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
            let rows = backend.query(|l| l.artists()).await.unwrap_or_default();
            if let Some(view) = weak.upgrade() {
                view.rows.replace(rows);
                view.fill(true);
            }
        });
    }

    fn fill(self: &Rc<Self>, letters: bool) {
        self.quiet.set(true);
        crate::widgets::remove_rows(&self.list);
        self.row_avatars.borrow_mut().clear();
        let rows = self.rows.borrow().clone();
        self.count.set_text(&crate::format::count(rows.len()));
        {
            let mut map = self.letters.borrow_mut();
            map.clear();
            if letters {
                map.extend(rows.iter().map(|a| (a.key.0.to_string(), letter(a))));
            }
        }
        for artist in &rows {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&artist.key.0.to_string());
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let avatar = Avatar::new(26);
            avatar.set_name(&artist.name);
            avatar.set_photo(&self.covers, artist.photo);
            self.row_avatars
                .borrow_mut()
                .insert(artist.key, avatar.clone());
            line.append(&avatar);
            let name = label(&artist.name, &[]);
            name.set_hexpand(true);
            line.append(&name);
            line.append(&label(
                &crate::format::count(artist.album_count),
                &["hk-count"],
            ));
            row.set_child(Some(&line));
            row.update_property(&[gtk::accessible::Property::Label(&fill(
                &ntr("{}, {} album", "{}, {} albums", artist.album_count),
                &[&artist.name, &artist.album_count.to_string()],
            ))]);
            self.list.append(&row);
        }
        self.list.invalidate_headers();
        self.schedule_photo_pass();
        self.quiet.set(false);
        let wanted = self.wanted.get().or_else(|| rows.first().map(|r| r.key));
        self.select(wanted);
    }

    fn select(self: &Rc<Self>, artist: Option<ArtistKey>) {
        let Some(artist) = artist else {
            self.detail.set_visible_child_name("none");
            return;
        };
        self.wanted.set(Some(artist));
        self.quiet.set(true);
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && row.widget_name() == artist.0.to_string()
            {
                self.list.select_row(Some(row));
                // The focus follows within the list, but is not taken from
                // elsewhere (such as the sidebar the view was opened from).
                let in_list = self
                    .list
                    .root()
                    .and_then(|root| root.focus())
                    .is_some_and(|focus| focus.is_ancestor(&self.list));
                if self.list.is_mapped() && in_list {
                    row.grab_focus();
                }
            }
            child = widget.next_sibling();
        }
        self.quiet.set(false);
        if self.current.get() == Some(artist) && self.detail_data.borrow().is_some() {
            return;
        }
        self.current.set(Some(artist));
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let detail = backend.query(move |l| l.artist(artist)).await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() == generation {
                view.show_detail(detail.ok().flatten());
            }
        });
    }

    fn show_detail(self: &Rc<Self>, detail: Option<ArtistDetail>) {
        let Some(detail) = detail else {
            self.detail.set_visible_child_name("none");
            return;
        };
        self.detail.set_visible_child_name("artist");
        self.avatar.set_name(&detail.row.name);
        self.avatar.set_photo(&self.covers, detail.row.photo);
        self.show_photo_menu(&detail.row);
        self.name.set_text(&detail.row.name);
        self.name.set_tooltip_text(Some(&detail.row.name));
        let mut parts = Vec::new();
        if !detail.albums.is_empty() {
            parts.push(fill(
                &ntr("{} album", "{} albums", detail.albums.len()),
                &[&crate::format::count(detail.albums.len())],
            ));
        }
        if !detail.singles.is_empty() {
            parts.push(fill(
                &ntr(
                    "{} EP or single",
                    "{} EPs and singles",
                    detail.singles.len(),
                ),
                &[&crate::format::count(detail.singles.len())],
            ));
        }
        parts.push(fill(
            &ntr("{} track", "{} tracks", detail.row.track_count),
            &[&crate::format::count(detail.row.track_count)],
        ));
        parts.push(format::minutes(detail.duration));
        self.summary.set_text(&parts.join(" · "));
        while let Some(child) = self.releases.first_child() {
            self.releases.remove(&child);
        }
        self.tiles.borrow_mut().clear();
        for (title, albums) in [
            (tr("Albums"), &detail.albums),
            (tr("EPs and singles"), &detail.singles),
            (tr("Appears on"), &detail.appears_on),
        ] {
            if albums.is_empty() {
                continue;
            }
            let group = gtk::Box::new(gtk::Orientation::Vertical, 10);
            group.append(&label(&title, &["hk-section-label"]));
            // The tiles of a group wrap too, so a long group never widens the page.
            let row = gtk::FlowBox::new();
            row.set_selection_mode(gtk::SelectionMode::None);
            row.set_column_spacing(20);
            row.set_row_spacing(20);
            row.set_homogeneous(true);
            row.set_max_children_per_line(u32::try_from(albums.len().min(6)).unwrap_or(1));
            row.set_halign(gtk::Align::Start);
            row.set_valign(gtk::Align::Start);
            for album in albums.iter().take(24) {
                row.append(&self.tile(album));
            }
            group.append(&row);
            self.releases.append(&group);
            // A group is not a stop for the keyboard; its tiles are.
            if let Some(cell) = group.parent() {
                cell.set_focusable(false);
            }
        }
        self.detail_data.replace(Some(detail));
        self.show_most_played();
        if self.pages.visible_child_name().as_deref() == Some("tracks") {
            self.show_tracks();
        }
    }

    /// A tile in the cell of its group that takes the keyboard focus and
    /// carries the album's name.
    fn tile(self: &Rc<Self>, album: &AlbumRow) -> gtk::FlowBoxChild {
        let tile = AlbumTile::default();
        let cell = gtk::FlowBoxChild::new();
        cell.set_child(Some(&tile));
        cell.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{} by {}"),
            &[&album.title, &album.artist],
        ))]);
        tile.cover().set_shape(160, 6.0);
        tile.set_hexpand(false);
        tile.set_size_request(160, -1);
        let item = AlbumItem::new(album.key, 0, Some(Rc::new(album.clone())));
        tile.bind(&item, &self.covers);
        tile.cover()
            .set_playing(self.playing_album.get() == Some(album.key));
        let key = album.key;
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let weak = Rc::downgrade(self);
        let weak_tile = tile.downgrade();
        click.connect_released(move |gesture, presses, x, y| {
            let (Some(view), Some(tile)) = (weak.upgrade(), weak_tile.upgrade()) else {
                return;
            };
            match gesture.current_button() {
                gdk::BUTTON_SECONDARY => {
                    actions::popup(&tile, Target::Album(key), Some((x, y)));
                }
                gdk::BUTTON_PRIMARY if presses == 2 => {
                    view.backend.send(PlayerRequest::PlayAlbum {
                        album: key,
                        start: 0,
                        shuffle: false,
                    });
                }
                gdk::BUTTON_PRIMARY => (view.open_album)(key),
                _ => {}
            }
        });
        tile.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        let weak_tile = tile.downgrade();
        keys.connect_key_pressed(move |_, pressed, _, state| {
            let (Some(view), Some(tile)) = (weak.upgrade(), weak_tile.upgrade()) else {
                return glib::Propagation::Proceed;
            };
            if actions::is_menu_key(pressed, state) {
                actions::popup(&tile, Target::Album(key), None);
            } else if matches!(pressed, gdk::Key::Return | gdk::Key::KP_Enter) {
                (view.open_album)(key);
            } else {
                return glib::Propagation::Proceed;
            }
            glib::Propagation::Stop
        });
        cell.add_controller(keys);
        let title = album.title.clone();
        DraggedTracks::source(&tile, move |_, _| {
            Some((
                DraggedTracks {
                    album: Some(key),
                    ..DraggedTracks::default()
                },
                title.clone(),
            ))
        });
        self.tiles.borrow_mut().push(tile);
        cell
    }

    fn show_most_played(&self) {
        while let Some(child) = self.most_played.first_child() {
            self.most_played.remove(&child);
        }
        let detail = self.detail_data.borrow();
        let tracks: &[TrackRow] = detail.as_ref().map_or(&[], |d| &d.most_played);
        self.most_played_box.set_visible(!tracks.is_empty());
        for track in tracks {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&track.key.0.to_string());
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            let title = TitleLine::new(&track.title, &[]);
            title.set_copies(track.key, track.copies);
            line.append(&title);
            let album = label(&track.album_title, &["hk-t2"]);
            album.set_hexpand(true);
            album.set_max_width_chars(1);
            line.append(&album);
            for (group, (text, width)) in self.figures.iter().zip([
                (track.plays.to_string(), 44),
                (track.duration.map(format::duration).unwrap_or_default(), 44),
            ]) {
                let l = label(&text, &["hk-t2", "hk-numeric"]);
                l.set_xalign(1.0);
                l.set_size_request(width, -1);
                group.add_widget(&l);
                line.append(&l);
            }
            row.set_child(Some(&line));
            row.update_property(&[gtk::accessible::Property::Label(&fill(
                &tr("{} · {}"),
                &[&track.title, &track.album_title],
            ))]);
            self.most_played.append(&row);
        }
        self.mark_most_played();
    }

    /// Marks the row of the playing track in Most played: the playing-row
    /// look, with the indicator before the title as the table has no number
    /// column.
    fn mark_most_played(&self) {
        let playing = self.playing_tracks.get();
        let mut child = self.most_played.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() else {
                continue;
            };
            let on = row
                .widget_name()
                .parse::<u64>()
                .is_ok_and(|key| playing.contains(TrackKey(key)));
            for class in ["playing", "hk-playing"] {
                if on {
                    row.add_css_class(class);
                } else {
                    row.remove_css_class(class);
                }
            }
            if let Some(title) = row
                .child()
                .and_then(|line| line.first_child())
                .and_downcast::<TitleLine>()
            {
                title.set_playing(on, true);
            }
        }
    }

    fn show_tracks(&self) {
        if let Some(artist) = self.current.get() {
            let name = self.name.text();
            self.tracks.set_scope(TrackScope::Artist(artist), &name);
        }
    }

    /// The hero avatar: its menu on a right click, and a picture dropped on it
    /// becomes the photo.
    fn connect_avatar(self: &Rc<Self>) {
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            let (Some(key), Some(hero)) = (view.current.get(), view.avatar.parent()) else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let menu = artist_photo::menu_section(key, view.own_photo(key));
            let popover = gtk::PopoverMenu::from_model(Some(&menu));
            crate::widgets::menu_names::name_menus(&popover);
            popover.add_css_class("hk-context-menu");
            popover.set_parent(&hero);
            popover.set_has_arrow(false);
            popover.set_halign(gtk::Align::Start);
            let at = view
                .avatar
                .compute_point(&hero, &gtk::graphene::Point::new(x as f32, y as f32))
                .unwrap_or_else(|| gtk::graphene::Point::new(0.0, 0.0));
            popover.set_pointing_to(Some(&gdk::Rectangle::new(
                at.x() as i32,
                at.y() as i32,
                1,
                1,
            )));
            popover.connect_closed(|p| {
                let p = p.clone();
                glib::idle_add_local_once(move || p.unparent());
            });
            actions::popup_when_built(&popover);
        });
        self.avatar.add_controller(click);
        let weak = Rc::downgrade(self);
        self.avatar
            .add_controller(artist_photo::drop_target(move |path| {
                if let Some(view) = weak.upgrade()
                    && let Some(key) = view.current.get()
                {
                    view.change_photo(key, Some(path));
                }
            }));
    }

    /// Asks again, once on the next frame, for the photos of the rows in view
    /// that are still waiting, so that they are made first: the library makes
    /// a bounded number of the latest requests, and a long list asks for more.
    fn schedule_photo_pass(self: &Rc<Self>) {
        if self.photo_pass.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        self.list.add_tick_callback(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.photo_pass.set(false);
                view.photos_in_view();
            }
            glib::ControlFlow::Break
        });
    }

    fn photos_in_view(&self) {
        let Some(scroller) = self.list.ancestor(gtk::ScrolledWindow::static_type()) else {
            return;
        };
        let height = scroller.height() as f32;
        let visible: Vec<_> = self
            .row_avatars
            .borrow()
            .values()
            .filter_map(|avatar| {
                let wanted = avatar.waiting()?;
                let bounds = avatar.compute_bounds(&scroller)?;
                (bounds.y() + bounds.height() >= 0.0 && bounds.y() <= height).then_some(wanted)
            })
            .collect();
        if !visible.is_empty() {
            self.covers.prioritise(&visible);
        }
    }

    /// Whether `artist` has a photo chosen in the app.
    fn own_photo(&self, artist: ArtistKey) -> bool {
        self.rows
            .borrow()
            .iter()
            .any(|r| r.key == artist && r.own_photo)
    }

    /// The photo items of the more menu, for `row`.
    fn show_photo_menu(&self, row: &ArtistRow) {
        self.photo_menu.remove_all();
        let items = artist_photo::menu_section(row.key, row.own_photo);
        for i in 0..items.n_items() {
            self.photo_menu
                .append_item(&gio::MenuItem::from_model(&items, i));
        }
    }

    /// Asks for a picture to become the photo of `artist`.
    fn choose_photo(self: &Rc<Self>, artist: ArtistKey) {
        let name = self
            .rows
            .borrow()
            .iter()
            .find(|r| r.key == artist)
            .map(|r| r.name.clone())
            .unwrap_or_default();
        let weak = Rc::downgrade(self);
        artist_photo::choose(self.root.upcast_ref(), &name, move |path| {
            if let Some(view) = weak.upgrade() {
                view.change_photo(artist, Some(path));
            }
        });
    }

    /// Makes the picture at `path` the photo of `artist`, or forgets the photo
    /// chosen for them; then shows the photo they have now.
    pub(crate) fn change_photo(self: &Rc<Self>, artist: ArtistKey, path: Option<PathBuf>) {
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| match path {
                    Some(path) => l.set_artist_photo(artist, &path),
                    None => l.remove_artist_photo(artist),
                })
                .await;
            let Some(view) = weak.upgrade() else { return };
            match result {
                Ok(row) => view.photo_changed(&row),
                Err(e) => {
                    if let Some(window) =
                        view.root.root().and_downcast::<crate::window::MainWindow>()
                    {
                        window.toast(&fill(&tr("Cannot set the artist photo: {}"), &[&e.0]), true);
                    }
                }
            }
        });
    }

    /// Shows the new photo of `row` in the list and, if it is shown, the hero.
    fn photo_changed(&self, row: &ArtistRow) {
        for known in self
            .rows
            .borrow_mut()
            .iter_mut()
            .filter(|r| r.key == row.key)
        {
            known.photo = row.photo;
            known.own_photo = row.own_photo;
        }
        if let Some(avatar) = self.row_avatars.borrow().get(&row.key) {
            avatar.set_photo(&self.covers, row.photo);
        }
        if self.current.get() == Some(row.key) {
            if let Some(detail) = self.detail_data.borrow_mut().as_mut() {
                detail.row.photo = row.photo;
                detail.row.own_photo = row.own_photo;
            }
            self.avatar.set_photo(&self.covers, row.photo);
            self.show_photo_menu(row);
        }
    }

    /// The photo of the hero and of the list row of `artist`: what each asked
    /// for and whether it is drawn (for tests).
    pub(crate) fn photo_state(&self, artist: ArtistKey) -> [(bool, bool); 2] {
        let row = self
            .row_avatars
            .borrow()
            .get(&artist)
            .map_or((false, false), |a| (a.photo().is_some(), a.shows_photo()));
        let hero = if self.current.get() == Some(artist) {
            (self.avatar.photo().is_some(), self.avatar.shows_photo())
        } else {
            (false, false)
        };
        [hero, row]
    }

    /// The artist shown (for tests).
    pub(crate) fn current(&self) -> Option<ArtistKey> {
        self.current.get()
    }

    /// Reloads after the library changed.
    pub(crate) fn library_changed(self: &Rc<Self>) {
        if self.loaded.get() {
            self.detail_data.replace(None);
            self.load();
            self.tracks.library_changed();
        }
    }

    /// Shows a changed rating in the track table.
    pub(crate) fn ratings_changed(&self, keys: &[TrackKey], stars: Option<u8>) {
        self.tracks.ratings_changed(keys, stars);
    }

    /// The selection of the All tracks tab.
    pub(crate) fn target(&self) -> Option<Target> {
        if self.pages.visible_child_name().as_deref() == Some("tracks") {
            self.tracks.target()
        } else {
            self.current.get().map(Target::Artist)
        }
    }

    /// The list, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.list.clone().upcast()
    }

    /// The number of artists listed (for tests).
    pub(crate) fn count(&self) -> usize {
        self.rows.borrow().len()
    }
}
