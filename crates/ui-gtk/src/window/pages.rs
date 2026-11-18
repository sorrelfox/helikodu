// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The library pages of the window beyond Albums: Artists, Tracks, Genres,
//! Folders, playlists and search; the playlists in the sidebar; the context
//! menu target and its actions; the file actions of the main menu; and files
//! dropped on the window.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use super::toast_overlay::ToastAction;
use super::{MainWindow, Page};
use crate::backend::{
    ArtistKey, Backend, PinKey, PinRow, PlayerRequest, PlaylistKey, PlaylistRow, PlaylistType,
    TrackKey, TrackRow,
};
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::playback::Playback;
use crate::popovers::query_completion::QueryCompletion;
use crate::views::actions::{self, Origin, Target};
use crate::views::artists::ArtistsView;
use crate::views::category::{Category, CategoryView};
use crate::views::drag::{self, DraggedTracks};
use crate::views::folders::FoldersView;
use crate::views::genres::GenresView;
use crate::views::playlist::PlaylistView;
use crate::views::search::SearchView;
use crate::views::smart_playlist::SmartPlaylistView;
use crate::views::tracks::TracksView;

/// The views and state of the library pages.
pub(crate) struct Pages {
    pub(super) backend: Backend,
    pub(super) tracks: Rc<TracksView>,
    artists: Rc<ArtistsView>,
    genres: Rc<GenresView>,
    composers: Rc<CategoryView>,
    years: Rc<CategoryView>,
    folders: Rc<FoldersView>,
    playlist: Rc<PlaylistView>,
    pub(super) smart: Rc<SmartPlaylistView>,
    pub(super) covers: Rc<CoverCache>,
    pub(super) pins: RefCell<Vec<PinRow>>,
    search: Rc<SearchView>,
    search_entry: gtk::SearchEntry,
    completion: Rc<QueryCompletion>,
    target: RefCell<Option<Target>>,
    playlists: RefCell<Vec<PlaylistRow>>,
    undo: RefCell<Option<Rc<dyn Fn()>>>,
    before_search: RefCell<Option<Page>>,
}

impl std::fmt::Debug for Pages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pages").finish_non_exhaustive()
    }
}

/// The library pages this module shows.
const NAMES: &[&str] = &[
    "artists",
    "tracks",
    "genres",
    "composers",
    "years",
    "folders",
];

impl MainWindow {
    pub(super) fn pages(&self) -> Option<Rc<Pages>> {
        self.imp().pages.borrow().clone()
    }

    /// The sidebar.
    pub(super) fn imp_sidebar(&self) -> Option<Rc<super::sidebar::Sidebar>> {
        self.imp().sidebar.borrow().clone()
    }

    pub(super) fn setup_pages(
        &self,
        backend: &Backend,
        playback: &Rc<Playback>,
        covers: &Rc<CoverCache>,
    ) {
        let imp = self.imp();
        let weak = self.downgrade();
        let open_album = move |key| {
            if let Some(window) = weak.upgrade() {
                window.open_album(key);
            }
        };
        let tracks = TracksView::new(backend, playback, false);
        let artists = ArtistsView::new(backend, playback, covers, open_album.clone());
        let genres = GenresView::new(backend, playback, covers, open_album.clone());
        let composers = CategoryView::new(
            Category::Composers,
            backend,
            playback,
            covers,
            open_album.clone(),
        );
        let years = CategoryView::new(
            Category::Years,
            backend,
            playback,
            covers,
            open_album.clone(),
        );
        let folders = FoldersView::new(backend, playback);
        let playlist = PlaylistView::new(backend, playback, covers);
        let smart = SmartPlaylistView::new(backend, covers);
        smart.install_actions();
        let search = SearchView::new(backend, covers, playback);
        imp.views.add_named(&tracks.root, Some("tracks"));
        imp.views.add_named(&artists.root, Some("artists"));
        imp.views.add_named(&genres.root, Some("genres"));
        imp.views.add_named(&composers.root, Some("composers"));
        imp.views.add_named(&years.root, Some("years"));
        imp.views.add_named(&folders.root, Some("folders"));
        imp.views.add_named(&playlist.root, Some("playlist"));
        imp.views.add_named(&smart.root, Some("smart-playlist"));
        imp.views.add_named(&search.root, Some("search"));
        // Each page is as wide as it needs; a wide page must not widen the window.
        imp.views.set_hhomogeneous(false);
        if let Some(popover) = imp.menu_button.popover() {
            popover.set_has_arrow(false);
        }
        let search_entry = self.search_entry();
        let completion = QueryCompletion::new(&search_entry);
        let pages = Rc::new(Pages {
            backend: backend.clone(),
            tracks,
            artists,
            genres,
            composers,
            years,
            folders,
            playlist,
            smart,
            covers: covers.clone(),
            pins: RefCell::new(Vec::new()),
            search,
            search_entry,
            completion,
            target: RefCell::new(None),
            playlists: RefCell::new(Vec::new()),
            undo: RefCell::new(None),
            before_search: RefCell::new(None),
        });
        imp.pages.replace(Some(pages.clone()));

        let weak = self.downgrade();
        pages.artists.connect_open(move |key| {
            if let Some(window) = weak.upgrade() {
                window.go(Page::Artist(key), true);
            }
        });
        let weak = self.downgrade();
        pages.genres.connect_open(move |key| {
            if let Some(window) = weak.upgrade() {
                window.go(Page::Genre(key), true);
            }
        });
        let weak = self.downgrade();
        pages.search.connect_open(move |page| {
            if let Some(window) = weak.upgrade() {
                window.go(page, true);
            }
        });
        let weak = self.downgrade();
        pages.search.connect_show_all(move |text| {
            if let Some(window) = weak.upgrade() {
                window.show_all_tracks(&text);
            }
        });
        let weak = self.downgrade();
        pages.search.connect_clear(move || {
            if let Some(window) = weak.upgrade() {
                window.clear_search();
            }
        });
        let weak = self.downgrade();
        pages.search.connect_use_text(move |text| {
            if let Some(pages) = weak.upgrade().and_then(|w| w.pages()) {
                pages.search_entry.set_text(&text);
                pages.search_entry.grab_focus();
                pages.search_entry.set_position(-1);
            }
        });
        self.setup_search(&pages);
        self.guard_text_fields();
        actions::install(self);
        self.install_page_actions();
        self.setup_drops(backend);
        self.setup_queue_menu();
        self.install_playlist_actions();
        self.load_playlists(None);
        self.load_pins();
    }

    /// The search entry of the content header.
    pub(super) fn search_entry(&self) -> gtk::SearchEntry {
        let header = self.imp().back.parent().and_then(|b| b.parent());
        header
            .and_downcast::<gtk::CenterBox>()
            .and_then(|c| c.center_widget())
            .and_downcast::<gtk::SearchEntry>()
            .unwrap_or_default()
    }

    fn setup_search(&self, pages: &Rc<Pages>) {
        let entry = &pages.search_entry;
        add_shortcut_hint(entry);
        entry.set_sensitive(true);
        entry.set_tooltip_text(None);
        entry.set_search_delay(60);
        entry.update_property(&[gtk::accessible::Property::Label(&tr("Search the library"))]);
        entry.set_key_capture_widget(None::<&gtk::Widget>);
        let weak = self.downgrade();
        entry.connect_search_changed(move |entry| {
            let Some(window) = weak.upgrade() else { return };
            let text = entry.text().to_string();
            window.search_for(&text);
        });
        // The query font and the underline follow every keystroke, and so does
        // the completion.
        let weak = self.downgrade();
        entry.connect_changed(move |entry| {
            crate::query_text::mark(entry, &crate::query_text::read(&entry.text()));
            if let Some(pages) = weak.upgrade().and_then(|w| w.pages()) {
                pages.completion.update();
            }
        });
        let weak = self.downgrade();
        entry.connect_stop_search(move |_| {
            if let Some(window) = weak.upgrade() {
                window.clear_search();
            }
        });
        let weak = self.downgrade();
        entry.connect_activate(move |_| {
            if let Some(pages) = weak.upgrade().and_then(|w| w.pages()) {
                pages.search.activate_selected();
            }
        });
        // Up and Down move through the results without leaving the entry.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(pages) = weak.upgrade().and_then(|w| w.pages()) else {
                return glib::Propagation::Proceed;
            };
            if pages.completion.key(key) {
                return glib::Propagation::Stop;
            }
            let by = match key {
                gdk::Key::Down => 1,
                gdk::Key::Up => -1,
                _ => return glib::Propagation::Proceed,
            };
            pages.search.move_selection(by);
            glib::Propagation::Stop
        });
        entry.add_controller(keys);
    }

    /// Ctrl+A and Ctrl+Z belong to a text field while it has the focus: the
    /// window's shortcuts for them are lifted until it loses it. (Space is a
    /// shortcut of the window that yields to the focused widget anyway.)
    fn guard_text_fields(&self) {
        const KEYS: &[(&str, &str)] =
            &[("win.select-all", "<Control>a"), ("win.undo", "<Control>z")];
        self.connect_focus_widget_notify(|window| {
            let Some(application) = window.application() else {
                return;
            };
            let typing = gtk::prelude::GtkWindowExt::focus(window)
                .is_some_and(|w| w.is::<gtk::Text>() || w.is::<gtk::TextView>());
            // The keys come back as the settings give them.
            let settings = window
                .backend()
                .map(|b| b.services().settings().shortcuts.clone())
                .unwrap_or_default();
            for (action, key) in KEYS {
                let accels = application.accels_for_action(action);
                let has = accels.iter().any(|a| a == key);
                if typing && has {
                    let rest: Vec<&str> = accels
                        .iter()
                        .map(|a| a.as_str())
                        .filter(|a| a != key)
                        .collect();
                    application.set_accels_for_action(action, &rest);
                } else if !typing {
                    crate::shortcuts::apply_action(&application, action, &settings);
                }
            }
        });
    }

    fn search_for(&self, text: &str) {
        let Some(pages) = self.pages() else { return };
        if text.trim().is_empty() {
            if self.imp().views.visible_child_name().as_deref() == Some("search") {
                self.leave_search();
            }
            return;
        }
        if self.imp().views.visible_child_name().as_deref() != Some("search") {
            let current = self.current_page();
            pages.before_search.replace(current);
            self.go(Page::Search, true);
            pages.search.refresh_saved();
        }
        pages.search.search(text);
    }

    pub(super) fn current_page(&self) -> Option<Page> {
        let imp = self.imp();
        imp.history.borrow().get(imp.index.get()).copied()
    }

    /// Empties the search entry and goes back to where the search started.
    fn clear_search(&self) {
        let Some(pages) = self.pages() else { return };
        if !pages.search_entry.text().is_empty() {
            // Its change handler leaves the search.
            pages.search_entry.set_text("");
        } else {
            self.leave_search();
        }
    }

    fn leave_search(&self) {
        let Some(pages) = self.pages() else { return };
        let before = pages.before_search.take();
        self.go(before.unwrap_or(Page::Library("albums")), true);
    }

    pub(super) fn show_all_tracks(&self, text: &str) {
        let Some(pages) = self.pages() else { return };
        pages.before_search.replace(None);
        pages.tracks.filter_entry().set_text(text);
        pages.search_entry.set_text("");
        self.go(Page::Library("tracks"), true);
    }

    /// Whether `name` is one of the pages of this module.
    pub(super) fn has_page(&self, name: &str) -> bool {
        NAMES.contains(&name)
    }

    /// Shows `page`; returns the sidebar row to select ("" for none).
    pub(super) fn show_page(&self, page: Page) -> String {
        let Some(pages) = self.pages() else {
            return String::new();
        };
        let views = &self.imp().views;
        match page {
            Page::Library("tracks") => {
                pages
                    .tracks
                    .set_scope(crate::backend::TrackScope::All, &tr("Tracks"));
                views.set_visible_child_name("tracks");
                "tracks".into()
            }
            Page::Library("artists") => {
                pages.artists.show(None);
                views.set_visible_child_name("artists");
                "artists".into()
            }
            Page::Artist(key) => {
                pages.artists.show(Some(key));
                views.set_visible_child_name("artists");
                "artists".into()
            }
            Page::Library("genres") => {
                pages.genres.show(None);
                views.set_visible_child_name("genres");
                "genres".into()
            }
            Page::Genre(key) => {
                pages.genres.show(Some(key));
                views.set_visible_child_name("genres");
                "genres".into()
            }
            Page::Library("composers") => {
                pages.composers.show();
                views.set_visible_child_name("composers");
                "composers".into()
            }
            Page::Library("years") => {
                pages.years.show();
                views.set_visible_child_name("years");
                "years".into()
            }
            Page::Library("folders") => {
                pages.folders.show();
                views.set_visible_child_name("folders");
                "folders".into()
            }
            Page::Playlist(key) => {
                let smart = pages
                    .playlists
                    .borrow()
                    .iter()
                    .any(|p| p.key == key && p.kind == PlaylistType::Smart);
                if smart {
                    let editing = pages.smart.key() == Some(key) && pages.smart.is_editing();
                    pages.smart.show(key, editing);
                    views.set_visible_child_name("smart-playlist");
                } else {
                    pages.playlist.show(key);
                    views.set_visible_child_name("playlist");
                }
                format!("playlist:{}", key.0)
            }
            Page::Search => {
                views.set_visible_child_name("search");
                String::new()
            }
            Page::Library(_) | Page::Album(_) => String::new(),
        }
    }

    /// The widget F6 focuses in the page `name`.
    pub(super) fn page_focus_target(&self, name: &str) -> Option<gtk::Widget> {
        let pages = self.pages()?;
        Some(match name {
            "tracks" => pages.tracks.focus_target(),
            "artists" => pages.artists.focus_target(),
            "genres" => pages.genres.focus_target(),
            "composers" => pages.composers.focus_target(),
            "years" => pages.years.focus_target(),
            "folders" => pages.folders.focus_target(),
            "playlist" => pages.playlist.focus_target(),
            "smart-playlist" => pages.smart.focus_target(),
            "search" => pages.search_entry.clone().upcast(),
            "history" => self.history_view()?.focus_target(),
            "home" => self.home_focus_target()?,
            "statistics" => self.statistics_focus()?,
            _ => return None,
        })
    }

    /// Reloads the pages after the library changed.
    pub(super) fn library_changed(&self) {
        let Some(pages) = self.pages() else { return };
        pages.tracks.library_changed();
        pages.artists.library_changed();
        pages.genres.library_changed();
        pages.composers.library_changed();
        pages.years.library_changed();
        pages.folders.library_changed();
        pages.playlist.reload();
        pages.smart.reload();
        self.load_playlists(None);
        self.load_pins();
    }

    /// A sidebar row other than a library page: a playlist, a pin or "New
    /// playlist".
    pub(super) fn open_sidebar_row(&self, name: &str) {
        if name == "new-playlist" {
            let row = self.imp_sidebar().and_then(|s| s.row(name));
            match row {
                Some(row) => {
                    self.new_menu(row.upcast_ref());
                    // The row only opens the menu; the page stays selected.
                    let page = self.current_page();
                    if let Some(sidebar) = self.imp_sidebar() {
                        let selected = match page {
                            Some(Page::Playlist(key)) => Some(format!("playlist:{}", key.0)),
                            Some(Page::Library(name)) => Some(name.to_owned()),
                            _ => None,
                        };
                        sidebar.select(selected.as_deref());
                    }
                }
                None => self.new_playlist(),
            }
        } else if let Some(pin) = PinKey::from_row_name(name) {
            self.open_pin(pin);
        } else if let Some(key) = name
            .strip_prefix("playlist:")
            .and_then(|k| k.parse::<u64>().ok())
        {
            self.go(Page::Playlist(PlaylistKey(key)), true);
        }
    }

    /// The backend.
    pub(crate) fn backend(&self) -> Option<Backend> {
        self.imp().backend.borrow().clone()
    }

    /// The static playlists as the sidebar lists them.
    pub(crate) fn playlist_rows(&self) -> Vec<PlaylistRow> {
        self.pages()
            .map(|p| p.playlists.borrow().clone())
            .unwrap_or_default()
    }

    /// Sets what the context menu acts on, and which of its actions apply.
    pub(crate) fn set_target(&self, target: Option<Target>) {
        let Some(pages) = self.pages() else { return };
        if let Some(target) = &target {
            let (album, artist, remove) = actions::applies(target);
            for (name, on) in [
                ("sel-go-album", album),
                ("sel-go-artist", artist),
                ("sel-remove", remove),
            ] {
                if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                    action.set_enabled(on);
                }
            }
        }
        pages.target.replace(target);
    }

    /// Takes what the context menu acted on.
    pub(crate) fn take_target(&self) -> Option<Target> {
        self.pages().and_then(|p| p.target.take())
    }

    /// The selection of the visible view, for the keyboard actions.
    fn current_target(&self) -> Option<Target> {
        let pages = self.pages()?;
        match self.imp().views.visible_child_name().as_deref() {
            Some("tracks") => pages.tracks.target(),
            Some("artists") => pages.artists.target(),
            Some("playlist") => pages.playlist.target(),
            Some("smart-playlist") => pages.smart.target(),
            Some("folders") => pages.folders.target(),
            Some("search") => pages.search.target(),
            _ => None,
        }
    }

    /// Shows a toast with Undo; Ctrl+Z runs it while the toast is up.
    pub(crate) fn toast_undo(&self, text: &str, undo: impl Fn() + 'static) {
        let undo: Rc<dyn Fn()> = Rc::new(undo);
        if let Some(pages) = self.pages() {
            pages.undo.replace(Some(undo.clone()));
        }
        let weak = self.downgrade();
        let run = move || {
            if let Some(pages) = weak.upgrade().and_then(|w| w.pages()) {
                pages.undo.replace(None);
            }
            undo();
        };
        if let Some(toasts) = self.imp().toasts.borrow().as_ref() {
            toasts.show(
                text,
                Some(ToastAction {
                    label: tr("Undo"),
                    undo: true,
                    run: Box::new(run),
                }),
                false,
            );
        }
    }

    /// Reloads the playlists after a change, and the open one if it is `changed`.
    pub(crate) fn playlists_changed(&self, changed: Option<PlaylistKey>) {
        if let Some(pages) = self.pages()
            && changed.is_some()
        {
            if pages.playlist.key() == changed {
                pages.playlist.reload();
            }
            if pages.smart.key() == changed {
                pages.smart.reload();
            }
        }
        self.load_playlists(None);
        self.load_pins();
    }

    fn load_playlists(&self, then_rename: Option<PlaylistKey>) {
        let Some(pages) = self.pages() else { return };
        let backend = pages.backend.clone();
        // Desktop integrations that list the playlists read them again.
        backend.playlists_changed();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let lists = backend.query(|l| l.playlists()).await;
            let Some(window) = weak.upgrade() else { return };
            let Some(pages) = window.pages() else { return };
            let lists = match lists {
                Ok(lists) => lists,
                Err(error) => {
                    window.toast(&error.0, true);
                    return;
                }
            };
            pages.playlists.replace(lists.clone());
            if let Some(sidebar) = window.imp().sidebar.borrow().as_ref() {
                sidebar.set_playlists(&lists);
                let weak = window.downgrade();
                sidebar.connect_playlist_menu(move |widget, key| {
                    if let Some(window) = weak.upgrade() {
                        window.playlist_menu(widget, key);
                    }
                });
                let weak = window.downgrade();
                sidebar.connect_drop(move |key, dragged| {
                    if let Some(window) = weak.upgrade() {
                        window.drop_on_playlist(key, dragged, None);
                    }
                });
                let weak = window.downgrade();
                sidebar.connect_folder_drop(move |key, folder| {
                    if let Some(window) = weak.upgrade() {
                        window.move_playlist_to(key, Some(folder));
                    }
                });
            }
            let visible = window.imp().views.visible_child_name();
            if let Some(Page::Playlist(key)) = window.current_page()
                && matches!(visible.as_deref(), Some("playlist" | "smart-playlist"))
            {
                // A playlist opened before the list came shows on its own page.
                let smart = lists
                    .iter()
                    .any(|p| p.key == key && p.kind == PlaylistType::Smart);
                if smart != (visible.as_deref() == Some("smart-playlist")) {
                    window.go(Page::Playlist(key), false);
                }
                if lists.iter().any(|p| p.key == key) {
                    if let Some(sidebar) = window.imp().sidebar.borrow().as_ref() {
                        sidebar.select(Some(&format!("playlist:{}", key.0)));
                    }
                } else {
                    window.go(Page::Library("albums"), true);
                }
            }
            if let Some(key) = then_rename {
                window.rename_playlist(key);
            }
        });
    }

    /// Opens the playlist `key`.
    pub(crate) fn open_playlist(&self, key: PlaylistKey) {
        self.go(Page::Playlist(key), true);
    }

    /// Opens the page of the artist `key`.
    pub(crate) fn open_artist(&self, key: ArtistKey) {
        self.go(Page::Artist(key), true);
    }

    /// Opens the tag editor for `tracks`.
    pub(crate) fn open_tag_editor(&self, tracks: Vec<TrackKey>) {
        if tracks.is_empty() {
            return;
        }
        crate::dialogs::tag_editor::open(self, &tracks);
    }

    /// Shows changed ratings where tracks are listed.
    pub(crate) fn ratings_changed(&self, keys: &[TrackKey], stars: Option<u8>) {
        if let Some(pages) = self.pages() {
            pages.tracks.ratings_changed(keys, stars);
            pages.artists.ratings_changed(keys, stars);
            pages.playlist.ratings_changed(keys, stars);
        }
        if let Some(album) = self.imp().album.borrow().as_ref() {
            album.ratings_changed(keys, stars);
        }
    }

    /// Makes a new playlist, opens it and asks for its name.
    fn new_playlist(&self) {
        let Some(backend) = self.backend() else {
            return;
        };
        let taken: Vec<String> = self.playlist_rows().into_iter().map(|p| p.name).collect();
        let base = tr("New playlist");
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base} {n}")
                }
            })
            .find(|n| !taken.contains(n))
            .unwrap_or(base);
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let created = backend.query(move |l| l.create_playlist(&name)).await;
            let Some(window) = weak.upgrade() else { return };
            match created {
                Ok(key) => {
                    window.go(Page::Playlist(key), true);
                    window.load_playlists(Some(key));
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// Asks for a new name of the playlist `key` on its page, or of a folder
    /// beside its sidebar row.
    pub(crate) fn rename_playlist(&self, key: PlaylistKey) {
        let Some(pages) = self.pages() else { return };
        match self.playlist_row(key).map(|p| p.kind) {
            Some(PlaylistType::Folder) => self.ask_folder_name(key),
            Some(PlaylistType::Smart) => {
                self.open_smart_playlist(key, pages.smart.is_editing());
                let name = pages.smart.name_label();
                let current = self.playlist_row(key).map(|p| p.name).unwrap_or_default();
                let weak = self.downgrade();
                super::playlists::ask_name(
                    name.upcast_ref(),
                    &current,
                    &tr("Playlist name"),
                    move |name| {
                        if let Some(window) = weak.upgrade() {
                            window.rename_quietly(key, name);
                        }
                    },
                );
            }
            _ => {
                if pages.playlist.key() != Some(key) {
                    self.go(Page::Playlist(key), true);
                }
                pages.playlist.ask_name();
            }
        }
    }

    /// Opens the smart playlist `key`, viewing or editing its rules.
    pub(crate) fn open_smart_playlist(&self, key: PlaylistKey, editing: bool) {
        let Some(pages) = self.pages() else { return };
        pages.smart.show(key, editing);
        self.go(Page::Playlist(key), true);
        self.imp().views.set_visible_child_name("smart-playlist");
    }

    /// Asks before deleting the playlist `key`.
    pub(crate) fn delete_playlist(&self, key: PlaylistKey) {
        let name = self
            .playlist_rows()
            .into_iter()
            .find(|p| p.key == key)
            .map(|p| p.name)
            .unwrap_or_default();
        let folder = self
            .playlist_row(key)
            .is_some_and(|p| p.kind == PlaylistType::Folder);
        let (message, detail) = if folder {
            (
                fill(&tr("Delete the folder “{}”?"), &[&name]),
                tr("The playlists in it move up a level."),
            )
        } else {
            (
                fill(&tr("Delete the playlist “{}”?"), &[&name]),
                tr("Its tracks stay in the library. This cannot be undone."),
            )
        };
        let dialog = gtk::AlertDialog::builder()
            .message(message)
            .detail(detail)
            .buttons([tr("Cancel"), tr("Delete")])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let weak = self.downgrade();
        dialog.choose(Some(self), gio::Cancellable::NONE, move |answer| {
            let (Some(window), Ok(1)) = (weak.upgrade(), answer) else {
                return;
            };
            let Some(backend) = window.backend() else {
                return;
            };
            let weak = window.downgrade();
            glib::spawn_future_local(async move {
                let result = backend.query(move |l| l.delete_playlist(key)).await;
                if let Some(window) = weak.upgrade() {
                    match result {
                        Ok(()) => window.playlists_changed(None),
                        Err(error) => window.toast(&error.0, true),
                    }
                }
            });
        });
    }

    /// Asks for a file and exports the playlist `key` to it.
    pub(crate) fn export_playlist(&self, key: PlaylistKey) {
        let name = self
            .playlist_rows()
            .into_iter()
            .find(|p| p.key == key)
            .map(|p| p.name)
            .unwrap_or_default();
        // The extension of the name chooses the format; M3U8 when it names none.
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        for (title, suffix) in [
            (tr("M3U8 playlists"), "m3u8"),
            (tr("M3U playlists"), "m3u"),
            (tr("PLS playlists"), "pls"),
            (tr("XSPF playlists"), "xspf"),
        ] {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(&title));
            filter.add_suffix(suffix);
            filters.append(&filter);
        }
        let dialog = gtk::FileDialog::builder()
            .title(tr("Export playlist"))
            .initial_name(format!("{name}.m3u8"))
            .filters(&filters)
            .modal(true)
            .build();
        let weak = self.downgrade();
        dialog.save(Some(self), gio::Cancellable::NONE, move |file| {
            let (Some(window), Ok(Some(path))) = (weak.upgrade(), file.map(|f| f.path())) else {
                return;
            };
            let Some(backend) = window.backend() else {
                return;
            };
            let weak = window.downgrade();
            glib::spawn_future_local(async move {
                let result = backend.query(move |l| l.export_playlist(key, &path)).await;
                if let Some(window) = weak.upgrade() {
                    match result {
                        Ok(count) => window.toast(
                            &fill(
                                &ntr("{} track exported", "{} tracks exported", count),
                                &[&count.to_string()],
                            ),
                            false,
                        ),
                        Err(error) => window.toast(&error.0, true),
                    }
                }
            });
        });
    }

    fn import_playlist(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&tr("Playlists")));
        for suffix in ["m3u", "m3u8", "pls", "xspf"] {
            filter.add_suffix(suffix);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(tr("Import playlist"))
            .filters(&filters)
            .modal(true)
            .build();
        let weak = self.downgrade();
        dialog.open(Some(self), gio::Cancellable::NONE, move |file| {
            let (Some(window), Ok(Some(path))) = (weak.upgrade(), file.map(|f| f.path())) else {
                return;
            };
            let Some(backend) = window.backend() else {
                return;
            };
            let weak = window.downgrade();
            glib::spawn_future_local(async move {
                let result = backend.query(move |l| l.import_playlist(&path)).await;
                let Some(window) = weak.upgrade() else { return };
                match result {
                    Ok((key, name)) => {
                        window.playlists_changed(None);
                        window.go(Page::Playlist(key), true);
                        window.toast(&fill(&tr("Imported “{}”"), &[&name]), false);
                    }
                    Err(error) => window.toast(&error.0, true),
                }
            });
        });
    }

    fn open_files(&self, folder: bool) {
        let dialog = gtk::FileDialog::builder()
            .title(if folder {
                tr("Open folder")
            } else {
                tr("Open files")
            })
            .modal(true)
            .build();
        let weak = self.downgrade();
        let play = move |paths: Vec<PathBuf>| {
            if let Some(window) = weak.upgrade() {
                window.play_files(paths, false);
            }
        };
        if folder {
            dialog.select_folder(Some(self), gio::Cancellable::NONE, move |file| {
                if let Ok(Some(path)) = file.map(|f| f.path()) {
                    play(vec![path]);
                }
            });
        } else {
            dialog.open_multiple(Some(self), gio::Cancellable::NONE, move |files| {
                let Ok(files) = files else { return };
                let paths = (0..files.n_items())
                    .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                    .filter_map(|f| f.path())
                    .collect();
                play(paths);
            });
        }
    }

    /// Plays files, or adds them to the queue. A folder stands for the audio
    /// files in it.
    fn play_files(&self, paths: Vec<PathBuf>, enqueue: bool) {
        let Some(backend) = self.backend() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let files = gio::spawn_blocking(move || expand_folders(paths))
                .await
                .unwrap_or_default();
            if files.is_empty() {
                if let Some(window) = weak.upgrade() {
                    window.toast(&tr("There is nothing to play there."), false);
                }
                return;
            }
            backend.send(PlayerRequest::PlayFiles { files, enqueue });
        });
    }

    fn install_page_actions(&self) {
        let search = gio::ActionEntry::builder("search")
            .activate(|window: &Self, _, _| {
                if let Some(pages) = window.pages() {
                    pages.search_entry.grab_focus();
                }
            })
            .build();
        let properties = gio::ActionEntry::builder("track-properties")
            .activate(|window: &Self, _, _| {
                if let Some(target) = window.current_target() {
                    window.set_target(Some(target));
                    WidgetExt::activate_action(window, "win.sel-edit-tags", None).ok();
                }
            })
            .build();
        let copies = gio::ActionEntry::builder("track-copies")
            .activate(|window: &Self, _, _| {
                let track = window
                    .current_target()
                    .as_ref()
                    .and_then(crate::views::actions::with_copies);
                match track {
                    Some(track) => {
                        crate::tools::copies::open(window, track);
                    }
                    None => window.toast(&tr("The selected track has no copies."), false),
                }
            })
            .build();
        let select_all = gio::ActionEntry::builder("select-all")
            .activate(|window: &Self, _, _| {
                let Some(pages) = window.pages() else { return };
                match window.imp().views.visible_child_name().as_deref() {
                    Some("tracks") => pages.tracks.select_all(),
                    Some("playlist") => pages.playlist.select_all(),
                    _ => {}
                }
            })
            .build();
        let undo = gio::ActionEntry::builder("undo")
            .activate(|window: &Self, _, _| {
                let undo = window.pages().and_then(|p| p.undo.take());
                if let Some(undo) = undo {
                    undo();
                    if let Some(toasts) = window.imp().toasts.borrow().as_ref() {
                        toasts.dismiss();
                    }
                } else if let Some(key) = window.open_playlist_key() {
                    // Without a toast to undo, the open playlist's last edit.
                    window.step_playlist(key, false);
                } else {
                    // Otherwise the newest tag edit.
                    window.step_tags(false);
                }
            })
            .build();
        let save_queue = gio::ActionEntry::builder("save-queue")
            .activate(|window: &Self, _, _| {
                let Some(playback) = window.imp().playback.borrow().clone() else {
                    return;
                };
                let rows: Vec<Rc<TrackRow>> = playback
                    .view()
                    .queue
                    .items
                    .into_iter()
                    .map(Rc::new)
                    .collect();
                let Some(backend) = window.backend() else {
                    return;
                };
                actions::add_to_playlist(window, backend, Target::tracks(rows), None, None);
            })
            .build();
        self.add_action_entries([search, properties, copies, select_all, undo, save_queue]);

        // The main menu's file actions belong to the application; the window
        // that is open answers them.
        let Some(application) = self.application() else {
            return;
        };
        for name in [
            "open-files",
            "open-folder",
            "new-playlist",
            "import-playlist",
        ] {
            if application.lookup_action(name).is_some() {
                continue;
            }
            let action = gio::SimpleAction::new(name, None);
            let app = application.downgrade();
            action.connect_activate(move |action, _| {
                let Some(window) = app
                    .upgrade()
                    .and_then(|a| a.active_window())
                    .and_downcast::<MainWindow>()
                else {
                    return;
                };
                match action.name().as_str() {
                    "open-files" => window.open_files(false),
                    "open-folder" => window.open_files(true),
                    "new-playlist" => window.new_playlist(),
                    _ => window.import_playlist(),
                }
            });
            application.add_action(&action);
        }
    }

    /// Files from a file manager go into the queue anywhere in the window;
    /// dragged tracks go into the queue on the panel.
    fn setup_drops(&self, backend: &Backend) {
        let weak = self.downgrade();
        let files = drag::file_target(move |paths| {
            if let Some(window) = weak.upgrade() {
                window.play_files(paths, true);
            }
        });
        self.imp().panes_overlay.add_controller(files);
        let target = DraggedTracks::target(gdk::DragAction::COPY);
        let b = backend.clone();
        target.connect_drop(move |_, value, _, _| {
            let Ok(dragged) = value.get::<DraggedTracks>() else {
                return false;
            };
            let b = b.clone();
            glib::spawn_future_local(async move {
                let keys = resolve(&b, &dragged).await;
                if !keys.is_empty() {
                    b.send(PlayerRequest::Enqueue {
                        tracks: keys,
                        next: false,
                    });
                }
            });
            true
        });
        self.imp().panel.add_controller(target);
    }

    /// Dropped tracks go into the playlist `key`, at `position` or the end.
    pub(crate) fn drop_on_playlist(
        &self,
        key: PlaylistKey,
        dragged: DraggedTracks,
        position: Option<usize>,
    ) {
        let Some(backend) = self.backend() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let keys = resolve(&backend, &dragged).await;
            let rows = keys
                .into_iter()
                .map(|key| {
                    Rc::new(TrackRow {
                        key,
                        ..TrackRow::default()
                    })
                })
                .collect();
            if let Some(window) = weak.upgrade() {
                actions::add_to_playlist(
                    &window,
                    backend,
                    Target::tracks(rows),
                    Some(key),
                    position,
                );
            }
        });
    }

    /// The context menu of the queue's rows, and Delete to remove them.
    fn setup_queue_menu(&self) {
        let list = self.imp().panel.queue_list();
        list.set_selection_mode(gtk::SelectionMode::Single);
        let target_at = |list: &gtk::ListBox, row: &gtk::ListBoxRow| -> Option<Target> {
            let entry = row.widget_name().parse::<u64>().ok()?;
            let window = list.root().and_downcast::<MainWindow>()?;
            let playback = window.imp().playback.borrow().clone()?;
            let track = playback
                .view()
                .queue
                .items
                .into_iter()
                .find(|t| t.entry == Some(entry))?;
            Some(Target::Tracks {
                rows: vec![Rc::new(track)],
                origin: Origin::Queue {
                    entries: vec![entry],
                },
            })
        };
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(list) = gesture.widget().and_downcast::<gtk::ListBox>() else {
                return;
            };
            let Some(row) = list.row_at_y(y as i32) else {
                return;
            };
            if let Some(target) = target_at(&list, &row) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                list.select_row(Some(&row));
                actions::popup(&list, target, Some((x, y)));
            }
        });
        list.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |controller, key, _, state| {
            let Some(list) = controller.widget().and_downcast::<gtk::ListBox>() else {
                return glib::Propagation::Proceed;
            };
            let row = list
                .selected_row()
                .or_else(|| list.focus_child().and_downcast::<gtk::ListBoxRow>());
            let Some(row) = row else {
                return glib::Propagation::Proceed;
            };
            let Some(target) = target_at(&list, &row) else {
                return glib::Propagation::Proceed;
            };
            if actions::is_menu_key(key, state) {
                actions::popup(&row, target, None);
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::Delete
                && let Some(window) = list.root().and_downcast::<MainWindow>()
            {
                window.set_target(Some(target));
                WidgetExt::activate_action(&window, "win.sel-remove", None).ok();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        list.add_controller(keys);
    }

    /// The menu of a playlist row in the sidebar.
    /// Plays the playlist `key` from its start.
    pub(crate) fn play_playlist(&self, key: PlaylistKey) {
        let Some(backend) = self.backend() else {
            return;
        };
        let name = self
            .playlist_rows()
            .into_iter()
            .find(|p| p.key == key)
            .map(|p| p.name)
            .unwrap_or_default();
        let smart = self
            .playlist_row(key)
            .is_some_and(|p| p.kind == PlaylistType::Smart);
        glib::spawn_future_local(async move {
            let Ok(items) = backend
                .query(move |l| {
                    if smart {
                        Ok(l.smart_tracks(key)?.into_iter().map(|t| t.row).collect())
                    } else {
                        l.playlist_items(key)
                    }
                })
                .await
            else {
                return;
            };
            let tracks = items
                .iter()
                .filter(|t| !t.missing && t.key.0 != 0)
                .map(|t| t.key)
                .collect();
            backend.send(PlayerRequest::PlayPlaylist {
                key,
                tracks,
                start: 0,
                title: name,
                shuffle: false,
            });
        });
    }

    /// Opens the playlist named `name` (for tests and screenshots).
    #[doc(hidden)]
    pub fn open_playlist_named(&self, name: &str, editing: bool) -> bool {
        let Some(row) = self.playlist_rows().into_iter().find(|p| p.name == name) else {
            return false;
        };
        if row.kind == PlaylistType::Smart {
            self.open_smart_playlist(row.key, editing);
        } else {
            self.open_playlist(row.key);
        }
        true
    }

    /// The chips of the open smart playlist's rule line and its query text,
    /// and how many tracks it lists (for tests).
    #[doc(hidden)]
    pub fn smart_playlist_state(&self) -> (Vec<String>, String, usize) {
        self.pages().map_or_else(Default::default, |p| {
            let (chips, text) = p.smart.rule_texts();
            (chips, text, p.smart.track_count())
        })
    }

    /// Types query text into the open smart playlist's editor (for tests).
    #[doc(hidden)]
    pub fn smart_playlist_text(&self, text: &str) {
        if let Some(pages) = self.pages() {
            pages.smart.enter_text(text);
        }
    }

    /// Opens the open playlist's more menu (for tests and screenshots).
    #[doc(hidden)]
    pub fn demo_playlist_more_menu(&self) -> Option<gtk::Popover> {
        self.pages()?.playlist.demo_more_menu()
    }

    /// The names of the pinned rows (for tests).
    #[doc(hidden)]
    pub fn pin_names(&self) -> Vec<String> {
        self.pages()
            .map(|p| p.pins.borrow().iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default()
    }

    /// The search view (for tests).
    #[doc(hidden)]
    pub fn search_results(&self) -> Option<(usize, usize, usize)> {
        self.pages().map(|p| p.search.counts())
    }

    /// Types `text` into the search entry (for tests).
    #[doc(hidden)]
    pub fn search_text(&self, text: &str) {
        if let Some(pages) = self.pages() {
            pages.search_entry.set_text(text);
        }
    }

    /// The number of tracks the query results list and whether the notice
    /// about text that cannot be read is shown; `None` while words are
    /// searched (for tests).
    #[doc(hidden)]
    pub fn search_query_state(&self) -> Option<(usize, bool)> {
        self.pages().and_then(|p| p.search.query_state())
    }

    /// The names of the saved searches in the side column (for tests).
    #[doc(hidden)]
    pub fn saved_search_names(&self) -> Vec<String> {
        self.pages()
            .map(|p| p.search.saved_names())
            .unwrap_or_default()
    }

    /// The names the completion under the search entry lists (for tests).
    #[doc(hidden)]
    pub fn search_completions(&self) -> Vec<String> {
        self.pages()
            .map(|p| p.completion.names())
            .unwrap_or_default()
    }

    /// Types `text` into the focused search entry and returns the completion
    /// under it (for screenshots).
    #[doc(hidden)]
    pub fn demo_query_completion(&self, text: &str) -> Option<gtk::Popover> {
        let pages = self.pages()?;
        pages.search_entry.grab_focus();
        pages.search_entry.set_text(text);
        pages.search_entry.set_position(-1);
        pages.completion.update();
        Some(pages.completion.popover())
    }

    /// Whether the search entry is set in the query font (for tests).
    #[doc(hidden)]
    pub fn search_entry_is_query(&self) -> bool {
        self.pages()
            .is_some_and(|p| p.search_entry.has_css_class("hk-query"))
    }

    /// Opens a library page by its name (for tests).
    #[doc(hidden)]
    pub fn open_page(&self, name: &str) {
        self.open_library(name);
    }

    /// Opens the page of the artist `key` (for tests and screenshots).
    #[doc(hidden)]
    pub fn demo_open_artist(&self, key: ArtistKey) {
        self.open_artist(key);
    }

    /// How many tracks the track table holds and the batches it fetched (for tests).
    #[doc(hidden)]
    pub fn track_table_stats(&self) -> (u32, u64) {
        self.pages().map_or((0, 0), |p| p.tracks.stats())
    }

    /// The vertical adjustment of the track table (for tests).
    #[doc(hidden)]
    pub fn track_table_adjustment(&self) -> Option<gtk::Adjustment> {
        self.pages().and_then(|p| p.tracks.vadjustment())
    }

    /// Sorts the track table by the given columns, descending where `true`
    /// (for tests).
    #[doc(hidden)]
    pub fn sort_tracks(&self, keys: &[(crate::backend::TrackColumn, bool)]) {
        if let Some(pages) = self.pages() {
            pages.tracks.set_sort(
                keys.iter()
                    .map(|(column, descending)| crate::backend::TrackSort {
                        column: *column,
                        descending: *descending,
                    })
                    .collect(),
            );
        }
    }

    /// The first keys of the track table in their order (for tests).
    #[doc(hidden)]
    pub fn track_keys(&self, count: usize) -> Vec<TrackKey> {
        self.pages()
            .map(|p| p.tracks.keys().iter().take(count).copied().collect())
            .unwrap_or_default()
    }

    /// Opens the context menu of three entries of the open playlist (for
    /// screenshots).
    #[doc(hidden)]
    pub fn demo_track_menu(&self) -> Option<gtk::Popover> {
        self.pages()?.playlist.demo_menu(0, 3).map(|p| p.upcast())
    }

    /// The tiles in view of the Genres page's album grid, and how many show
    /// their cover image (for tests).
    #[doc(hidden)]
    pub fn genre_cover_stats(&self) -> (usize, usize) {
        self.pages().map_or((0, 0), |p| p.genres.cover_stats())
    }

    /// Opens the context menu of the first track of the album page (for
    /// tests and screenshots).
    #[doc(hidden)]
    pub fn demo_album_track_menu(&self) -> Option<gtk::Popover> {
        self.imp()
            .album
            .borrow()
            .as_ref()?
            .demo_menu()
            .map(|p| p.upcast())
    }

    /// Selects `count` rows of the track table from `start` (for screenshots).
    #[doc(hidden)]
    pub fn demo_select_tracks(&self, start: u32, count: u32) {
        if let Some(pages) = self.pages() {
            pages.tracks.select_rows(start, count);
        }
    }

    /// Starts editing `column` of the selected rows of the track table, as F2
    /// does; the entry, if the edit began (for tests).
    #[doc(hidden)]
    pub fn demo_edit_track_cell(&self, column: crate::backend::TrackColumn) -> Option<gtk::Entry> {
        self.pages().and_then(|p| p.tracks.demo_edit(column))
    }

    /// The texts the other selected rows show while a cell of `column` is
    /// edited, and the status bar's texts (for tests).
    #[doc(hidden)]
    pub fn track_edit_preview(
        &self,
        column: crate::backend::TrackColumn,
    ) -> (Vec<String>, (String, String)) {
        self.pages()
            .map(|p| (p.tracks.previewed(column), p.tracks.status_texts()))
            .unwrap_or_default()
    }

    /// Opens a folder of the folder tree, a level per call (for screenshots).
    #[doc(hidden)]
    pub fn demo_folder_step(&self) {
        if let Some(pages) = self.pages() {
            pages.folders.demo_step();
        }
    }

    /// Opens the main menu (for screenshots).
    #[doc(hidden)]
    pub fn demo_main_menu(&self) -> Option<gtk::Popover> {
        let button = &self.imp().menu_button;
        button.popup();
        button.popover()
    }

    /// Opens the play button's menu (for screenshots).
    #[doc(hidden)]
    pub fn demo_play_menu(&self) -> gtk::Popover {
        self.imp().deck.popup_play_menu().upcast()
    }

    /// Opens the first playlist (for tests and screenshots).
    #[doc(hidden)]
    pub fn open_first_playlist(&self) -> bool {
        // The sample's playlist with album groups and a missing entry, or
        // the first static playlist.
        let rows: Vec<PlaylistRow> = self
            .playlist_rows()
            .into_iter()
            .filter(|p| p.kind == PlaylistType::Static)
            .collect();
        let first = rows
            .iter()
            .find(|p| p.grouped || p.dead > 0)
            .or(rows.first())
            .cloned();
        match first {
            Some(row) => {
                self.open_playlist(row.key);
                true
            }
            None => false,
        }
    }

    /// Rates the track table's row at `position` with its stars (for tests).
    #[doc(hidden)]
    pub fn demo_rate_track_row(&self, position: u32, stars: Option<u8>) -> bool {
        self.pages()
            .is_some_and(|p| p.tracks.demo_rate(position, stars))
    }

    /// The rating the track table's row at `position` shows, if it is shown
    /// (for tests).
    #[doc(hidden)]
    pub fn track_row_rating(&self, position: u32) -> Option<Option<u8>> {
        self.pages().and_then(|p| p.tracks.shown_rating(position))
    }

    /// The ratings the album page's track rows show (for tests).
    #[doc(hidden)]
    pub fn album_track_ratings(&self) -> Vec<Option<u8>> {
        self.imp()
            .album
            .borrow()
            .as_ref()
            .map(|a| a.shown_ratings())
            .unwrap_or_default()
    }

    /// The playlists in the sidebar (for tests).
    #[doc(hidden)]
    pub fn playlist_names(&self) -> Vec<String> {
        self.playlist_rows().into_iter().map(|p| p.name).collect()
    }

    /// The rows of the open playlist (for tests).
    #[doc(hidden)]
    pub fn playlist_titles(&self) -> Vec<String> {
        self.pages()
            .map(|p| p.playlist.titles())
            .unwrap_or_default()
    }

    /// The number of artists, genres and folder roots listed (for tests).
    #[doc(hidden)]
    pub fn list_counts(&self) -> (usize, usize, usize) {
        self.pages().map_or((0, 0, 0), |p| {
            (p.artists.count(), p.genres.count(), p.folders.count())
        })
    }

    /// The rows of the Composers and the Years lists (for tests).
    #[doc(hidden)]
    pub fn browse_counts(&self) -> (usize, usize) {
        self.pages()
            .map_or((0, 0), |p| (p.composers.count(), p.years.count()))
    }

    /// Chooses row `index` of the Composers (`years` false) or Years list and
    /// returns the name its detail shows (for tests).
    #[doc(hidden)]
    pub fn browse_choose(&self, years: bool, index: usize) -> String {
        let Some(pages) = self.pages() else {
            return String::new();
        };
        let view = if years {
            &pages.years
        } else {
            &pages.composers
        };
        view.choose(index);
        view.shown_name()
    }
}

/// The tracks a drag carries, looking up an album's.
async fn resolve(backend: &Backend, dragged: &DraggedTracks) -> Vec<TrackKey> {
    match dragged.album {
        Some(album) => backend
            .query(move |l| l.album(album))
            .await
            .ok()
            .flatten()
            .map(|d| d.tracks().map(|t| t.key).collect())
            .unwrap_or_default(),
        None => dragged.keys.clone(),
    }
}

/// The files of `paths`, with each folder replaced by the audio files in it,
/// by name, one level deep and then its folders.
fn expand_folders(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&path)
                .map(|e| e.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            entries.sort();
            let (folders, plain): (Vec<PathBuf>, Vec<PathBuf>) =
                entries.into_iter().partition(|p| p.is_dir());
            files.extend(plain.into_iter().filter(|p| {
                p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    [
                        "flac", "mp3", "ogg", "oga", "opus", "m4a", "m4b", "wav", "aif", "aiff",
                        "ape", "wv", "mpc", "dsf", "dff", "aac",
                    ]
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(e))
                })
            }));
            files.extend(expand_folders(
                folders
                    .into_iter()
                    .filter(|p| {
                        !p.file_name()
                            .is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
                    })
                    .collect(),
            ));
        } else {
            files.push(path);
        }
    }
    files
}

/// Puts the hint "Ctrl F" at the right end of the empty search entry: the
/// entry goes into an overlay that takes its place in the header.
fn add_shortcut_hint(entry: &gtk::SearchEntry) {
    let Some(header) = entry.parent().and_downcast::<gtk::CenterBox>() else {
        return;
    };
    header.set_center_widget(None::<&gtk::Widget>);
    let overlay = gtk::Overlay::new();
    overlay.set_valign(gtk::Align::Center);
    overlay.set_child(Some(entry));
    let hint = crate::widgets::label("Ctrl F", &["hk-key", "hint"]);
    hint.set_halign(gtk::Align::End);
    hint.set_valign(gtk::Align::Center);
    hint.set_margin_end(entry.margin_end() + 10);
    hint.set_can_target(false);
    overlay.add_overlay(&hint);
    entry
        .bind_property("text", &hint, "visible")
        .transform_to(|_, text: String| Some(text.is_empty()))
        .sync_create()
        .build();
    header.set_center_widget(Some(&overlay));
}
