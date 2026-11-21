// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sidebar: the brand strip, then the library views, what is pinned, and
//! the playlists with their folders.
//!
//! A folder row opens or closes the folder; the playlists in it are indented
//! under it. A playlist dragged onto a folder moves into it.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use gtk::prelude::*;

use crate::backend::{LibraryCounts, PinKey, PinRow, PlaylistKey, PlaylistRow, PlaylistType};
use crate::i18n::tr;
use crate::models::CoverCache;
use crate::views::drag::{DraggedPlaylist, DraggedTracks};
use crate::widgets::{BrandMark, CoverArt, icon_button, label};

/// One library row: its page name, icon, and whether it shows a count.
const LIBRARY: &[(&str, &str)] = &[
    ("home", "go-home-symbolic"),
    ("albums", "media-optical-symbolic"),
    ("artists", "avatar-default-symbolic"),
    ("tracks", "audio-x-generic-symbolic"),
    ("genres", "helikodu-tag-symbolic"),
    ("composers", "helikodu-composer-symbolic"),
    ("years", "helikodu-calendar-symbolic"),
    ("labels", "helikodu-label-symbolic"),
    ("folders", "folder-symbolic"),
];

/// Whether `playlist` is hidden because a folder it is in is closed.
fn hidden_by(all: &[PlaylistRow], playlist: &PlaylistRow, closed: &HashSet<PlaylistKey>) -> bool {
    let mut parent = playlist.parent;
    while let Some(folder) = parent {
        if closed.contains(&folder) {
            return true;
        }
        parent = all.iter().find(|p| p.key == folder).and_then(|p| p.parent);
    }
    false
}

/// The visible name of a library page.
pub(crate) fn page_title(page: &str) -> String {
    match page {
        "home" => tr("Home"),
        "albums" => tr("Albums"),
        "artists" => tr("Artists"),
        "tracks" => tr("Tracks"),
        "genres" => tr("Genres"),
        "composers" => tr("Composers"),
        "years" => tr("Years"),
        "labels" => tr("Labels"),
        "folders" => tr("Folders"),
        _ => String::new(),
    }
}

/// The icon of a library page.
pub(crate) fn page_icon(page: &str) -> &'static str {
    LIBRARY
        .iter()
        .find(|(name, _)| *name == page)
        .map_or("folder-symbolic", |(_, icon)| icon)
}

type Navigate = Box<dyn Fn(&str)>;
type PlaylistMenu = Rc<dyn Fn(&gtk::Widget, PlaylistKey)>;
type PlaylistDrop = Rc<dyn Fn(PlaylistKey, DraggedTracks)>;
type FolderDrop = Rc<dyn Fn(PlaylistKey, PlaylistKey)>;
type PinMenu = Rc<dyn Fn(&gtk::Widget, PinKey)>;

/// The icon of a playlist row.
pub(crate) fn playlist_icon(kind: PlaylistType, from_file: bool) -> &'static str {
    match kind {
        PlaylistType::Folder => "folder-symbolic",
        PlaylistType::Smart => "helikodu-funnel-symbolic",
        PlaylistType::Generator => "helikodu-steps-symbolic",
        PlaylistType::Static if from_file => "text-x-generic-symbolic",
        PlaylistType::Static => "helikodu-playlist-symbolic",
    }
}

/// Whether a row name is one of the playlists section.
fn in_playlists(name: &str) -> bool {
    name == "new-playlist" || name.starts_with("playlist:") || name.starts_with("folder:")
}

/// The sidebar and its list.
pub(crate) struct Sidebar {
    pub(crate) root: gtk::Box,
    pub(crate) controls: gtk::WindowControls,
    list: gtk::ListBox,
    counts: RefCell<Vec<(String, gtk::Label)>>,
    quiet: Cell<bool>,
    navigate: RefCell<Option<Navigate>>,
    playlist_rows: RefCell<Vec<gtk::ListBoxRow>>,
    playlist_menu: RefCell<Option<PlaylistMenu>>,
    playlist_drop: RefCell<Option<PlaylistDrop>>,
    folder_drop: RefCell<Option<FolderDrop>>,
    pin_rows: RefCell<Vec<gtk::ListBoxRow>>,
    pin_menu: RefCell<Option<PinMenu>>,
    /// The playlists last listed, for opening and closing folders.
    playlists: RefCell<Vec<PlaylistRow>>,
    /// The folders that are closed.
    closed: RefCell<HashSet<PlaylistKey>>,
}

impl std::fmt::Debug for Sidebar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sidebar").finish_non_exhaustive()
    }
}

impl Sidebar {
    pub(crate) fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-sidebar");
        root.set_accessible_role(gtk::AccessibleRole::Navigation);
        root.update_property(&[gtk::accessible::Property::Label(&tr("Library"))]);

        let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        brand.add_css_class("hk-header");
        let controls = gtk::WindowControls::new(gtk::PackType::Start);
        controls.set_valign(gtk::Align::Center);
        brand.append(&controls);
        brand.append(&BrandMark::default());
        let name = label("Helikodu", &["hk-brand"]);
        name.set_hexpand(true);
        brand.append(&name);
        let hide = icon_button("helikodu-sidebar-symbolic", &tr("Hide the sidebar"));
        hide.set_action_name(Some("win.toggle-sidebar"));
        brand.append(&hide);
        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&brand));
        root.append(&handle);

        let list = gtk::ListBox::new();
        list.add_css_class("hk-list");
        list.set_margin_start(8);
        list.set_margin_end(8);
        list.set_margin_bottom(8);
        list.update_property(&[gtk::accessible::Property::Label(&tr("Library"))]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        root.append(&scroller);

        let sidebar = Rc::new(Self {
            root,
            controls,
            list,
            counts: RefCell::new(Vec::new()),
            quiet: Cell::new(false),
            navigate: RefCell::new(None),
            playlist_rows: RefCell::new(Vec::new()),
            playlist_menu: RefCell::new(None),
            playlist_drop: RefCell::new(None),
            folder_drop: RefCell::new(None),
            pin_rows: RefCell::new(Vec::new()),
            pin_menu: RefCell::new(None),
            playlists: RefCell::new(Vec::new()),
            closed: RefCell::new(HashSet::new()),
        });
        for (page, icon) in LIBRARY {
            sidebar.add_row(page, icon, &page_title(page), &[]);
        }
        // Until the counts say a label exists.
        if let Some(row) = sidebar.row("labels") {
            row.set_visible(false);
        }
        sidebar.add_row(
            "new-playlist",
            "list-add-symbolic",
            &tr("New playlist"),
            &["hk-dim"],
        );
        sidebar.list.set_header_func(|row, before| {
            let name = row.widget_name();
            let pin = |n: &str| n.starts_with("pin:");
            let text = match (name.as_str(), before) {
                (_, None) => Some((tr("Library"), true)),
                (n, Some(b)) if in_playlists(n) && !in_playlists(b.widget_name().as_str()) => {
                    Some((tr("Playlists"), false))
                }
                (n, Some(b)) if pin(n) && !pin(b.widget_name().as_str()) => {
                    Some((tr("Pinned"), false))
                }
                _ => None,
            };
            row.set_header(
                text.map(|(text, first)| {
                    let header = label(&text, &["hk-section-label", "hk-list-section"]);
                    if first {
                        header.add_css_class("first");
                    }
                    header
                })
                .as_ref(),
            );
        });
        let weak = Rc::downgrade(&sidebar);
        sidebar.list.connect_row_activated(move |_, row| {
            let Some(sidebar) = weak.upgrade() else {
                return;
            };
            if let Some(key) = row
                .widget_name()
                .strip_prefix("folder:")
                .and_then(|k| k.parse().ok())
            {
                sidebar.toggle_folder(PlaylistKey(key));
            }
        });
        let weak = Rc::downgrade(&sidebar);
        sidebar.list.connect_row_selected(move |_, row| {
            let (Some(sidebar), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            if sidebar.quiet.get() {
                return;
            }
            if let Some(navigate) = sidebar.navigate.borrow().as_ref() {
                navigate(row.widget_name().as_str());
            }
        });
        sidebar
    }

    fn add_row(&self, page: &str, icon: &str, title: &str, classes: &[&str]) {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(page);
        for class in classes {
            row.add_css_class(class);
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.append(&gtk::Image::from_icon_name(icon));
        let name = label(title, &[]);
        name.set_hexpand(true);
        line.append(&name);
        let count = label("", &["hk-count"]);
        count.set_xalign(1.0);
        line.append(&count);
        self.counts.borrow_mut().push((page.to_owned(), count));
        row.set_child(Some(&line));
        row.update_property(&[gtk::accessible::Property::Label(title)]);
        self.list.append(&row);
    }

    /// Lists `playlists` (in their tree order) between the pinned rows and
    /// "New playlist".
    pub(crate) fn set_playlists(self: &Rc<Self>, playlists: &[PlaylistRow]) {
        let selected = self.list.selected_row().map(|r| r.widget_name());
        for row in self.playlist_rows.take() {
            self.list.remove(&row);
        }
        self.counts
            .borrow_mut()
            .retain(|(page, _)| !page.starts_with("playlist:"));
        let position = i32::try_from(LIBRARY.len() + self.pin_rows.borrow().len()).unwrap_or(0);
        let mut rows = Vec::new();
        let closed = self.closed.borrow().clone();
        for (i, playlist) in playlists.iter().enumerate() {
            let folder = playlist.kind == PlaylistType::Folder;
            let name = if folder {
                format!("folder:{}", playlist.key.0)
            } else {
                format!("playlist:{}", playlist.key.0)
            };
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&name);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            line.set_margin_start(18 * i32::try_from(playlist.depth).unwrap_or(0));
            // An empty folder shows closed.
            let open = folder
                && !closed.contains(&playlist.key)
                && playlists.iter().any(|p| p.parent == Some(playlist.key));
            let icon = if open {
                "folder-open-symbolic"
            } else if playlist.builtin {
                "starred-symbolic"
            } else {
                playlist_icon(playlist.kind, playlist.from_file)
            };
            line.append(&gtk::Image::from_icon_name(icon));
            let title = label(&playlist.name, &[]);
            title.set_hexpand(true);
            line.append(&title);
            if folder {
                // Not selectable: the arrow keys pass over it, Enter and a
                // click open or close it.
                row.set_selectable(false);
                let chevron = gtk::Image::from_icon_name(if open {
                    "pan-down-symbolic"
                } else {
                    "pan-end-symbolic"
                });
                chevron.set_pixel_size(12);
                chevron.add_css_class("hk-t3");
                line.append(&chevron);
                row.update_state(&[gtk::accessible::State::Expanded(Some(open))]);
            } else {
                let count = label(&crate::format::count(playlist.items), &["hk-count"]);
                count.set_xalign(1.0);
                line.append(&count);
            }
            row.set_child(Some(&line));
            row.update_property(&[gtk::accessible::Property::Label(&playlist.name)]);
            row.set_visible(!hidden_by(playlists, playlist, &closed));
            // The steps of a generator playlist choose its tracks: it takes
            // no dropped tracks.
            let generator = playlist.kind == PlaylistType::Generator;
            self.attach_playlist_controllers(&row, playlist.key, folder, generator);
            self.list
                .insert(&row, position + i32::try_from(i).unwrap_or(0));
            rows.push(row);
        }
        self.playlists.replace(playlists.to_vec());
        self.playlist_rows.replace(rows);
        self.list.invalidate_headers();
        if let Some(selected) = selected {
            self.select(Some(selected.as_str()));
        }
    }

    /// Opens or closes the folder `key`.
    pub(crate) fn toggle_folder(self: &Rc<Self>, key: PlaylistKey) {
        {
            let mut closed = self.closed.borrow_mut();
            if !closed.remove(&key) {
                closed.insert(key);
            }
        }
        let playlists = self.playlists.borrow().clone();
        let focused = format!("folder:{}", key.0);
        self.set_playlists(&playlists);
        if let Some(row) = self
            .playlist_rows
            .borrow()
            .iter()
            .find(|r| r.widget_name() == focused.as_str())
        {
            row.grab_focus();
        }
    }

    /// Lists what is pinned between the library rows and the playlists; the
    /// section shows only while something is pinned.
    pub(crate) fn set_pins(self: &Rc<Self>, pins: &[PinRow], covers: &Rc<CoverCache>) {
        let selected = self.list.selected_row().map(|r| r.widget_name());
        for row in self.pin_rows.take() {
            self.list.remove(&row);
        }
        let position = i32::try_from(LIBRARY.len()).unwrap_or(0);
        let mut rows = Vec::new();
        for (i, pin) in pins.iter().enumerate() {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&pin.key.row_name());
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            match pin.key {
                PinKey::Album(_) => {
                    let art = CoverArt::new(16, 3.0);
                    covers.show(&art, pin.cover, pin.cover_seed, 16);
                    line.append(&art);
                }
                PinKey::Artist(_) => {
                    let art = CoverArt::new(16, 8.0);
                    art.show(pin.cover_seed, None);
                    line.append(&art);
                }
                PinKey::Playlist(_) => {
                    line.append(&gtk::Image::from_icon_name(playlist_icon(
                        pin.playlist.unwrap_or_default(),
                        false,
                    )));
                }
            }
            let title = label(&pin.name, &[]);
            title.set_hexpand(true);
            line.append(&title);
            row.set_child(Some(&line));
            row.update_property(&[gtk::accessible::Property::Label(&pin.name)]);
            self.attach_pin_controllers(&row, pin.key);
            self.list
                .insert(&row, position + i32::try_from(i).unwrap_or(0));
            rows.push(row);
        }
        self.pin_rows.replace(rows);
        self.list.invalidate_headers();
        if let Some(selected) = selected {
            self.select(Some(selected.as_str()));
        }
    }

    fn attach_pin_controllers(self: &Rc<Self>, row: &gtk::ListBoxRow, key: PinKey) {
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, _, _| {
            let (Some(sidebar), Some(widget)) = (weak.upgrade(), gesture.widget()) else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let menu = sidebar.pin_menu.borrow().clone();
            if let Some(menu) = menu {
                menu(&widget, key);
            }
        });
        row.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |controller, key_pressed, _, state| {
            let (Some(sidebar), Some(widget)) = (weak.upgrade(), controller.widget()) else {
                return gtk::glib::Propagation::Proceed;
            };
            if !crate::views::actions::is_menu_key(key_pressed, state) {
                return gtk::glib::Propagation::Proceed;
            }
            let menu = sidebar.pin_menu.borrow().clone();
            if let Some(menu) = menu {
                menu(&widget, key);
            }
            gtk::glib::Propagation::Stop
        });
        row.add_controller(keys);
    }

    /// Calls `menu` for the context menu of a pinned row.
    pub(crate) fn connect_pin_menu(&self, menu: impl Fn(&gtk::Widget, PinKey) + 'static) {
        self.pin_menu.replace(Some(Rc::new(menu)));
    }

    /// Calls `moved` with a playlist dropped on a folder, and the folder.
    pub(crate) fn connect_folder_drop(&self, moved: impl Fn(PlaylistKey, PlaylistKey) + 'static) {
        self.folder_drop.replace(Some(Rc::new(moved)));
    }

    /// The row named `name`.
    pub(crate) fn row(&self, name: &str) -> Option<gtk::ListBoxRow> {
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && row.widget_name() == name
            {
                return Some(row.clone());
            }
            child = widget.next_sibling();
        }
        None
    }

    fn attach_playlist_controllers(
        self: &Rc<Self>,
        row: &gtk::ListBoxRow,
        key: PlaylistKey,
        folder: bool,
        generator: bool,
    ) {
        // Dragging a playlist or folder, onto a folder.
        let source = gtk::DragSource::new();
        source.set_actions(gtk::gdk::DragAction::MOVE);
        source.connect_prepare(move |_, _, _| {
            Some(gtk::gdk::ContentProvider::for_value(
                &DraggedPlaylist(key).to_value(),
            ))
        });
        row.add_controller(source);
        if folder {
            let target =
                gtk::DropTarget::new(DraggedPlaylist::static_type(), gtk::gdk::DragAction::MOVE);
            let weak = Rc::downgrade(self);
            target.connect_drop(move |_, value, _, _| {
                let (Some(sidebar), Ok(dragged)) = (weak.upgrade(), value.get::<DraggedPlaylist>())
                else {
                    return false;
                };
                if dragged.0 == key {
                    return false;
                }
                let moved = sidebar.folder_drop.borrow().clone();
                if let Some(moved) = moved {
                    moved(dragged.0, key);
                }
                true
            });
            let row_weak = row.downgrade();
            target.connect_enter(move |_, _, _| {
                if let Some(row) = row_weak.upgrade() {
                    row.add_css_class("hk-drop-into");
                }
                gtk::gdk::DragAction::MOVE
            });
            let row_weak = row.downgrade();
            target.connect_leave(move |_| {
                if let Some(row) = row_weak.upgrade() {
                    row.remove_css_class("hk-drop-into");
                }
            });
            row.add_controller(target);
        }
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, _, _| {
            let (Some(sidebar), Some(widget)) = (weak.upgrade(), gesture.widget()) else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let menu = sidebar.playlist_menu.borrow().clone();
            if let Some(menu) = menu {
                menu(&widget, key);
            }
        });
        row.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |controller, key_pressed, _, state| {
            let (Some(sidebar), Some(widget)) = (weak.upgrade(), controller.widget()) else {
                return gtk::glib::Propagation::Proceed;
            };
            if !crate::views::actions::is_menu_key(key_pressed, state) {
                return gtk::glib::Propagation::Proceed;
            }
            let menu = sidebar.playlist_menu.borrow().clone();
            if let Some(menu) = menu {
                menu(&widget, key);
            }
            gtk::glib::Propagation::Stop
        });
        row.add_controller(keys);
        if folder || generator {
            return;
        }
        let target = DraggedTracks::target(gtk::gdk::DragAction::COPY);
        let weak = Rc::downgrade(self);
        target.connect_drop(move |_, value, _, _| {
            let (Some(sidebar), Ok(dragged)) = (weak.upgrade(), value.get::<DraggedTracks>())
            else {
                return false;
            };
            let drop = sidebar.playlist_drop.borrow().clone();
            if let Some(drop) = drop {
                drop(key, dragged);
            }
            true
        });
        let row_weak = row.downgrade();
        target.connect_enter(move |_, _, _| {
            if let Some(row) = row_weak.upgrade() {
                row.add_css_class("hk-drop-into");
            }
            gtk::gdk::DragAction::COPY
        });
        let row_weak = row.downgrade();
        target.connect_leave(move |_| {
            if let Some(row) = row_weak.upgrade() {
                row.remove_css_class("hk-drop-into");
            }
        });
        row.add_controller(target);
    }

    /// Calls `menu` for the context menu of a playlist row.
    pub(crate) fn connect_playlist_menu(&self, menu: impl Fn(&gtk::Widget, PlaylistKey) + 'static) {
        self.playlist_menu.replace(Some(Rc::new(menu)));
    }

    /// Calls `drop` with tracks dropped on a playlist row.
    pub(crate) fn connect_drop(&self, drop: impl Fn(PlaylistKey, DraggedTracks) + 'static) {
        self.playlist_drop.replace(Some(Rc::new(drop)));
    }

    /// Calls `navigate` with the page name when the user picks a row.
    pub(crate) fn connect_navigate(&self, navigate: impl Fn(&str) + 'static) {
        self.navigate.replace(Some(Box::new(navigate)));
    }

    /// Selects the row of `page` without navigating; no row for other pages.
    pub(crate) fn select(&self, page: Option<&str>) {
        self.quiet.set(true);
        let mut child = self.list.first_child();
        let mut found = None;
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && Some(row.widget_name().as_str()) == page
            {
                found = Some(row.clone());
            }
            child = widget.next_sibling();
        }
        self.list.select_row(found.as_ref());
        self.quiet.set(false);
    }

    /// Shows the library totals beside the rows.
    pub(crate) fn set_counts(&self, counts: &LibraryCounts) {
        // The Labels row shows once a label exists.
        if let Some(row) = self.row("labels") {
            row.set_visible(counts.labels > 0);
        }
        for (page, label) in self.counts.borrow().iter() {
            let value = match page.as_str() {
                "albums" => Some(counts.albums),
                "artists" => Some(counts.artists),
                "tracks" => Some(counts.tracks),
                "genres" => Some(counts.genres),
                "composers" => Some(counts.composers),
                "years" => Some(counts.years),
                "labels" => Some(counts.labels),
                _ => None,
            };
            label.set_text(
                &value
                    .filter(|v| *v > 0)
                    .map(crate::format::count)
                    .unwrap_or_default(),
            );
        }
    }

    /// The row to focus when the sidebar is entered from the keyboard: the
    /// selected one, else the first; the arrow keys move on from there.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.list
            .selected_row()
            .or_else(|| self.list.row_at_index(0))
            .map_or_else(|| self.list.clone().upcast(), Cast::upcast)
    }
}
