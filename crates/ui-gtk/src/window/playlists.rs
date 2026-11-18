// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the window does with playlists beyond showing them: folders, smart
//! playlists, pins, the lock and grouping, tidying, undo and redo, and the
//! menus of playlists, folders and pins.
//!
//! The actions take the playlist's number as their parameter, so the same
//! menu serves the sidebar and the playlist pages:
//! `win.playlist-play`, `-rename`, `-export`, `-delete`, `-duplicate`,
//! `-pin`, `-lock`, `-group`, `-dedupe`, `-remove-missing`, `-shuffle`,
//! `-edit-rules` (a number); `win.playlist-move` ("playlist/folder", folder 0
//! for the top); `win.new-playlist-folder` and `win.new-smart-playlist-in` (the
//! parent folder, 0 for the top); `win.new-smart-playlist` (query text);
//! `win.pin-album`, `win.pin-artist` (a number); `win.unpin` (a pin's row
//! name); and `win.redo`. Every change of a playlist shows a toast with Undo.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_service::library::parse_query;

use super::{MainWindow, Page};
use crate::backend::{
    AlbumKey, ArtistKey, BackendError, LibrarySource, PinKey, PlaylistKey, PlaylistRow,
    PlaylistType, Query,
};
use crate::i18n::{fill, ntr, tr};

/// The rules a new smart playlist starts with: what came in lately.
const NEW_RULES: &str = "added<1m sort:added desc";

impl MainWindow {
    /// The playlist or folder `key`, as last listed.
    pub(crate) fn playlist_row(&self, key: PlaylistKey) -> Option<PlaylistRow> {
        self.playlist_rows().into_iter().find(|p| p.key == key)
    }

    /// Whether `key` is the built-in Favourites playlist.
    pub(crate) fn is_builtin_playlist(&self, key: PlaylistKey) -> bool {
        self.playlist_row(key).is_some_and(|p| p.builtin)
    }

    /// Whether `key` is pinned to the sidebar.
    pub(crate) fn is_pinned(&self, key: PinKey) -> bool {
        self.pages()
            .is_some_and(|p| p.pins.borrow().iter().any(|pin| pin.key == key))
    }

    /// Runs `job` on the playlist `key`; then shows `done` (from what the job
    /// gave) in a toast with Undo, which undoes the playlist's last edit.
    pub(crate) fn change_playlist<T: Send + 'static>(
        &self,
        key: PlaylistKey,
        job: impl FnOnce(&dyn LibrarySource) -> Result<T, BackendError> + Send + 'static,
        done: impl FnOnce(&T) -> Option<String> + 'static,
    ) {
        let Some(backend) = self.backend() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = backend.query(job).await;
            let Some(window) = weak.upgrade() else { return };
            window.playlists_changed(Some(key));
            match result {
                Ok(value) => {
                    if let Some(text) = done(&value) {
                        window.toast_playlist_undo(key, &text);
                    }
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// A toast saying `text`, whose Undo undoes the last edit of `key`.
    pub(crate) fn toast_playlist_undo(&self, key: PlaylistKey, text: &str) {
        let weak = self.downgrade();
        self.toast_undo(text, move || {
            if let Some(window) = weak.upgrade() {
                window.step_playlist(key, false);
            }
        });
    }

    /// Undoes (or, with `redo`, does again) the last edit of `key`.
    pub(crate) fn step_playlist(&self, key: PlaylistKey, redo: bool) {
        let Some(backend) = self.backend() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| {
                    if redo {
                        l.redo_playlist(key)
                    } else {
                        l.undo_playlist(key)
                    }
                })
                .await;
            let Some(window) = weak.upgrade() else { return };
            match result {
                Ok(true) => window.playlists_changed(Some(key)),
                Ok(false) => window.toast(
                    &if redo {
                        tr("There is nothing to redo.")
                    } else {
                        tr("There is nothing to undo.")
                    },
                    false,
                ),
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// The playlist open in the content area, if one is.
    pub(crate) fn open_playlist_key(&self) -> Option<PlaylistKey> {
        match self.current_page() {
            Some(Page::Playlist(key)) => Some(key),
            _ => None,
        }
    }

    /// Makes a smart playlist of `query` in `parent`, opens it and edits its
    /// rules.
    pub(crate) fn new_smart_playlist(&self, query: Query, parent: Option<PlaylistKey>) {
        let Some(backend) = self.backend() else {
            return;
        };
        let taken: Vec<String> = self.playlist_rows().into_iter().map(|p| p.name).collect();
        let name = unused_name(&tr("New smart playlist"), &taken);
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let created = backend
                .query(move |l| l.create_smart_playlist(&name, &query, parent))
                .await;
            let Some(window) = weak.upgrade() else { return };
            match created {
                Ok(key) => {
                    window.playlists_changed(None);
                    window.open_smart_playlist(key, true);
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// Makes a folder in `parent` and asks for its name.
    pub(crate) fn new_playlist_folder(&self, parent: Option<PlaylistKey>) {
        let Some(backend) = self.backend() else {
            return;
        };
        let taken: Vec<String> = self.playlist_rows().into_iter().map(|p| p.name).collect();
        let name = unused_name(&tr("New folder"), &taken);
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let created = backend
                .query(move |l| l.create_playlist_folder(&name, parent))
                .await;
            let Some(window) = weak.upgrade() else { return };
            match created {
                Ok(key) => window.reload_playlists_then(move |window| window.ask_folder_name(key)),
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// Asks for a new name of the folder `key` beside its sidebar row.
    pub(crate) fn ask_folder_name(&self, key: PlaylistKey) {
        let Some(row) = self
            .imp_sidebar()
            .and_then(|s| s.row(&format!("folder:{}", key.0)))
        else {
            return;
        };
        let current = self.playlist_row(key).map(|p| p.name).unwrap_or_default();
        let weak = self.downgrade();
        ask_name(
            row.upcast_ref(),
            &current,
            &tr("Folder name"),
            move |name| {
                if let Some(window) = weak.upgrade() {
                    window.rename_quietly(key, name);
                }
            },
        );
    }

    /// Renames `key` without a page to show it on.
    pub(crate) fn rename_quietly(&self, key: PlaylistKey, name: String) {
        self.change_playlist(key, move |l| l.rename_playlist(key, &name), |_| None);
    }

    /// The menu of a playlist or folder, for the sidebar and its page.
    pub(crate) fn playlist_menu_model(&self, key: PlaylistKey) -> gio::Menu {
        let rows = self.playlist_rows();
        let kind = rows
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.kind)
            .unwrap_or_default();
        // Favourites is never renamed, moved or deleted.
        let builtin = rows.iter().any(|p| p.key == key && p.builtin);
        let id = key.0;
        let item = |label: &str, action: &str| {
            let item = gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(Some(action), Some(&id.to_variant()));
            item
        };
        let menu = gio::Menu::new();
        let first = gio::Menu::new();
        match kind {
            PlaylistType::Folder => {
                let new = gio::MenuItem::new(Some(&tr("New smart playlist here")), None);
                new.set_action_and_target_value(
                    Some("win.new-smart-playlist-in"),
                    Some(&id.to_variant()),
                );
                first.append_item(&new);
                first.append_item(&item(&tr("New folder here"), "win.new-playlist-folder"));
            }
            PlaylistType::Smart => {
                first.append_item(&item(&tr("Play"), "win.playlist-play"));
                first.append_item(&item(&tr("Edit rules"), "win.playlist-edit-rules"));
            }
            PlaylistType::Static => {
                first.append_item(&item(&tr("Play"), "win.playlist-play"));
            }
        }
        menu.append_section(None, &first);
        let edit = gio::Menu::new();
        let mut rename = item(&tr("Rename…"), "win.playlist-rename");
        if kind == PlaylistType::Static {
            rename = item(&tr("Rename…"), "win.playlist-rename");
            rename.set_attribute_value("accel", Some(&"F2".to_variant()));
        }
        if !builtin {
            edit.append_item(&rename);
        }
        match kind {
            PlaylistType::Static => {
                edit.append_item(&item(&tr("Duplicate"), "win.playlist-duplicate"));
            }
            PlaylistType::Smart => {
                edit.append_item(&item(
                    &tr("Save a copy as a playlist"),
                    "win.playlist-duplicate",
                ));
            }
            PlaylistType::Folder => {}
        }
        if !builtin {
            edit.append_submenu(Some(&tr("Move to folder")), &self.move_menu(key, &rows));
        }
        let pinned = self.is_pinned(PinKey::Playlist(key));
        edit.append_item(&item(
            &if pinned {
                tr("Unpin")
            } else {
                tr("Pin to the sidebar")
            },
            "win.playlist-pin",
        ));
        menu.append_section(None, &edit);
        let last = gio::Menu::new();
        if kind == PlaylistType::Static {
            last.append_item(&item(&tr("Export…"), "win.playlist-export"));
        }
        if !builtin {
            last.append_item(&item(
                &if kind == PlaylistType::Folder {
                    tr("Delete folder…")
                } else {
                    tr("Delete playlist…")
                },
                "win.playlist-delete",
            ));
        }
        menu.append_section(None, &last);
        menu
    }

    /// "Move to folder ▸": the top and every folder `key` can go into.
    fn move_menu(&self, key: PlaylistKey, rows: &[PlaylistRow]) -> gio::Menu {
        let menu = gio::Menu::new();
        let parent = rows.iter().find(|p| p.key == key).and_then(|p| p.parent);
        let target = |folder: u64| format!("{}/{folder}", key.0);
        if parent.is_some() {
            let top = gio::MenuItem::new(Some(&tr("Top level")), None);
            top.set_action_and_target_value(
                Some("win.playlist-move"),
                Some(&target(0).to_variant()),
            );
            menu.append_item(&top);
        }
        for folder in rows.iter().filter(|p| p.kind == PlaylistType::Folder) {
            let inside = {
                let mut at = Some(folder.key);
                let mut found = false;
                while let Some(f) = at {
                    if f == key {
                        found = true;
                        break;
                    }
                    at = rows.iter().find(|p| p.key == f).and_then(|p| p.parent);
                }
                found
            };
            if inside || Some(folder.key) == parent {
                continue;
            }
            let entry = gio::MenuItem::new(Some(&folder.name), None);
            entry.set_action_and_target_value(
                Some("win.playlist-move"),
                Some(&target(folder.key.0).to_variant()),
            );
            menu.append_item(&entry);
        }
        let new = gio::MenuItem::new(Some(&tr("New folder…")), None);
        new.set_action_and_target_value(
            Some("win.playlist-move"),
            Some(&format!("{}/new", key.0).to_variant()),
        );
        menu.append_item(&new);
        menu
    }

    /// Shows the menu of a playlist or folder at `widget` (a sidebar row).
    pub(crate) fn playlist_menu(&self, widget: &gtk::Widget, key: PlaylistKey) {
        popup_menu(widget, &self.playlist_menu_model(key));
    }

    /// Shows the menu of a pinned row.
    pub(crate) fn pin_menu(&self, widget: &gtk::Widget, key: PinKey) {
        let menu = gio::Menu::new();
        let open = gio::MenuItem::new(Some(&tr("Open")), None);
        open.set_action_and_target_value(Some("win.open-pin"), Some(&key.row_name().to_variant()));
        menu.append_item(&open);
        let unpin = gio::MenuItem::new(Some(&tr("Unpin")), None);
        unpin.set_action_and_target_value(Some("win.unpin"), Some(&key.row_name().to_variant()));
        menu.append_item(&unpin);
        popup_menu(widget, &menu);
    }

    /// Opens what a pinned row stands for.
    pub(crate) fn open_pin(&self, key: PinKey) {
        match key {
            PinKey::Album(album) => self.open_album(album),
            PinKey::Artist(artist) => self.open_artist(artist),
            PinKey::Playlist(playlist) => self.go(Page::Playlist(playlist), true),
        }
    }

    /// Pins or unpins `key`, with Undo.
    pub(crate) fn set_pinned(&self, key: PinKey, on: bool) {
        let Some(backend) = self.backend() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| if on { l.pin(key) } else { l.unpin(key) })
                .await;
            let Some(window) = weak.upgrade() else { return };
            match result {
                Ok(()) => {
                    window.load_pins();
                    let text = if on {
                        tr("Pinned to the sidebar")
                    } else {
                        tr("Unpinned")
                    };
                    let undo = window.downgrade();
                    window.toast_undo(&text, move || {
                        if let Some(window) = undo.upgrade() {
                            window.set_pinned(key, !on);
                        }
                    });
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// Lists the pins in the sidebar again.
    pub(crate) fn load_pins(&self) {
        let Some(pages) = self.pages() else { return };
        let backend = pages.backend.clone();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let pins = backend.query(|l| l.pins()).await.unwrap_or_default();
            let Some(window) = weak.upgrade() else { return };
            let Some(pages) = window.pages() else { return };
            pages.pins.replace(pins.clone());
            if let Some(sidebar) = window.imp_sidebar() {
                sidebar.set_pins(&pins, &pages.covers);
                let weak = window.downgrade();
                sidebar.connect_pin_menu(move |widget, key| {
                    if let Some(window) = weak.upgrade() {
                        window.pin_menu(widget, key);
                    }
                });
            }
        });
    }

    /// The menu of the "New playlist" row: a playlist, a smart playlist or a
    /// folder.
    pub(crate) fn new_menu(&self, widget: &gtk::Widget) {
        let menu = gio::Menu::new();
        menu.append(Some(&tr("Playlist")), Some("app.new-playlist"));
        let smart = gio::MenuItem::new(Some(&tr("Smart playlist")), None);
        smart.set_action_and_target_value(
            Some("win.new-smart-playlist-in"),
            Some(&0u64.to_variant()),
        );
        menu.append_item(&smart);
        let folder = gio::MenuItem::new(Some(&tr("Folder")), None);
        folder
            .set_action_and_target_value(Some("win.new-playlist-folder"), Some(&0u64.to_variant()));
        menu.append_item(&folder);
        popup_menu(widget, &menu);
    }

    /// Moves the playlist `key` into `folder` (the top for `None`), with Undo.
    pub(crate) fn move_playlist_to(&self, key: PlaylistKey, folder: Option<PlaylistKey>) {
        if self.is_builtin_playlist(key) {
            return;
        }
        let before = self.playlist_row(key).and_then(|p| p.parent);
        let Some(backend) = self.backend() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| l.move_playlist(key, folder, None))
                .await;
            let Some(window) = weak.upgrade() else { return };
            window.playlists_changed(None);
            match result {
                Ok(()) => {
                    let name = folder
                        .and_then(|f| window.playlist_row(f))
                        .map_or_else(|| tr("the top level"), |f| f.name);
                    let undo = window.downgrade();
                    window.toast_undo(&fill(&tr("Moved to {}"), &[&name]), move || {
                        if let Some(window) = undo.upgrade() {
                            window.move_playlist_to(key, before);
                        }
                    });
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// Sets the lock or the grouping of `key` (`lock` picks which), with Undo.
    pub(crate) fn set_playlist_flag(&self, key: PlaylistKey, lock: bool, on: bool) {
        if lock && on && self.is_builtin_playlist(key) {
            return;
        }
        let Some(backend) = self.backend() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| {
                    if lock {
                        l.set_playlist_locked(key, on)
                    } else {
                        l.set_playlist_grouped(key, on)
                    }
                })
                .await;
            let Some(window) = weak.upgrade() else { return };
            window.playlists_changed(Some(key));
            match result {
                Ok(()) => {
                    let text = match (lock, on) {
                        (true, true) => tr("Locked against changes"),
                        (true, false) => tr("Unlocked"),
                        (false, true) => tr("Grouped by album"),
                        (false, false) => tr("Not grouped"),
                    };
                    let undo = window.downgrade();
                    window.toast_undo(&text, move || {
                        if let Some(window) = undo.upgrade() {
                            window.set_playlist_flag(key, lock, !on);
                        }
                    });
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// Lists the playlists again, then runs `then`.
    fn reload_playlists_then(&self, then: impl FnOnce(&MainWindow) + 'static) {
        let Some(backend) = self.backend() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let _ = backend.query(|l| l.playlists()).await;
            let Some(window) = weak.upgrade() else { return };
            window.playlists_changed(None);
            // The sidebar is filled once the list is back.
            let weak = window.downgrade();
            glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || {
                if let Some(window) = weak.upgrade() {
                    then(&window);
                }
            });
        });
    }

    /// Installs the playlist actions of the window.
    pub(super) fn install_playlist_actions(&self) {
        let number = |name: &str, run: fn(&MainWindow, PlaylistKey)| {
            gio::ActionEntry::builder(name)
                .parameter_type(Some(glib::VariantTy::UINT64))
                .activate(move |window: &MainWindow, _, value| {
                    if let Some(key) = value.and_then(u64::from_variant) {
                        run(window, PlaylistKey(key));
                    }
                })
                .build()
        };
        let text = |name: &str, run: fn(&MainWindow, &str)| {
            gio::ActionEntry::builder(name)
                .parameter_type(Some(glib::VariantTy::STRING))
                .activate(move |window: &MainWindow, _, value| {
                    if let Some(text) = value.and_then(|v| v.str().map(str::to_owned)) {
                        run(window, &text);
                    }
                })
                .build()
        };
        let entries = [
            number("playlist-play", |w, k| w.play_playlist(k)),
            number("playlist-rename", |w, k| w.rename_playlist(k)),
            number("playlist-export", |w, k| w.export_playlist(k)),
            number("playlist-delete", |w, k| w.delete_playlist(k)),
            number("playlist-edit-rules", |w, k| w.open_smart_playlist(k, true)),
            number("playlist-duplicate", |w, k| {
                let name = w.playlist_row(k).map(|p| p.name).unwrap_or_default();
                let copy = fill(&tr("{} (copy)"), &[&name]);
                let Some(backend) = w.backend() else { return };
                let weak = w.downgrade();
                glib::spawn_future_local(async move {
                    let result = backend.query(move |l| l.duplicate_playlist(k, &copy)).await;
                    let Some(window) = weak.upgrade() else { return };
                    match result {
                        Ok(copy) => {
                            window.playlists_changed(None);
                            window.go(Page::Playlist(copy), true);
                        }
                        Err(error) => window.toast(&error.0, true),
                    }
                });
            }),
            number("playlist-pin", |w, k| {
                let on = !w.is_pinned(PinKey::Playlist(k));
                w.set_pinned(PinKey::Playlist(k), on);
            }),
            number("playlist-lock", |w, k| {
                let on = !w.playlist_row(k).is_some_and(|p| p.locked);
                w.set_playlist_flag(k, true, on);
            }),
            number("playlist-group", |w, k| {
                let on = !w.playlist_row(k).is_some_and(|p| p.grouped);
                w.set_playlist_flag(k, false, on);
            }),
            number("playlist-dedupe", |w, k| {
                w.change_playlist(
                    k,
                    move |l| l.remove_playlist_duplicates(k),
                    |n| {
                        Some(fill(
                            &ntr("{} duplicate removed", "{} duplicates removed", *n),
                            &[&n.to_string()],
                        ))
                    },
                );
            }),
            number("playlist-remove-missing", |w, k| {
                w.change_playlist(
                    k,
                    move |l| l.remove_missing_from_playlist(k),
                    |n| {
                        Some(fill(
                            &ntr("{} missing track removed", "{} missing tracks removed", *n),
                            &[&n.to_string()],
                        ))
                    },
                );
            }),
            number("playlist-shuffle", |w, k| {
                w.change_playlist(
                    k,
                    move |l| l.shuffle_playlist(k),
                    |()| Some(tr("The order is shuffled")),
                );
            }),
            number("new-playlist-folder", |w, k| {
                w.new_playlist_folder((k.0 != 0).then_some(k));
            }),
            number("new-smart-playlist-in", |w, k| {
                let query = parse_query(NEW_RULES).unwrap_or_default();
                w.new_smart_playlist(query, (k.0 != 0).then_some(k));
            }),
            number("pin-album", |w, k| {
                let key = PinKey::Album(AlbumKey(k.0));
                w.set_pinned(key, !w.is_pinned(key));
            }),
            number("pin-artist", |w, k| {
                let key = PinKey::Artist(ArtistKey(k.0));
                w.set_pinned(key, !w.is_pinned(key));
            }),
            text("new-smart-playlist", |w, text| match parse_query(text) {
                Ok(query) => w.new_smart_playlist(query.normalized(), None),
                Err(error) => w.toast(
                    &fill(&tr("The text cannot be read: {}"), &[&error.to_string()]),
                    true,
                ),
            }),
            text("playlist-move", |w, target| {
                let Some((key, folder)) = target.split_once('/') else {
                    return;
                };
                let Ok(key) = key.parse::<u64>() else { return };
                let key = PlaylistKey(key);
                if folder == "new" {
                    w.move_into_new_folder(key);
                } else if let Ok(folder) = folder.parse::<u64>() {
                    w.move_playlist_to(key, (folder != 0).then_some(PlaylistKey(folder)));
                }
            }),
            text("unpin", |w, name| {
                if let Some(key) = PinKey::from_row_name(name) {
                    w.set_pinned(key, false);
                }
            }),
            text("open-pin", |w, name| {
                if let Some(key) = PinKey::from_row_name(name) {
                    w.open_pin(key);
                }
            }),
        ];
        let redo = gio::ActionEntry::builder("redo")
            .activate(|window: &MainWindow, _, _| {
                if let Some(key) = window.open_playlist_key() {
                    window.step_playlist(key, true);
                } else {
                    window.step_undo_history(true);
                }
            })
            .build();
        self.add_action_entries(entries);
        self.add_action_entries([redo]);
    }

    /// Makes a folder at the top and moves `key` into it.
    fn move_into_new_folder(&self, key: PlaylistKey) {
        let Some(backend) = self.backend() else {
            return;
        };
        let taken: Vec<String> = self.playlist_rows().into_iter().map(|p| p.name).collect();
        let name = unused_name(&tr("New folder"), &taken);
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| {
                    let folder = l.create_playlist_folder(&name, None)?;
                    l.move_playlist(key, Some(folder), None)?;
                    Ok(folder)
                })
                .await;
            let Some(window) = weak.upgrade() else { return };
            match result {
                Ok(folder) => {
                    window.reload_playlists_then(move |window| window.ask_folder_name(folder));
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }
}

/// `base`, or `base 2`, `base 3`, … whichever is not taken.
pub(crate) fn unused_name(base: &str, taken: &[String]) -> String {
    (1..)
        .map(|n| {
            if n == 1 {
                base.to_owned()
            } else {
                format!("{base} {n}")
            }
        })
        .find(|n| !taken.contains(n))
        .unwrap_or_else(|| base.to_owned())
}

/// Shows `model` as a menu at `widget`.
fn popup_menu(widget: &gtk::Widget, model: &gio::Menu) {
    let popover = gtk::PopoverMenu::from_model(Some(model));
    popover.set_parent(widget);
    popover.set_has_arrow(false);
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    crate::views::actions::popup_when_built(&popover);
}

/// Asks for a name in a popover under `anchor`, starting from `current`, and
/// calls `done` with a new one.
pub(crate) fn ask_name(
    anchor: &gtk::Widget,
    current: &str,
    what: &str,
    done: impl Fn(String) + 'static,
) {
    let popover = gtk::Popover::new();
    popover.add_css_class("hk-rename");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let entry = gtk::Entry::new();
    entry.add_css_class("hk-input");
    entry.set_width_chars(28);
    entry.set_text(current);
    entry.update_property(&[gtk::accessible::Property::Label(what)]);
    line.append(&entry);
    let save = gtk::Button::with_label(&tr("Rename"));
    save.add_css_class("hk-btn");
    save.add_css_class("primary");
    save.add_css_class("compact");
    line.append(&save);
    popover.set_child(Some(&line));
    popover.set_parent(anchor);
    popover.set_position(gtk::PositionType::Bottom);
    let current = current.to_owned();
    let commit = Rc::new(move |entry: &gtk::Entry, popover: &gtk::Popover| {
        let name = entry.text().trim().to_owned();
        popover.popdown();
        if !name.is_empty() && name != current {
            done(name);
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_names_count_up() {
        let taken = vec!["New folder".to_owned(), "New folder 2".to_owned()];
        assert_eq!(unused_name("New folder", &taken), "New folder 3");
        assert_eq!(unused_name("Other", &taken), "Other");
    }
}
