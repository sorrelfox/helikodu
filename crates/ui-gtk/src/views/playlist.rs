// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A static playlist: the hero (a mosaic of the first covers, the name, a
//! summary, Play, Shuffle, Export and more), the table of its entries, and the
//! status bar.
//!
//! Entries are reordered by dragging them: the dragged rows fade, a line marks
//! where they go, and the status bar says so. Tracks dragged in from other
//! lists are inserted where they are dropped. Delete removes the selected
//! entries, with Undo in the toast; Ctrl+Z undoes the last change of the
//! playlist and Ctrl+Shift+Z does it again. A locked playlist hides the drag
//! handles and ignores drops and removals.
//!
//! Grouped by album, the entries that follow each other on one album get a
//! group row (cover, album, artist and year, count and time); the Album
//! column then gives way to the format. An entry whose file is missing reads
//! "Missing" in place of its length.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::backend::{Backend, PlayerRequest, PlaylistKey, PlaylistRow, TrackKey, TrackRow};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::{CoverCache, TrackItem};
use crate::playback::{Change, Playback, PlayingKeys};
use crate::views::actions::{self, Origin, Target};
use crate::views::artists::button;
use crate::views::drag::DraggedTracks;
use crate::views::table::{TableCell, cell_at, selected_positions};
use crate::widgets::{CoverArt, PlayingIndicator, TitleLine, label};

/// "edited yesterday", from when a playlist last changed.
fn edited(modified: Option<SystemTime>) -> Option<String> {
    let modified = modified?;
    let then = glib::DateTime::from_unix_local(
        i64::try_from(
            modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()?
                .as_secs(),
        )
        .ok()?,
    )
    .ok()?;
    let now = glib::DateTime::now_local().ok()?;
    let day = |d: &glib::DateTime| d.year() * 400 + d.day_of_year();
    let days = day(&now) - day(&then);
    Some(match days {
        ..=0 => tr("edited today"),
        1 => tr("edited yesterday"),
        2..=6 => fill(
            &ntr("edited {} day ago", "edited {} days ago", days as usize),
            &[&days.to_string()],
        ),
        _ => fill(&tr("edited {}"), &[then.format("%x").ok()?.as_str()]),
    })
}

/// The columns of the table: label, width (0 expands), right-aligned. In
/// groups by album the format takes the Album column's place.
fn columns(grouped: bool) -> Vec<(String, i32, bool)> {
    let mut columns = vec![
        (String::new(), 16, false),
        ("#".to_owned(), 22, true),
        (tr("Title"), 0, false),
        (tr("Artist"), 0, false),
    ];
    if grouped {
        columns.push((tr("Format"), 110, false));
        columns.push((tr("Length"), 64, true));
    } else {
        columns.push((tr("Album"), 0, false));
        columns.push((tr("Length"), 48, true));
    }
    columns
}

/// The entries that follow each other on one album, from `start`: how many,
/// and their length.
fn group_at(rows: &[TrackRow], start: usize) -> (usize, Duration) {
    let Some(first) = rows.get(start) else {
        return (0, Duration::ZERO);
    };
    let same = |r: &TrackRow| {
        if first.album.0 != 0 {
            r.album == first.album
        } else {
            r.album_title == first.album_title
        }
    };
    let run: Vec<&TrackRow> = rows[start..].iter().take_while(|r| same(r)).collect();
    (run.len(), run.iter().filter_map(|r| r.duration).sum())
}

/// Whether the entry at `position` starts a group.
fn starts_group(rows: &[TrackRow], position: usize) -> bool {
    match (
        position.checked_sub(1).and_then(|p| rows.get(p)),
        rows.get(position),
    ) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(before), Some(row)) => {
            if row.album.0 != 0 {
                before.album != row.album
            } else {
                before.album_title != row.album_title
            }
        }
    }
}

/// A playlist page.
pub(crate) struct PlaylistView {
    pub(crate) root: gtk::Box,
    mosaic: [CoverArt; 4],
    name: gtk::Label,
    summary: gtk::Label,
    list: gtk::ListView,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    status_left: gtk::Label,
    status_right: gtk::Label,
    backend: Backend,
    covers: Rc<CoverCache>,
    key: Cell<Option<PlaylistKey>>,
    rows: RefCell<Vec<TrackRow>>,
    info: RefCell<Option<PlaylistRow>>,
    generation: Cell<u64>,
    playing: Cell<PlayingKeys>,
    cells: RefCell<Vec<glib::WeakRef<TableCell>>>,
    drop_line: RefCell<Option<TableCell>>,
    header: gtk::Box,
    lock: gtk::ToggleButton,
    more: gtk::MenuButton,
    /// Set while the lock button is set from the playlist, so it changes nothing.
    quiet: Cell<bool>,
    /// The page's rename and delete actions, off for Favourites.
    page_actions: RefCell<Option<gio::SimpleActionGroup>>,
}

impl std::fmt::Debug for PlaylistView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaylistView")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl PlaylistView {
    /// The page over `backend`.
    pub(crate) fn new(
        backend: &Backend,
        playback: &Rc<Playback>,
        covers: &Rc<CoverCache>,
    ) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let hero = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        hero.add_css_class("hk-playlist-hero");
        let grid = gtk::Grid::new();
        grid.add_css_class("hk-mosaic");
        grid.set_valign(gtk::Align::Center);
        grid.set_overflow(gtk::Overflow::Hidden);
        let mosaic = [
            CoverArt::new(44, 0.0),
            CoverArt::new(44, 0.0),
            CoverArt::new(44, 0.0),
            CoverArt::new(44, 0.0),
        ];
        for (i, art) in mosaic.iter().enumerate() {
            art.add_css_class("hk-mosaic-tile");
            grid.attach(art, (i % 2) as i32, (i / 2) as i32, 1, 1);
        }
        hero.append(&grid);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        titles.set_valign(gtk::Align::Center);
        titles.append(&label(&tr("Playlist"), &["hk-section-label"]));
        let name = label("", &["hk-hero-title", "small"]);
        name.set_accessible_role(gtk::AccessibleRole::Heading);
        titles.append(&name);
        let summary = label("", &["hk-t3"]);
        titles.append(&summary);
        hero.append(&titles);
        let play = button(&tr("Play"), Some("media-playback-start-symbolic"), true);
        let shuffle = button(
            &tr("Shuffle"),
            Some("media-playlist-shuffle-symbolic"),
            false,
        );
        let export = button(&tr("Export"), Some("helikodu-export-symbolic"), false);
        let lock = gtk::ToggleButton::new();
        lock.set_icon_name("helikodu-lock-symbolic");
        lock.add_css_class("hk-more");
        lock.add_css_class("hk-lock");
        lock.set_tooltip_text(Some(&tr("Lock against changes")));
        lock.update_property(&[gtk::accessible::Property::Label(&tr(
            "Lock against changes",
        ))]);
        let more = gtk::MenuButton::new();
        more.set_icon_name("view-more-symbolic");
        more.add_css_class("hk-more");
        more.set_tooltip_text(Some(&tr("More")));
        more.update_property(&[gtk::accessible::Property::Label(&tr("More"))]);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_valign(gtk::Align::Center);
        for b in [
            play.upcast_ref::<gtk::Widget>(),
            shuffle.upcast_ref(),
            export.upcast_ref(),
            lock.upcast_ref(),
            more.upcast_ref(),
        ] {
            buttons.append(b);
        }
        hero.append(&buttons);
        root.append(&hero);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        header.add_css_class("hk-table-header");
        header.add_css_class("hk-playlist-row");
        root.append(&header);

        let store = gio::ListStore::new::<TrackItem>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let list = gtk::ListView::new(Some(selection.clone()), None::<gtk::ListItemFactory>);
        list.add_css_class("hk-playlist");
        list.set_vexpand(true);
        list.update_property(&[gtk::accessible::Property::Label(&tr("Playlist entries"))]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        root.append(&scroller);
        let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        status.add_css_class("hk-statusbar");
        let status_left = label("", &[]);
        status_left.set_hexpand(true);
        status.append(&status_left);
        let status_right = label("", &[]);
        status.append(&status_right);
        root.append(&status);

        let view = Rc::new(Self {
            root,
            mosaic,
            name,
            summary,
            list,
            store,
            selection,
            status_left,
            status_right,
            backend: backend.clone(),
            covers: covers.clone(),
            key: Cell::new(None),
            rows: RefCell::new(Vec::new()),
            info: RefCell::new(None),
            generation: Cell::new(0),
            playing: Cell::new(PlayingKeys::default()),
            cells: RefCell::new(Vec::new()),
            drop_line: RefCell::new(None),
            header,
            lock,
            more,
            quiet: Cell::new(false),
            page_actions: RefCell::new(None),
        });
        view.list.set_factory(Some(&view.factory()));
        view.fill_header(false);
        view.connect(playback, &play, &shuffle, &export);
        view.connect_menu();
        view
    }

    /// Fills the table header for the grouping.
    fn fill_header(&self, grouped: bool) {
        while let Some(child) = self.header.first_child() {
            self.header.remove(&child);
        }
        for (i, (text, width, right)) in columns(grouped).into_iter().enumerate() {
            let l = label(&text, &["hk-section-label"]);
            if width > 0 {
                l.set_size_request(width, -1);
            } else {
                l.set_hexpand(true);
                l.set_max_width_chars(1);
            }
            if right {
                l.set_xalign(1.0);
            }
            if i == 2 && !grouped {
                // The rows' title cell starts with a cover; the same room here
                // keeps the columns of header and rows in line.
                let cell = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                cell.set_hexpand(true);
                cell.append(&l);
                let room = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                room.set_size_request(24, -1);
                cell.append(&room);
                self.header.append(&cell);
            } else {
                self.header.append(&l);
            }
        }
    }

    fn grouped(&self) -> bool {
        self.info.borrow().as_ref().is_some_and(|i| i.grouped)
    }

    fn locked(&self) -> bool {
        self.info.borrow().as_ref().is_some_and(|i| i.locked)
    }

    fn factory(self: &Rc<Self>) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
            // The group row, shown above the first entry of a group.
            let group = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            group.add_css_class("hk-group-row");
            let group_cover = CoverArt::new(32, 4.0);
            group.append(&group_cover);
            let album = gtk::Button::new();
            album.add_css_class("hk-link");
            album.set_child(Some(&label("", &["hk-strong"])));
            group.append(&album);
            let about = label("", &["hk-t3"]);
            about.set_hexpand(true);
            group.append(&about);
            let size = label("", &["hk-t3", "hk-numeric", "hk-small"]);
            group.append(&size);
            group.set_visible(false);
            let weak_view = weak.clone();
            album.connect_clicked(move |button| {
                let (Some(view), Some(window)) = (
                    weak_view.upgrade(),
                    button.root().and_downcast::<crate::window::MainWindow>(),
                ) else {
                    return;
                };
                let key: Option<u64> = button.widget_name().parse().ok();
                if let Some(key) = key.filter(|k| *k != 0) {
                    let _ = view;
                    window.open_album(crate::backend::AlbumKey(key));
                }
            });
            outer.append(&group);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            line.add_css_class("hk-playlist-row");
            let handle = gtk::Image::from_icon_name("helikodu-drag-handle-symbolic");
            handle.add_css_class("hk-drag-handle");
            handle.set_size_request(16, -1);
            line.append(&handle);
            let lead = gtk::Stack::new();
            lead.set_size_request(22, -1);
            let number = label("", &["hk-t3", "hk-numeric"]);
            number.set_xalign(1.0);
            lead.add_named(&number, Some("number"));
            let indicator = PlayingIndicator::default();
            indicator.set_halign(gtk::Align::End);
            lead.add_named(&indicator, Some("playing"));
            line.append(&lead);
            let title_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            title_box.set_hexpand(true);
            let cover = CoverArt::new(24, 4.0);
            cover.add_css_class("small");
            title_box.append(&cover);
            let warning = gtk::Image::from_icon_name("dialog-warning-symbolic");
            warning.set_visible(false);
            warning.set_tooltip_text(Some(&tr("The file is missing")));
            title_box.append(&warning);
            let title = TitleLine::new("", &["hk-medium"]);
            title_box.append(&title);
            line.append(&title_box);
            for _ in 0..2 {
                let l = label("", &["hk-t2"]);
                l.set_hexpand(true);
                l.set_max_width_chars(1);
                line.append(&l);
            }
            let format = label("", &["hk-t3", "hk-mono-meta"]);
            format.set_size_request(110, -1);
            line.append(&format);
            let length = label("", &["hk-t2", "hk-numeric"]);
            length.set_xalign(1.0);
            length.set_size_request(48, -1);
            line.append(&length);
            outer.append(&line);
            let cell = TableCell::new(&outer);
            if let Some(view) = weak.upgrade() {
                view.cells.borrow_mut().push(cell.downgrade());
            }
            item.set_child(Some(&cell));
        });
        let weak = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(cell), Some(track), Some(view)) = (
                item.child().and_downcast::<TableCell>(),
                item.item().and_downcast::<TrackItem>(),
                weak.upgrade(),
            ) else {
                return;
            };
            cell.set_position(Some(track.position()));
            let w = Rc::downgrade(&view);
            cell.follow(&track, move |cell, track| {
                if let Some(view) = w.upgrade() {
                    view.show_row(cell, track);
                }
            });
        });
        factory.connect_unbind(|_, item| {
            if let Some(cell) = item
                .downcast_ref::<gtk::ListItem>()
                .and_then(|i| i.child().and_downcast::<TableCell>())
            {
                cell.forget();
                cell.set_position(None);
            }
        });
        factory
    }

    fn show_row(&self, cell: &TableCell, track: &TrackItem) {
        let Some(row) = track.row() else { return };
        let Some(outer) = cell.child() else { return };
        let (Some(group), Some(line)) = (outer.first_child(), outer.last_child()) else {
            return;
        };
        let children = |widget: &gtk::Widget| -> Vec<gtk::Widget> {
            let mut out = Vec::new();
            let mut child = widget.first_child();
            while let Some(w) = child {
                child = w.next_sibling();
                out.push(w);
            }
            out
        };
        let grouped = self.grouped();
        let position = track.position() as usize;
        let first = grouped && starts_group(&self.rows.borrow(), position);
        group.set_visible(first);
        if first {
            let parts = children(&group);
            let (count, time) = group_at(&self.rows.borrow(), position);
            if let [cover, album, about, size] = parts.as_slice() {
                if let Some(cover) = cover.downcast_ref::<CoverArt>() {
                    self.covers.show(cover, row.cover, row.cover_seed, 32);
                }
                if let Some(album) = album.downcast_ref::<gtk::Button>() {
                    album.set_widget_name(&row.album.0.to_string());
                    album.set_sensitive(row.album.0 != 0);
                    if let Some(l) = album.child().and_downcast::<gtk::Label>() {
                        l.set_text(&row.album_title);
                    }
                }
                if let Some(about) = about.downcast_ref::<gtk::Label>() {
                    let mut text = vec![String::new(), row.artist.clone()];
                    text.extend(row.year.map(|y| y.to_string()));
                    about.set_text(&text.join(" · "));
                }
                if let Some(size) = size.downcast_ref::<gtk::Label>() {
                    size.set_text(&fill(
                        &tr("{} · {}"),
                        &[
                            &fill(&ntr("{} track", "{} tracks", count), &[&count.to_string()]),
                            &format::minutes(time),
                        ],
                    ));
                }
            }
            if position == 0 {
                group.add_css_class("first");
            } else {
                group.remove_css_class("first");
            }
        }
        let parts = children(&line);
        let [handle, lead, title_box, artist, album, format_label, length] = parts.as_slice()
        else {
            return;
        };
        handle.set_opacity(if self.locked() { 0.0 } else { 1.0 });
        let playing = !row.missing && self.playing.get().contains(row.key);
        if let Some(lead) = lead.downcast_ref::<gtk::Stack>() {
            lead.set_visible_child_name(if playing { "playing" } else { "number" });
            if let Some(number) = lead.child_by_name("number").and_downcast::<gtk::Label>() {
                number.set_text(&(position + 1).to_string());
            }
        }
        let title_parts = children(title_box);
        if let [cover, warning, title] = title_parts.as_slice() {
            cover.set_visible(!grouped);
            if let Some(cover) = cover.downcast_ref::<CoverArt>()
                && !grouped
            {
                self.covers.show(cover, row.cover, row.cover_seed, 24);
            }
            warning.set_visible(row.missing);
            if let Some(title) = title.downcast_ref::<TitleLine>() {
                title.set_text(&row.title);
                title.set_copies(row.key, if row.missing { 0 } else { row.copies });
            }
        }
        if let Some(l) = artist.downcast_ref::<gtk::Label>() {
            l.set_text(&row.artist);
        }
        album.set_visible(!grouped);
        if let Some(l) = album.downcast_ref::<gtk::Label>() {
            l.set_text(&row.album_title);
        }
        format_label.set_visible(grouped);
        if let Some(l) = format_label.downcast_ref::<gtk::Label>() {
            l.set_text(&if row.missing {
                String::new()
            } else {
                format::short(&row.format)
            });
        }
        if let Some(l) = length.downcast_ref::<gtk::Label>() {
            l.set_size_request(if grouped { 64 } else { 48 }, -1);
            if row.missing {
                l.set_text(&tr("Missing"));
            } else {
                l.set_text(&row.duration.map(format::duration).unwrap_or_default());
            }
        }
        for (class, on) in [
            ("hk-playing", playing),
            ("hk-missing", row.missing),
            ("hk-grouped", grouped),
        ] {
            if on {
                cell.add_css_class(class);
            } else {
                cell.remove_css_class(class);
            }
        }
    }

    fn connect(
        self: &Rc<Self>,
        playback: &Rc<Playback>,
        play: &gtk::Button,
        shuffle: &gtk::Button,
        export: &gtk::Button,
    ) {
        for (button, random) in [(play, false), (shuffle, true)] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.play_from(0, random);
                }
            });
        }
        // Export offers the playlist file and a copy of its files in a folder.
        let exports = gio::SimpleActionGroup::new();
        for (name, to_folder) in [("file", false), ("folder", true)] {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let (Some(view), Some(window)) =
                    (weak.upgrade(), weak.upgrade().and_then(|v| v.window()))
                    && let Some(key) = view.key.get()
                {
                    if to_folder {
                        crate::tools::sync_export::open_with(&window, Some(key));
                    } else {
                        window.export_playlist(key);
                    }
                }
            });
            exports.add_action(&action);
        }
        export.insert_action_group("export", Some(&exports));
        let menu = gio::Menu::new();
        menu.append(Some(&tr("Save as a playlist file…")), Some("export.file"));
        menu.append(
            Some(&tr("Copy the files to a folder…")),
            Some("export.folder"),
        );
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(export);
        popover.set_has_arrow(false);
        let shown = popover.downgrade();
        export.connect_clicked(move |_| {
            if let Some(popover) = shown.upgrade() {
                popover.popup();
            }
        });
        export.connect_destroy(move |_| popover.unparent());
        let group = gio::SimpleActionGroup::new();
        for name in ["rename", "delete"] {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |action, _| {
                let Some(view) = weak.upgrade() else { return };
                let (Some(key), Some(window)) = (view.key.get(), view.window()) else {
                    return;
                };
                if action.name() == "rename" {
                    window.rename_playlist(key);
                } else {
                    window.delete_playlist(key);
                }
            });
            group.add_action(&action);
        }
        self.root.insert_action_group("playlist-page", Some(&group));
        self.page_actions.replace(Some(group));
        let weak = Rc::downgrade(self);
        self.list.connect_activate(move |_, position| {
            if let Some(view) = weak.upgrade() {
                view.play_from(position as usize, false);
            }
        });
        let weak = Rc::downgrade(self);
        playback.connect(move |player, change| {
            if let Some(view) = weak.upgrade()
                && change == Change::Playing
                && view.playing.replace(player.playing) != player.playing
            {
                view.redraw();
            }
        });
        let weak = Rc::downgrade(self);
        self.selection.connect_selection_changed(move |_, _, _| {
            if let Some(view) = weak.upgrade() {
                view.show_status();
            }
        });
        // Context menu, Menu key, Delete.
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            if let Some(position) = cell_at(&view.list, x, y).and_then(|c| c.position()) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if !view.selection.is_selected(position) {
                    view.selection.select_item(position, true);
                }
                if let Some(target) = view.target() {
                    actions::popup(&view.list, target, Some((x, y)));
                }
            }
        });
        self.list.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if key == gdk::Key::F2
                && let (Some(playlist), Some(window)) = (view.key.get(), view.window())
            {
                window.rename_playlist(playlist);
                return glib::Propagation::Stop;
            }
            let Some(target) = view.target() else {
                return glib::Propagation::Proceed;
            };
            if actions::is_menu_key(key, state) {
                actions::popup(&view.list, target, None);
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::Delete
                && !view.locked()
                && let Some(window) = view.window()
            {
                window.set_target(Some(target));
                WidgetExt::activate_action(&window, "win.sel-remove", None).ok();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.list.add_controller(keys);
        // Dragging rows out (and within, to reorder).
        let weak = Rc::downgrade(self);
        let source = DraggedTracks::source(&self.list, move |x, y| {
            let view = weak.upgrade()?;
            let key = view.key.get()?;
            let position = cell_at(&view.list, x, y)?.position()?;
            if !view.selection.is_selected(position) {
                view.selection.select_item(position, true);
            }
            let positions: Vec<usize> = selected_positions(&view.selection)
                .into_iter()
                .map(|p| p as usize)
                .collect();
            let rows = view.rows.borrow();
            let keys = positions
                .iter()
                .filter_map(|p| rows.get(*p))
                .filter(|r| !r.missing)
                .map(|r| r.key)
                .collect();
            let label = match positions.as_slice() {
                [one] => rows.get(*one).map(|r| r.title.clone()).unwrap_or_default(),
                many => fill(
                    &ntr("{} track", "{} tracks", many.len()),
                    &[&crate::format::count(many.len())],
                ),
            };
            Some((
                DraggedTracks {
                    keys,
                    album: None,
                    from_playlist: Some((key, positions)),
                },
                label,
            ))
        });
        let weak = Rc::downgrade(self);
        source.connect_drag_begin(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.mark_dragged(true);
            }
        });
        let weak = Rc::downgrade(self);
        source.connect_drag_end(move |_, _, _| {
            if let Some(view) = weak.upgrade() {
                view.mark_dragged(false);
                view.clear_drop_line();
                view.show_status();
            }
        });
        let target = DraggedTracks::target(gdk::DragAction::COPY | gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        target.connect_motion(move |target, x, y| {
            let Some(view) = weak.upgrade() else {
                return gdk::DragAction::empty();
            };
            if view.locked() {
                return gdk::DragAction::empty();
            }
            let moving = target
                .value()
                .and_then(|v| v.get::<DraggedTracks>().ok())
                .and_then(|d| d.from_playlist.filter(|(k, _)| Some(*k) == view.key.get()))
                .map(|(_, positions)| positions.len());
            let to = view.drop_position(x, y);
            match moving {
                Some(count) => {
                    view.status_left.set_text(&fill(
                        &ntr(
                            "Moving {} track to position {}",
                            "Moving {} tracks to position {}",
                            count,
                        ),
                        &[&count.to_string(), &(to + 1).to_string()],
                    ));
                    view.status_right
                        .set_text(&tr("Esc cancels · Ctrl Z undoes"));
                    gdk::DragAction::MOVE
                }
                None => gdk::DragAction::COPY,
            }
        });
        let weak = Rc::downgrade(self);
        target.connect_leave(move |_| {
            if let Some(view) = weak.upgrade() {
                view.clear_drop_line();
            }
        });
        let weak = Rc::downgrade(self);
        target.connect_drop(move |_, value, x, y| {
            let (Some(view), Ok(dragged)) = (weak.upgrade(), value.get::<DraggedTracks>()) else {
                return false;
            };
            if view.locked() {
                view.clear_drop_line();
                return false;
            }
            let to = view.drop_position(x, y);
            view.clear_drop_line();
            view.dropped(dragged, to);
            true
        });
        self.list.add_controller(target);
    }

    fn window(&self) -> Option<crate::window::MainWindow> {
        self.root.root().and_downcast::<crate::window::MainWindow>()
    }

    /// The entry position a drop at `(x, y)` goes before; draws the line.
    fn drop_position(&self, x: f64, y: f64) -> usize {
        let count = self.rows.borrow().len();
        let Some(cell) = cell_at(&self.list, x, y) else {
            self.clear_drop_line();
            return count;
        };
        let Some(position) = cell.position() else {
            return count;
        };
        let below = cell
            .compute_point(&self.list, &gtk::graphene::Point::new(0.0, 0.0))
            .is_some_and(|p| y - f64::from(p.y()) > f64::from(cell.height()) / 2.0);
        self.clear_drop_line();
        cell.add_css_class(if below {
            "hk-drop-below"
        } else {
            "hk-drop-above"
        });
        self.drop_line.replace(Some(cell));
        (position as usize + usize::from(below)).min(count)
    }

    fn clear_drop_line(&self) {
        if let Some(cell) = self.drop_line.take() {
            cell.remove_css_class("hk-drop-above");
            cell.remove_css_class("hk-drop-below");
        }
    }

    fn mark_dragged(&self, on: bool) {
        let selected = selected_positions(&self.selection);
        for cell in self
            .cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
        {
            let dragged = on && cell.position().is_some_and(|p| selected.contains(&p));
            if dragged {
                cell.add_css_class("hk-dragged");
            } else {
                cell.remove_css_class("hk-dragged");
            }
        }
    }

    fn dropped(&self, dragged: DraggedTracks, to: usize) {
        let Some(key) = self.key.get() else { return };
        let Some(window) = self.window() else { return };
        match dragged.from_playlist {
            Some((from, positions)) if from == key => {
                let backend = self.backend.clone();
                let weak = window.downgrade();
                glib::spawn_future_local(async move {
                    let result = backend
                        .query(move |l| l.move_in_playlist(key, &positions, to))
                        .await;
                    if let Some(window) = weak.upgrade() {
                        if let Err(error) = result {
                            window.toast(&error.0, true);
                        }
                        window.playlists_changed(Some(key));
                    }
                });
            }
            _ => window.drop_on_playlist(key, dragged, Some(to)),
        }
    }

    /// Shows the playlist `key`.
    pub(crate) fn show(self: &Rc<Self>, key: PlaylistKey) {
        if self.key.replace(Some(key)) != Some(key) {
            self.selection.unselect_all();
            self.store.remove_all();
            self.rows.borrow_mut().clear();
            self.name.set_text("");
            self.summary.set_text("");
        }
        self.reload();
    }

    /// The playlist shown.
    pub(crate) fn key(&self) -> Option<PlaylistKey> {
        self.key.get()
    }

    /// Fetches the entries again.
    pub(crate) fn reload(self: &Rc<Self>) {
        let Some(key) = self.key.get() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| {
                    let info = l.playlists()?.into_iter().find(|p| p.key == key);
                    Ok((info, l.playlist_items(key)?))
                })
                .await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            match result {
                Ok((info, rows)) => view.show_rows(info, rows),
                Err(error) => {
                    if let Some(window) = view.window() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    fn show_rows(&self, info: Option<PlaylistRow>, rows: Vec<TrackRow>) {
        if let Some(info) = &info {
            self.name.set_text(&info.name);
            let mut parts = vec![
                fill(
                    &ntr("{} track", "{} tracks", info.items),
                    &[&crate::format::count(info.items)],
                ),
                format::minutes(info.duration),
            ];
            parts.extend(edited(info.modified));
            self.summary.set_text(&parts.join(" · "));
        }
        let grouped = info.as_ref().is_some_and(|i| i.grouped);
        let was_grouped = self.grouped();
        let locked = info.as_ref().is_some_and(|i| i.locked);
        // Favourites cannot be locked, renamed or deleted: the heart must
        // always reach it.
        let builtin = info.as_ref().is_some_and(|i| i.builtin);
        self.lock.set_visible(!builtin);
        if let Some(group) = self.page_actions.borrow().as_ref() {
            for name in ["rename", "delete"] {
                if let Some(action) = group
                    .lookup_action(name)
                    .and_downcast::<gio::SimpleAction>()
                {
                    action.set_enabled(!builtin);
                }
            }
        }
        self.info.replace(info);
        self.quiet.set(true);
        self.lock.set_active(locked);
        self.quiet.set(false);
        self.lock.set_tooltip_text(Some(&if locked {
            tr("Unlock")
        } else {
            tr("Lock against changes")
        }));
        if grouped != was_grouped || self.header.first_child().is_none() {
            self.fill_header(grouped);
        }
        let mut seeds = Vec::new();
        for row in rows.iter().filter(|r| !r.missing) {
            if !seeds.iter().any(|(s, _)| *s == row.cover_seed) {
                seeds.push((row.cover_seed, row.cover));
            }
            if seeds.len() == 4 {
                break;
            }
        }
        for (i, art) in self.mosaic.iter().enumerate() {
            match seeds.get(i).or(seeds.first()) {
                Some((seed, cover)) => self.covers.show(art, *cover, *seed, 44),
                None => art.show(i as u64 * 7 + 3, None),
            }
        }
        let selected = selected_positions(&self.selection);
        // The same entries again (after an edit): their rows change in place,
        // so the list keeps its scroll position.
        let same = self.store.n_items() as usize == rows.len()
            && self
                .rows
                .borrow()
                .iter()
                .map(|r| r.key)
                .eq(rows.iter().map(|r| r.key));
        if same {
            self.rows.replace(rows.clone());
            for (i, r) in rows.iter().enumerate() {
                if let Some(item) = self.store.item(i as u32).and_downcast::<TrackItem>() {
                    item.fill(Rc::new(r.clone()));
                }
            }
            self.show_status();
            return;
        }
        let items: Vec<TrackItem> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| TrackItem::new(r.key, i as u32, Some(Rc::new(r.clone()))))
            .collect();
        self.rows.replace(rows);
        self.store.splice(0, self.store.n_items(), &items);
        for p in selected {
            if p < self.store.n_items() {
                self.selection.select_item(p, false);
            }
        }
        self.show_status();
    }

    /// Keeps a changed rating in the rows of `keys`, in place.
    pub(crate) fn ratings_changed(&self, keys: &[TrackKey], stars: Option<u8>) {
        for row in self
            .rows
            .borrow_mut()
            .iter_mut()
            .filter(|r| keys.contains(&r.key))
        {
            row.rating = stars;
        }
        for i in 0..self.store.n_items() {
            if let Some(item) = self.store.item(i).and_downcast::<TrackItem>()
                && keys.contains(&item.key())
                && let Some(row) = item.row()
            {
                let mut row = (*row).clone();
                row.rating = stars;
                item.fill(Rc::new(row));
            }
        }
    }

    fn redraw(&self) {
        for cell in self
            .cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
        {
            if let Some(item) = cell.item() {
                self.show_row(&cell, &item);
            }
        }
    }

    fn show_status(&self) {
        let rows = self.rows.borrow();
        let selected = selected_positions(&self.selection);
        let length: Duration = if selected.is_empty() {
            rows.iter().filter_map(|r| r.duration).sum()
        } else {
            selected
                .iter()
                .filter_map(|p| rows.get(*p as usize))
                .filter_map(|r| r.duration)
                .sum()
        };
        let count = if selected.is_empty() {
            fill(
                &ntr("{} track", "{} tracks", rows.len()),
                &[&crate::format::count(rows.len())],
            )
        } else {
            fill(
                &ntr("{} selected", "{} selected", selected.len()),
                &[&crate::format::count(selected.len())],
            )
        };
        let mut left = fill(&tr("{} · {}"), &[&count, &format::minutes(length)]);
        let missing = rows.iter().filter(|r| r.missing).count();
        if selected.is_empty() && missing > 0 {
            left = fill(
                &tr("{} · {}"),
                &[
                    &left,
                    &fill(
                        &ntr("{} file is missing", "{} files are missing", missing),
                        &[&missing.to_string()],
                    ),
                ],
            );
        }
        self.status_left.set_text(&left);
        let mut right = Vec::new();
        if self.grouped() {
            right.push(tr("Grouped by album"));
        }
        if self.locked() {
            right.push(tr("Locked against changes"));
        } else if self.info.borrow().as_ref().is_some_and(|i| i.can_undo) {
            right.push(tr("Ctrl Z undoes the last change"));
        }
        self.status_right.set_text(&right.join(" · "));
    }

    /// The lock button and the more menu.
    fn connect_menu(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.lock.connect_toggled(move |lock| {
            let Some(view) = weak.upgrade() else { return };
            if view.quiet.get() {
                return;
            }
            if let (Some(key), Some(window)) = (view.key.get(), view.window()) {
                window.set_playlist_flag(key, true, lock.is_active());
            }
        });
        let weak = Rc::downgrade(self);
        self.more.set_create_popup_func(move |more| {
            if let Some(view) = weak.upgrade() {
                view.build_menu(more);
            }
        });
    }

    /// The more menu: grouping and the lock, tidying, the playlist's own
    /// commands, export and delete.
    fn build_menu(self: &Rc<Self>, more: &gtk::MenuButton) {
        let (Some(key), Some(window)) = (self.key.get(), self.window()) else {
            return;
        };
        let id = key.0.to_variant();
        let item = |label: &str, action: &str| {
            let item = gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(Some(action), Some(&id));
            item
        };
        let group = gio::SimpleActionGroup::new();
        for (name, on, lock) in [
            ("group", self.grouped(), false),
            ("lock", self.locked(), true),
        ] {
            let action = gio::SimpleAction::new_stateful(name, None, &on.to_variant());
            let weak = window.downgrade();
            action.connect_activate(move |action, _| {
                let on = !action
                    .state()
                    .and_then(|s| s.get::<bool>())
                    .unwrap_or(false);
                action.set_state(&on.to_variant());
                if let Some(window) = weak.upgrade() {
                    window.set_playlist_flag(key, lock, on);
                }
            });
            group.add_action(&action);
        }
        self.root.insert_action_group("playlist-menu", Some(&group));
        let menu = gio::Menu::new();
        let checks = gio::Menu::new();
        checks.append(Some(&tr("Group by album")), Some("playlist-menu.group"));
        checks.append(
            Some(&tr("Lock against changes")),
            Some("playlist-menu.lock"),
        );
        menu.append_section(None, &checks);
        let tidy = gio::Menu::new();
        for custom in ["dedupe", "missing"] {
            let entry = gio::MenuItem::new(None, None);
            entry.set_attribute_value("custom", Some(&custom.to_variant()));
            tidy.append_item(&entry);
        }
        tidy.append_item(&item(&tr("Shuffle the order…"), "win.playlist-shuffle"));
        menu.append_section(None, &tidy);
        // Rename, duplicate, move, pin, export and delete as on the sidebar.
        let shared = window.playlist_menu_model(key);
        for section in 1..shared.n_items() {
            if let Some(link) = shared.item_link(section, gio::MENU_LINK_SECTION) {
                menu.append_section(None, &link);
            }
        }
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.add_css_class("hk-playlist-menu");
        let locked = self.locked();
        for (custom, text, action) in [
            ("dedupe", tr("Remove duplicates"), "win.playlist-dedupe"),
            (
                "missing",
                tr("Remove missing tracks"),
                "win.playlist-remove-missing",
            ),
        ] {
            let button = gtk::Button::new();
            button.add_css_class("hk-menu-row");
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let name = label(&text, &[]);
            name.set_hexpand(true);
            line.append(&name);
            let count = label("", &["hk-t3", "hk-numeric"]);
            line.append(&count);
            button.set_child(Some(&line));
            button.set_action_name(Some(action));
            button.set_action_target_value(Some(&id));
            button.set_sensitive(false);
            let p = popover.downgrade();
            button.connect_clicked(move |_| {
                if let Some(p) = p.upgrade() {
                    p.popdown();
                }
            });
            popover.add_child(&button, custom);
            let backend = self.backend.clone();
            let weak = button.downgrade();
            let duplicates = custom == "dedupe";
            glib::spawn_future_local(async move {
                let Ok(counts) = backend.query(move |l| l.playlist_cleanup(key)).await else {
                    return;
                };
                let n = if duplicates {
                    counts.duplicates
                } else {
                    counts.missing
                };
                if let Some(button) = weak.upgrade() {
                    if let Some(count) = button
                        .child()
                        .and_then(|c| c.last_child())
                        .and_downcast::<gtk::Label>()
                    {
                        count.set_text(&n.to_string());
                    }
                    button.set_sensitive(n > 0 && !locked);
                }
            });
        }
        more.set_popover(Some(&popover));
    }

    fn play_from(&self, start: usize, shuffle: bool) {
        let rows = self.rows.borrow();
        let playable: Vec<(usize, TrackKey)> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.missing && r.key.0 != 0)
            .map(|(i, r)| (i, r.key))
            .collect();
        let start = playable.iter().position(|(i, _)| *i >= start).unwrap_or(0);
        let title = self
            .info
            .borrow()
            .as_ref()
            .map(|i| i.name.clone())
            .unwrap_or_default();
        let tracks = playable.into_iter().map(|(_, k)| k).collect();
        self.backend.send(match self.key.get() {
            Some(key) => PlayerRequest::PlayPlaylist {
                key,
                tracks,
                start,
                title,
                shuffle,
            },
            None => PlayerRequest::PlayTracks {
                tracks,
                start,
                title,
                shuffle,
            },
        });
    }

    /// The selected entries as a context menu target.
    pub(crate) fn target(&self) -> Option<Target> {
        let key = self.key.get()?;
        let positions = selected_positions(&self.selection);
        if positions.is_empty() {
            return None;
        }
        let rows = self.rows.borrow();
        let chosen: Vec<Rc<TrackRow>> = positions
            .iter()
            .filter_map(|p| rows.get(*p as usize))
            .map(|r| Rc::new(r.clone()))
            .collect();
        Some(Target::Tracks {
            rows: chosen,
            origin: Origin::Playlist {
                key,
                positions: positions.into_iter().map(|p| p as usize).collect(),
            },
        })
    }

    /// Asks for a new name in a popover under the name.
    pub(crate) fn ask_name(self: &Rc<Self>) {
        let Some(key) = self.key.get() else { return };
        let popover = gtk::Popover::new();
        popover.add_css_class("hk-rename");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.set_width_chars(28);
        entry.set_text(&self.name.text());
        entry.update_property(&[gtk::accessible::Property::Label(&tr("Playlist name"))]);
        line.append(&entry);
        let save = gtk::Button::with_label(&tr("Rename"));
        save.add_css_class("hk-btn");
        save.add_css_class("primary");
        save.add_css_class("compact");
        line.append(&save);
        popover.set_child(Some(&line));
        // On the hero box, which disposes of it, under the name.
        let Some(hero) = self.name.parent().and_then(|t| t.parent()) else {
            return;
        };
        popover.set_parent(&hero);
        if let Some(bounds) = self.name.compute_bounds(&hero) {
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                bounds.x() as i32,
                bounds.y() as i32,
                bounds.width() as i32,
                bounds.height() as i32,
            )));
        }
        popover.set_position(gtk::PositionType::Bottom);
        let weak = Rc::downgrade(self);
        let commit = move |entry: &gtk::Entry, popover: &gtk::Popover| {
            let name = entry.text().trim().to_owned();
            popover.popdown();
            let Some(view) = weak.upgrade() else { return };
            if name.is_empty() || name == view.name.text() {
                return;
            }
            let backend = view.backend.clone();
            let window = view.window().map(|w| w.downgrade());
            glib::spawn_future_local(async move {
                let result = backend.query(move |l| l.rename_playlist(key, &name)).await;
                if let Some(window) = window.and_then(|w| w.upgrade()) {
                    if let Err(error) = result {
                        window.toast(&error.0, true);
                    }
                    window.playlists_changed(Some(key));
                }
            });
        };
        let commit = Rc::new(commit);
        let (c, p) = (commit.clone(), popover.downgrade());
        entry.connect_activate(move |entry| {
            if let Some(p) = p.upgrade() {
                c(entry, &p);
            }
        });
        let (c, p, e) = (commit, popover.downgrade(), entry.downgrade());
        save.connect_clicked(move |_| {
            if let (Some(p), Some(e)) = (p.upgrade(), e.upgrade()) {
                c(&e, &p);
            }
        });
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
        entry.grab_focus();
        entry.select_region(0, -1);
    }

    /// Selects the entries `from..to` and opens their context menu (for
    /// screenshots).
    pub(crate) fn demo_menu(&self, from: u32, to: u32) -> Option<gtk::PopoverMenu> {
        self.selection.select_range(from, to - from, true);
        let target = self.target()?;
        let y = 36.0 * f64::from(from) + 18.0;
        actions::popup(&self.list, target, Some((140.0, y)))
    }

    /// Opens the more menu (for tests and screenshots).
    pub(crate) fn demo_more_menu(&self) -> Option<gtk::Popover> {
        self.more.popup();
        self.more.popover()
    }

    /// Selects every entry.
    pub(crate) fn select_all(&self) {
        self.selection.select_all();
    }

    /// The list, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.list.clone().upcast()
    }

    /// The titles of the entries (for tests).
    pub(crate) fn titles(&self) -> Vec<String> {
        self.rows.borrow().iter().map(|r| r.title.clone()).collect()
    }
}
