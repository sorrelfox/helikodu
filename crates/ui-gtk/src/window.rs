// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The main window: the sidebar, the content area with its header and view
//! history, the queue panel, and the deck under all three.
//!
//! In the integrated style the window has no title bar: the three top strips
//! drag it and hold the desktop's window buttons. In the classic style the
//! desktop draws its title bar, and the menu bar can be shown (Ctrl+M).

pub(crate) mod dialog_hooks;
pub(crate) mod pages;
pub(crate) mod pane_handle;
pub(crate) mod panel;
pub(crate) mod sidebar;
pub(crate) mod toast_overlay;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{CompositeTemplate, gio, glib};

use crate::backend::{AlbumKey, ArtistKey, Backend, GenreKey, PlaylistKey, Update};
use crate::deck::Deck;
use crate::i18n::tr;
use crate::models::CoverCache;
use crate::playback::{Change, Playback};
use crate::views::placeholder::placeholder;
use crate::views::{AlbumPage, AlbumsView};
use pane_handle::{PaneHandle, Side};
use panel::Panel;
use sidebar::{Sidebar, page_icon, page_title};
use toast_overlay::ToastOverlay;

/// How the window is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowStyle {
    /// No title bar; the top strips drag the window and hold its buttons.
    #[default]
    Integrated,
    /// The desktop's title bar, and an optional menu bar.
    Classic,
}

impl WindowStyle {
    /// Integrated on GNOME, classic elsewhere.
    pub fn for_desktop() -> Self {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        if desktop.split(':').any(|d| d.eq_ignore_ascii_case("GNOME")) {
            Self::Integrated
        } else {
            Self::Classic
        }
    }
}

/// The menu bar's model from the main menu: the menu's direct items go under
/// File, its submenus follow as they are.
fn menubar_model(menu: &gio::MenuModel) -> gio::Menu {
    let bar = gio::Menu::new();
    let file = gio::Menu::new();
    let mut submenus = Vec::new();
    for section in 0..menu.n_items() {
        let Some(items) = menu.item_link(section, gio::MENU_LINK_SECTION) else {
            continue;
        };
        let direct = gio::Menu::new();
        for i in 0..items.n_items() {
            match items.item_link(i, gio::MENU_LINK_SUBMENU) {
                Some(submenu) => {
                    let label = items
                        .item_attribute_value(i, gio::MENU_ATTRIBUTE_LABEL, None)
                        .and_then(|v| v.get::<String>());
                    submenus.push((label, submenu));
                }
                None => direct.append_item(&gio::MenuItem::from_model(&items, i)),
            }
        }
        if direct.n_items() > 0 {
            file.append_section(None, &direct);
        }
    }
    bar.append_submenu(Some(&tr("File")), &file);
    for (label, submenu) in submenus {
        bar.append_submenu(label.as_deref(), &submenu);
    }
    bar
}

/// A page of the content area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Library(&'static str),
    Album(AlbumKey),
    Artist(ArtistKey),
    Genre(GenreKey),
    Playlist(PlaylistKey),
    Search,
}

const SIDEBAR_WIDTH: (i32, (i32, i32)) = (228, (180, 360));
const PANEL_WIDTH: (i32, (i32, i32)) = (320, (260, 520));

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/github/sorrelfox/Helikodu/ui/main_window.ui")]
    #[allow(unreachable_pub)]
    pub struct MainWindow {
        #[template_child]
        pub(super) menubar: TemplateChild<gtk::PopoverMenuBar>,
        #[template_child]
        pub(super) panes_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(super) sidebar_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub(super) back: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) forward: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) menu_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub(super) show_panel: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) header_divider: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) header_controls: TemplateChild<gtk::WindowControls>,
        #[template_child]
        pub(super) toast_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(super) views: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) panel_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub(super) panel: TemplateChild<Panel>,
        #[template_child]
        pub(super) deck: TemplateChild<Deck>,
        pub(super) style: Cell<WindowStyle>,
        pub(super) sidebar: RefCell<Option<Rc<Sidebar>>>,
        pub(super) toasts: RefCell<Option<Rc<ToastOverlay>>>,
        pub(super) handles: RefCell<Vec<Rc<PaneHandle>>>,
        pub(super) albums: RefCell<Option<AlbumsView>>,
        pub(super) album: RefCell<Option<AlbumPage>>,
        pub(super) backend: RefCell<Option<Backend>>,
        pub(super) playback: RefCell<Option<Rc<Playback>>>,
        pub(super) history: RefCell<Vec<Page>>,
        pub(super) index: Cell<usize>,
        pub(super) placeholders: RefCell<HashSet<&'static str>>,
        pub(super) notifications: Cell<bool>,
        pub(super) pages: RefCell<Option<Rc<pages::Pages>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MainWindow {
        const NAME: &'static str = "HkMainWindow";
        type Type = super::MainWindow;
        type ParentType = gtk::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            Deck::ensure_type();
            Panel::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MainWindow {}
    impl WidgetImpl for MainWindow {}
    impl WindowImpl for MainWindow {}
    impl ApplicationWindowImpl for MainWindow {}
}

glib::wrapper! {
    /// The main window.
    pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
        @extends gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl MainWindow {
    /// A main window of `application` over `backend`.
    pub fn new(application: &gtk::Application, backend: &Backend, style: WindowStyle) -> Self {
        let window: Self = glib::Object::builder()
            .property("application", application)
            .build();
        window.setup(backend, style);
        window
    }

    fn setup(&self, backend: &Backend, style: WindowStyle) {
        let imp = self.imp();
        crate::style::register_window(self);
        imp.backend.replace(Some(backend.clone()));
        let playback = Playback::follow(backend);
        imp.playback.replace(Some(playback.clone()));

        // Sidebar and the resizable panes.
        let sidebar = Sidebar::new();
        imp.sidebar_revealer.set_child(Some(&sidebar.root));
        let weak = self.downgrade();
        sidebar.connect_navigate(move |page| {
            if let Some(window) = weak.upgrade() {
                window.open_library(page);
            }
        });
        imp.sidebar.replace(Some(sidebar.clone()));
        let left = PaneHandle::new(
            &imp.panes_overlay,
            &sidebar.root,
            Side::Start,
            SIDEBAR_WIDTH.0,
            SIDEBAR_WIDTH.1,
        );
        let right = PaneHandle::new(
            &imp.panes_overlay,
            &imp.panel.get(),
            Side::End,
            PANEL_WIDTH.0,
            PANEL_WIDTH.1,
        );
        // The pane widths and the window size as the last run left them.
        let state = backend.ui_state().load();
        left.set_width(i32::try_from(state.panes.sidebar_width).unwrap_or(SIDEBAR_WIDTH.0));
        right.set_width(i32::try_from(state.panes.queue_width).unwrap_or(PANEL_WIDTH.0));
        imp.sidebar_revealer
            .set_reveal_child(state.panes.sidebar_visible);
        imp.panel_revealer
            .set_reveal_child(state.panes.queue_visible);
        self.set_default_size(
            i32::try_from(state.window.width).unwrap_or(1280),
            i32::try_from(state.window.height).unwrap_or(800),
        );
        if state.window.maximized {
            self.maximize();
        }
        for handle in [&left, &right] {
            let weak = self.downgrade();
            handle.connect_changed(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.save_ui_state();
                }
            });
        }
        imp.handles.replace(vec![left, right]);
        self.connect_close_request(|window| {
            window.save_ui_state();
            glib::Propagation::Proceed
        });

        // Views.
        let covers = CoverCache::new(backend);
        let albums = AlbumsView::default();
        let weak = self.downgrade();
        albums.bind(backend, &playback, &covers, move |key| {
            if let Some(window) = weak.upgrade() {
                window.go(Page::Album(key), true);
            }
        });
        imp.views.add_named(&albums, Some("albums"));
        let album = AlbumPage::default();
        album.bind(backend, &playback, &covers);
        imp.views.add_named(&album, Some("album"));
        imp.albums.replace(Some(albums));
        imp.album.replace(Some(album));
        self.setup_pages(backend, &playback, &covers);

        imp.deck.bind(backend, &playback, &covers);
        self.setup_dialogs(backend, &playback);
        imp.panel.bind(backend, &playback, &covers);
        let toasts = ToastOverlay::new(&imp.toast_overlay);
        imp.toasts.replace(Some(toasts));
        let weak = self.downgrade();
        let cache = Rc::downgrade(&covers);
        playback.connect_event(move |update| {
            let Some(window) = weak.upgrade() else { return };
            match update {
                Update::Problem { message } => window.toast(message, true),
                Update::Notice { message } => window.toast(message, false),
                Update::Thumbnail { cover, size, path } => {
                    if let Some(cache) = cache.upgrade() {
                        cache.thumbnail_ready(*cover, *size, path.clone());
                    }
                }
                Update::LibraryChanged => {
                    if let Some(albums) = window.imp().albums.borrow().as_ref() {
                        albums.reload();
                    }
                    window.load_counts();
                    window.library_changed();
                }
                Update::Raise => window.present(),
                Update::Quit => {
                    if let Some(application) = window.application() {
                        application.quit();
                    }
                }
                _ => {}
            }
        });

        self.install_actions();
        if let Some(menu) = imp.menu_button.menu_model() {
            imp.menubar.set_menu_model(Some(&menubar_model(&menu)));
        }
        self.set_style(style);
        self.go(Page::Library("albums"), true);
        self.load_counts();
        let weak = self.downgrade();
        playback.connect(move |view, change| {
            let Some(window) = weak.upgrade() else { return };
            match change {
                Change::Modes => {
                    if let Some(action) = window
                        .application()
                        .and_then(|a| a.lookup_action("stop-after"))
                        .and_downcast::<gio::SimpleAction>()
                    {
                        action.set_state(&view.stop_after.to_variant());
                    }
                }
                Change::Track => window.notify_track(view),
                _ => {}
            }
        });
        let weak = self.downgrade();
        crate::style::connect_changed(move |look| {
            if let Some(window) = weak.upgrade() {
                window.set_reduced_motion(look.reduced_motion);
            }
        });
        self.set_reduced_motion(crate::style::current().reduced_motion);
    }

    fn set_reduced_motion(&self, reduced: bool) {
        let imp = self.imp();
        let duration = if reduced { 0 } else { 150 };
        imp.sidebar_revealer.set_transition_duration(duration);
        imp.panel_revealer.set_transition_duration(duration);
    }

    fn set_style(&self, style: WindowStyle) {
        let imp = self.imp();
        imp.style.set(style);
        let integrated = style == WindowStyle::Integrated;
        if integrated {
            let empty = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            empty.set_visible(false);
            self.set_titlebar(Some(&empty));
        }
        if let Some(sidebar) = imp.sidebar.borrow().as_ref() {
            sidebar.controls.set_visible(integrated);
        }
        imp.panel.set_integrated(integrated);
        self.update_header();
    }

    /// Saves the pane widths, which panes are shown, and the window size.
    fn save_ui_state(&self) {
        let imp = self.imp();
        let Some(backend) = imp.backend.borrow().clone() else {
            return;
        };
        let store = backend.ui_state();
        let mut state = store.load();
        let handles = imp.handles.borrow();
        if let (Some(left), Some(right)) = (handles.first(), handles.get(1)) {
            state.panes.sidebar_width = u32::try_from(left.width()).unwrap_or(228);
            state.panes.queue_width = u32::try_from(right.width()).unwrap_or(320);
        }
        state.panes.sidebar_visible = imp.sidebar_revealer.reveals_child();
        state.panes.queue_visible = imp.panel_revealer.reveals_child();
        state.window.maximized = self.is_maximized();
        if !self.is_maximized() {
            let (w, h) = self.default_size();
            state.window.width = u32::try_from(w).unwrap_or(1280);
            state.window.height = u32::try_from(h).unwrap_or(800);
        }
        if let Err(error) = store.save(&state) {
            eprintln!("helikodu: cannot save the window state: {error}");
        }
    }

    /// Shows the window controls and the panel button in the header when the
    /// panel is hidden.
    fn update_header(&self) {
        let imp = self.imp();
        let panel_shown = imp.panel_revealer.reveals_child();
        let integrated = imp.style.get() == WindowStyle::Integrated;
        imp.show_panel.set_visible(!panel_shown);
        imp.header_divider
            .set_visible(!panel_shown && integrated && !imp.header_controls.is_empty());
        imp.header_controls.set_visible(!panel_shown && integrated);
        imp.menu_button.set_visible(!imp.menubar.is_visible());
        if let Some(handles) = imp.handles.borrow().get(1) {
            handles.set_shown(panel_shown);
        }
        if let Some(handles) = imp.handles.borrow().first() {
            handles.set_shown(imp.sidebar_revealer.reveals_child());
        }
    }

    fn install_actions(&self) {
        let toggle_sidebar = gio::ActionEntry::builder("toggle-sidebar")
            .activate(|window: &Self, _, _| {
                let revealer = &window.imp().sidebar_revealer;
                revealer.set_reveal_child(!revealer.reveals_child());
                window.update_header();
                window.save_ui_state();
            })
            .build();
        let toggle_panel = gio::ActionEntry::builder("toggle-panel")
            .activate(|window: &Self, _, _| {
                let revealer = &window.imp().panel_revealer;
                revealer.set_reveal_child(!revealer.reveals_child());
                window.update_header();
                window.save_ui_state();
            })
            .build();
        let show_menubar = gio::ActionEntry::builder("show-menubar")
            .state(false.to_variant())
            .activate(|window: &Self, action, _| {
                if window.imp().style.get() != WindowStyle::Classic {
                    return;
                }
                let shown = !action
                    .state()
                    .and_then(|s| s.get::<bool>())
                    .unwrap_or(false);
                action.set_state(&shown.to_variant());
                window.imp().menubar.set_visible(shown);
                window.update_header();
            })
            .build();
        let back = gio::ActionEntry::builder("back")
            .activate(|window: &Self, _, _| window.step_history(-1))
            .build();
        let forward = gio::ActionEntry::builder("forward")
            .activate(|window: &Self, _, _| window.step_history(1))
            .build();
        let cycle = gio::ActionEntry::builder("cycle-pane")
            .activate(|window: &Self, _, _| window.cycle_pane())
            .build();
        let jump = gio::ActionEntry::builder("jump-to-playing")
            .activate(|window: &Self, _, _| {
                let album = window
                    .imp()
                    .playback
                    .borrow()
                    .as_ref()
                    .and_then(|p| p.view().album());
                if let Some(album) = album {
                    window.go(Page::Album(album), true);
                }
            })
            .build();
        let volume_up = gio::ActionEntry::builder("volume-up")
            .activate(|window: &Self, _, _| window.imp().deck.adjust_volume(0.05))
            .build();
        let volume_down = gio::ActionEntry::builder("volume-down")
            .activate(|window: &Self, _, _| window.imp().deck.adjust_volume(-0.05))
            .build();
        self.add_action_entries([
            volume_up,
            volume_down,
            toggle_sidebar,
            toggle_panel,
            show_menubar,
            back,
            forward,
            cycle,
            jump,
        ]);
        self.update_history_actions();
    }

    fn open_library(&self, page: &str) {
        let page = match page {
            "home" => "home",
            "albums" => "albums",
            "artists" => "artists",
            "tracks" => "tracks",
            "genres" => "genres",
            "folders" => "folders",
            other => {
                self.open_sidebar_row(other);
                return;
            }
        };
        self.go(Page::Library(page), true);
    }

    pub(crate) fn go(&self, page: Page, record: bool) {
        let imp = self.imp();
        if record {
            let mut history = imp.history.borrow_mut();
            let index = imp.index.get();
            if history.get(index) != Some(&page) {
                if !history.is_empty() {
                    history.truncate(index + 1);
                }
                history.push(page);
                imp.index.set(history.len() - 1);
            }
        }
        let selected: String = match page {
            Page::Library(name) if self.has_page(name) => self.show_page(page),
            Page::Artist(_) | Page::Genre(_) | Page::Playlist(_) | Page::Search => {
                self.show_page(page)
            }
            Page::Library(name) => {
                if name != "albums" && imp.placeholders.borrow_mut().insert(name) {
                    imp.views
                        .add_named(&placeholder(page_icon(name), &page_title(name)), Some(name));
                }
                imp.views.set_visible_child_name(name);
                name.into()
            }
            Page::Album(key) => {
                if let Some(album) = imp.album.borrow().as_ref()
                    && album.album() != Some(key)
                {
                    album.show_album(key);
                }
                imp.views.set_visible_child_name("album");
                "albums".into()
            }
        };
        if let Some(sidebar) = imp.sidebar.borrow().as_ref() {
            sidebar.select((!selected.is_empty()).then_some(selected.as_ref()));
        }
        self.update_history_actions();
    }

    fn step_history(&self, by: isize) {
        let imp = self.imp();
        let target = imp.index.get() as isize + by;
        let page = usize::try_from(target)
            .ok()
            .and_then(|i| imp.history.borrow().get(i).copied().map(|p| (i, p)));
        if let Some((index, page)) = page {
            imp.index.set(index);
            self.go(page, false);
        }
    }

    fn update_history_actions(&self) {
        let imp = self.imp();
        let index = imp.index.get();
        let len = imp.history.borrow().len();
        for (name, enabled) in [("back", index > 0), ("forward", index + 1 < len)] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
    }

    fn load_counts(&self) {
        let Some(backend) = self.imp().backend.borrow().clone() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let counts = backend.query(|library| library.counts()).await;
            let Some(window) = weak.upgrade() else { return };
            match counts {
                Ok(counts) => {
                    if let Some(sidebar) = window.imp().sidebar.borrow().as_ref() {
                        sidebar.set_counts(&counts);
                    }
                }
                Err(error) => window.toast(
                    &crate::i18n::fill(&tr("The library cannot be read: {}"), &[&error.0]),
                    true,
                ),
            }
        });
    }

    /// Announces each new track with a desktop notification while the window is
    /// not in front.
    pub fn set_notifications(&self, on: bool) {
        self.imp().notifications.set(on);
    }

    fn notify_track(&self, view: &crate::playback::PlayerView) {
        let (Some(now), Some(application)) = (&view.now, self.application()) else {
            return;
        };
        if !self.imp().notifications.get() || self.is_active() {
            return;
        }
        let notification = gio::Notification::new(&now.track.title);
        let body = if now.album_title.is_empty() {
            now.track.artist.clone()
        } else {
            crate::i18n::fill(&tr("{} · {}"), &[&now.track.artist, &now.album_title])
        };
        notification.set_body(Some(&body));
        notification.set_default_action("app.activate");
        notification.add_button(&tr("Next"), "app.next");
        application.send_notification(Some("track"), &notification);
    }

    /// Shows a toast.
    pub fn toast(&self, text: &str, error: bool) {
        if let Some(toasts) = self.imp().toasts.borrow().as_ref() {
            toasts.show(text, None, error);
        }
    }

    /// The four regions F6 moves between: sidebar, content, panel, deck.
    fn regions(&self) -> Vec<(gtk::Widget, gtk::Widget)> {
        let imp = self.imp();
        let mut regions = Vec::new();
        if imp.sidebar_revealer.reveals_child()
            && let Some(sidebar) = imp.sidebar.borrow().as_ref()
        {
            regions.push((sidebar.root.clone().upcast(), sidebar.focus_target()));
        }
        let content_target = match imp.views.visible_child_name().as_deref() {
            Some("albums") => imp.albums.borrow().as_ref().map(AlbumsView::focus_target),
            Some("album") => imp.album.borrow().as_ref().map(AlbumPage::focus_target),
            Some(name) => self.page_focus_target(name),
            _ => None,
        }
        .unwrap_or_else(|| imp.back.get().upcast());
        regions.push((imp.views.get().upcast(), content_target));
        if imp.panel_revealer.reveals_child() {
            regions.push((imp.panel.get().upcast(), imp.panel.focus_target()));
        }
        regions.push((imp.deck.get().upcast(), imp.deck.focus_target()));
        regions
    }

    fn cycle_pane(&self) {
        let regions = self.regions();
        let focus = gtk::prelude::RootExt::focus(self);
        let current = focus.and_then(|f| {
            regions
                .iter()
                .position(|(root, _)| f == *root || f.is_ancestor(root))
        });
        let next = current.map_or(0, |i| (i + 1) % regions.len());
        if let Some((_, target)) = regions.get(next) {
            target.grab_focus();
        }
    }

    /// How many albums the grid holds and how many row batches it fetched.
    #[doc(hidden)]
    pub fn album_grid_stats(&self) -> (u32, u64) {
        self.imp()
            .albums
            .borrow()
            .as_ref()
            .map_or((0, 0), AlbumsView::stats)
    }

    /// The name of the visible view.
    #[doc(hidden)]
    pub fn visible_view(&self) -> Option<String> {
        self.imp().views.visible_child_name().map(|s| s.to_string())
    }

    /// Opens the page of `key`.
    pub fn open_album(&self, key: AlbumKey) {
        self.go(Page::Album(key), true);
    }

    /// Goes back in the view history.
    #[doc(hidden)]
    pub fn go_back(&self) {
        self.step_history(-1);
    }

    /// The text of the toast shown.
    #[doc(hidden)]
    pub fn toast_text(&self) -> Option<String> {
        self.imp().toasts.borrow().as_ref().and_then(|t| t.text())
    }

    /// Whether the sidebar and the panel are shown.
    #[doc(hidden)]
    pub fn panes_shown(&self) -> (bool, bool) {
        let imp = self.imp();
        (
            imp.sidebar_revealer.reveals_child(),
            imp.panel_revealer.reveals_child(),
        )
    }
}
