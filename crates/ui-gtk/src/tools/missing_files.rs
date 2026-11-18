// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Missing files: the tracks whose file is gone, by the library folder they
//! were in; a filter, "Look in a folder…" and "Look again"; the table of
//! tracks with how long they have been missing and where their file was found;
//! and the footer with "Remove N from the library" and "Relink N tracks".
//!
//! A file is recognised by its size and content, so a renamed file is found
//! too, and relinking keeps ratings, play counts and places in playlists.
//! Tracks on a drive that is not connected are hidden, not missing, and are
//! not listed. Removing shows an Undo toast; the tracks leave the library when
//! the toast has gone.
//!
//! A single click selects a row, Space ticks it, and a double click or Enter
//! plays its track.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::file_tools::{choose_folder, count_row, run_in_background};
use super::tool_window::{CHECK, Frame, GAP, footer, frame, primary_button, table_header};
use crate::backend::services::{
    CancelToken, FileId, FolderState, FoundFile, MissingFile, Services,
};
use crate::backend::{Backend, PlayerRequest, TrackKey};
use crate::dialogs::common::{compact_button, find_open, tilde};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "missing-files";

/// The width of the Missing column.
const DAYS: i32 = 62;
/// The width of the Found column.
const FOUND: i32 = 330;
/// How long removed tracks wait for an Undo before they leave the library.
const REMOVE_AFTER: Duration = Duration::from_secs(12);

struct Missing {
    window: gtk::Window,
    backend: Backend,
    files: RefCell<Vec<MissingFile>>,
    found: RefCell<HashMap<FileId, PathBuf>>,
    ticked: RefCell<HashSet<FileId>>,
    /// Removed, waiting for the toast to go.
    pending: RefCell<HashSet<FileId>>,
    roots: RefCell<Vec<PathBuf>>,
    root: RefCell<Option<PathBuf>>,
    folder: RefCell<Option<PathBuf>>,
    shown: RefCell<Vec<usize>>,
    /// The files listed now, without those waiting to be removed.
    visible_cache: RefCell<Vec<MissingFile>>,
    /// The library folders of the left pane, after "All missing files".
    side_roots: RefCell<Vec<PathBuf>>,
    grace: Cell<u32>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    list: gtk::ListBox,
    filter: gtk::SearchEntry,
    title: gtk::Label,
    header_summary: gtk::Label,
    side_note: gtk::Label,
    found_head: gtk::Label,
    summary: gtk::Label,
    remove: gtk::Button,
    relink: gtk::Button,
    look: gtk::Button,
    again: gtk::Button,
    empty: gtk::Label,
    stack: gtk::Stack,
    filling: Cell<bool>,
    looking: Cell<bool>,
}

fn days_since(seconds: i64) -> u64 {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    now.saturating_sub(u64::try_from(seconds).unwrap_or(0)) / 86_400
}

impl Missing {
    fn services(&self) -> Arc<dyn Services> {
        self.backend.services()
    }

    fn visible(&self) -> Vec<MissingFile> {
        let pending = self.pending.borrow();
        self.files
            .borrow()
            .iter()
            .filter(|f| !pending.contains(&f.file))
            .cloned()
            .collect()
    }

    /// Loads the missing files and the library folders, then looks for them.
    fn load(self: &Rc<Self>, then_look: bool) {
        let services = self.services();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (sender, receiver) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = sender.send_blocking((
                    services.missing_files(),
                    services.library_folders(),
                    services.settings().library.missing_grace_days,
                ));
            });
            let Ok((files, folders, grace)) = receiver.recv().await else {
                return;
            };
            let Some(this) = weak.upgrade() else { return };
            this.grace.set(grace);
            let folders = folders.unwrap_or_default();
            this.roots
                .replace(folders.iter().map(|f| f.path.clone()).collect());
            let hidden: Vec<String> = folders
                .iter()
                .filter(|f| f.state == FolderState::NotConnected)
                .map(|f| {
                    let name = f
                        .path
                        .file_name()
                        .map_or_else(|| tilde(&f.path), |n| n.to_string_lossy().into_owned());
                    let tracks = f.tracks.unwrap_or(0);
                    fill(
                        &ntr(
                            "{} is not connected. Its {} track is hidden, not missing.",
                            "{} is not connected. Its {} tracks are hidden, not missing.",
                            tracks,
                        ),
                        &[&name, &format::count(tracks)],
                    )
                })
                .collect();
            this.side_note.set_text(&if hidden.is_empty() {
                tr("Tracks on a drive that is not connected are hidden, not missing.")
            } else {
                hidden.join(" ")
            });
            match files {
                Ok(files) => {
                    this.files.replace(files);
                }
                Err(error) => this.summary.set_text(&error.0),
            }
            this.fill_side();
            this.rebuild();
            if then_look {
                this.look();
            }
        });
    }

    fn fill_side(&self) {
        self.filling.set(true);
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let files = self.visible();
        let (all, count) = count_row(&tr("All missing files"));
        count.set_text(&format::count(files.len()));
        self.list.append(&all);
        let mut roots: Vec<PathBuf> = files.iter().map(|f| f.root.clone()).collect();
        roots.sort();
        roots.dedup();
        let mut chosen = 0;
        for (i, root) in roots.iter().enumerate() {
            let (row, count) = count_row(&tilde(root));
            count.set_text(&format::count(
                files.iter().filter(|f| &f.root == root).count(),
            ));
            self.list.append(&row);
            if self.root.borrow().as_ref() == Some(root) {
                chosen = i + 1;
            }
        }
        if chosen == 0 {
            self.root.replace(None);
        }
        if let Some(row) = self.list.row_at_index(i32::try_from(chosen).unwrap_or(0)) {
            self.list.select_row(Some(&row));
        }
        self.filling.set(false);
        self.side_roots.replace(roots);
    }

    fn rebuild(&self) {
        let files = self.visible();
        let text = self.filter.text().trim().to_lowercase();
        let root = self.root.borrow().clone();
        let shown: Vec<usize> = files
            .iter()
            .enumerate()
            .filter(|(_, f)| root.as_ref().is_none_or(|r| &f.root == r))
            .filter(|(_, f)| {
                text.is_empty()
                    || [f.title.as_deref(), f.artist.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|t| t.to_lowercase().contains(&text))
                    || f.path.to_string_lossy().to_lowercase().contains(&text)
            })
            .map(|(i, _)| i)
            .collect();
        let count = shown.len();
        self.shown.replace(shown);
        self.visible_cache.replace(files);
        let items: Vec<glib::BoxedAnyObject> = (0..count).map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &items);
        self.title.set_text(
            &root
                .as_ref()
                .map_or_else(|| tr("All missing files"), |r| tilde(r)),
        );
        let grace = self.grace.get() as usize;
        self.header_summary.set_text(&fill(
            &tr("{} · they leave the library {} after they go missing"),
            &[
                &fill(
                    &ntr("{} track", "{} tracks", count),
                    &[&format::count(count)],
                ),
                &fill(&ntr("{} day", "{} days", grace), &[&format::count(grace)]),
            ],
        ));
        self.stack
            .set_visible_child_name(if count == 0 { "empty" } else { "table" });
        self.empty.set_text(&if self.files.borrow().is_empty() {
            tr("No file is missing.")
        } else {
            tr("No missing track matches the filter.")
        });
        self.update_found_head();
        self.update_footer();
    }

    fn update_found_head(&self) {
        let text = match self.folder.borrow().as_ref() {
            Some(folder) => fill(&tr("Found in {}"), &[&tilde(folder)]),
            None => tr("Found in the library folders"),
        };
        self.found_head.set_text(&text.to_uppercase());
    }

    fn update_footer(&self) {
        let files = self.visible_cache.borrow();
        let found = self.found.borrow();
        let ticked = self.ticked.borrow();
        let found_count = files.iter().filter(|f| found.contains_key(&f.file)).count();
        let not_found = files.len() - found_count;
        let relink = files
            .iter()
            .filter(|f| found.contains_key(&f.file) && ticked.contains(&f.file))
            .count();
        let place = match self.folder.borrow().as_ref() {
            Some(folder) => fill(&tr("found in {}"), &[&tilde(folder)]),
            None => tr("found in the library folders"),
        };
        self.summary.set_text(&if self.looking.get() {
            tr("Looking for the files…")
        } else {
            fill(
                &tr("{} {}, matched by size and content · {}"),
                &[
                    &format::count(found_count),
                    &place,
                    &fill(
                        &ntr("{} not found", "{} not found", not_found),
                        &[&format::count(not_found)],
                    ),
                ],
            )
        });
        self.remove.set_label(&fill(
            &ntr(
                "Remove {} from the library",
                "Remove {} from the library",
                not_found,
            ),
            &[&format::count(not_found)],
        ));
        self.remove
            .set_sensitive(not_found > 0 && !self.looking.get());
        self.relink.set_label(&fill(
            &ntr("Relink {} track", "Relink {} tracks", relink),
            &[&format::count(relink)],
        ));
        self.relink.set_sensitive(relink > 0 && !self.looking.get());
        self.look.set_sensitive(!self.looking.get());
        self.again.set_sensitive(!self.looking.get());
    }

    /// Looks for the listed files in the chosen folder (or the library folders).
    fn look(self: &Rc<Self>) {
        let files: Vec<FileId> = self.visible().iter().map(|f| f.file).collect();
        if files.is_empty() {
            return;
        }
        self.looking.set(true);
        self.update_footer();
        let services = self.services();
        let folder = self.folder.borrow().clone();
        let weak = Rc::downgrade(self);
        run_in_background(
            move |_| services.find_missing(&files, folder.as_deref(), &CancelToken::new()),
            |_, _| {},
            move |result| {
                let Some(this) = weak.upgrade() else { return };
                this.looking.set(false);
                match result {
                    Ok(found) => {
                        let map: HashMap<FileId, PathBuf> =
                            found.into_iter().map(|f| (f.file, f.at)).collect();
                        this.ticked.replace(map.keys().copied().collect());
                        this.found.replace(map);
                    }
                    Err(error) => this.summary.set_text(&error.0),
                }
                this.rebuild();
            },
        );
    }

    fn main_window(&self) -> Option<MainWindow> {
        self.window.transient_for().and_downcast::<MainWindow>()
    }

    fn relink(self: &Rc<Self>) {
        let found: Vec<FoundFile> = {
            let ticked = self.ticked.borrow();
            self.found
                .borrow()
                .iter()
                .filter(|(file, _)| ticked.contains(file))
                .map(|(file, at)| FoundFile {
                    file: *file,
                    at: at.clone(),
                })
                .collect()
        };
        if found.is_empty() {
            return;
        }
        let services = self.services();
        let weak = Rc::downgrade(self);
        self.relink.set_sensitive(false);
        run_in_background(
            move |_| services.relink_missing(&found),
            |_, _| {},
            move |result| {
                let Some(this) = weak.upgrade() else { return };
                match result {
                    Ok(outcomes) => {
                        let done = outcomes.iter().filter(|o| o.problem.is_none()).count();
                        if let Some(main) = this.main_window() {
                            main.toast(
                                &fill(
                                    &ntr("{} track relinked", "{} tracks relinked", done),
                                    &[&format::count(done)],
                                ),
                                false,
                            );
                        }
                    }
                    Err(error) => this.summary.set_text(&error.0),
                }
                for file in this.found.borrow().keys() {
                    this.ticked.borrow_mut().remove(file);
                }
                this.load(false);
            },
        );
    }

    /// "Remove N from the library": the tracks not found leave the list at
    /// once, and the library when the Undo toast has gone.
    fn remove(self: &Rc<Self>) {
        let files: Vec<FileId> = {
            let found = self.found.borrow();
            self.visible()
                .iter()
                .filter(|f| !found.contains_key(&f.file))
                .map(|f| f.file)
                .collect()
        };
        if files.is_empty() {
            return;
        }
        self.pending.borrow_mut().extend(files.iter().copied());
        self.fill_side();
        self.rebuild();
        let undone = Rc::new(Cell::new(false));
        let services = self.services();
        let text = fill(
            &ntr(
                "{} track removed from the library",
                "{} tracks removed from the library",
                files.len(),
            ),
            &[&format::count(files.len())],
        );
        if let Some(main) = self.main_window() {
            let weak = Rc::downgrade(self);
            let undone = undone.clone();
            let ids = files.clone();
            main.toast_undo(&text, move || {
                undone.set(true);
                if let Some(this) = weak.upgrade() {
                    for id in &ids {
                        this.pending.borrow_mut().remove(id);
                    }
                    this.fill_side();
                    this.rebuild();
                }
            });
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(REMOVE_AFTER, move || {
            if undone.get() {
                return;
            }
            let weak = weak.clone();
            run_in_background(
                move |_| services.remove_missing(&files),
                |_, _| {},
                move |_| {
                    if let Some(this) = weak.upgrade() {
                        this.pending.borrow_mut().clear();
                        this.load(false);
                    }
                },
            );
        });
    }

    fn play(&self, position: u32) {
        let Some(track) = self.row(position).and_then(|f| f.track) else {
            return;
        };
        self.backend.send(PlayerRequest::PlayTracks {
            tracks: vec![TrackKey(track.get())],
            start: 0,
            title: tr("Missing files"),
            shuffle: false,
        });
    }

    fn row(&self, position: u32) -> Option<MissingFile> {
        let index = *self.shown.borrow().get(position as usize)?;
        self.visible_cache.borrow().get(index).cloned()
    }

    fn toggle_selected(&self) -> bool {
        let position = self.selection.selected();
        let Some(file) = self.row(position) else {
            return false;
        };
        if self.found.borrow().contains_key(&file.file) {
            let on = !self.ticked.borrow().contains(&file.file);
            if on {
                self.ticked.borrow_mut().insert(file.file);
            } else {
                self.ticked.borrow_mut().remove(&file.file);
            }
            self.store.items_changed(position, 1, 1);
            self.update_footer();
        }
        true
    }

    fn bind(&self, row: &gtk::Box, item: &gtk::ListItem, position: u32) {
        let Some(file) = self.row(position) else {
            return;
        };
        let cells = children(row);
        let [check_cell, track, days, found_cell] = cells.as_slice() else {
            return;
        };
        let at = self.found.borrow().get(&file.file).cloned();
        let ticked = at.is_some() && self.ticked.borrow().contains(&file.file);
        if let Some(check) = check_cell.first_child().and_downcast::<gtk::CheckButton>() {
            self.filling.set(true);
            check.set_widget_name(&format!("relink-{}", file.file));
            check.set_sensitive(at.is_some());
            check.set_active(ticked);
            self.filling.set(false);
        }
        let title = file.title.clone().unwrap_or_else(|| {
            file.path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        let artist = file.artist.clone().unwrap_or_default();
        let lines = children(track);
        if let [top, path] = lines.as_slice() {
            let parts = children(top);
            if let [t, a] = parts.as_slice() {
                set_label(t, &title);
                set_label(
                    a,
                    &if artist.is_empty() {
                        String::new()
                    } else {
                        fill(&tr("· {}"), &[&artist])
                    },
                );
            }
            let old = tilde(&file.path);
            set_label(path, &old);
            path.set_tooltip_text(Some(&old));
        }
        let age = days_since(file.missing_since);
        let age_text = fill(
            &ntr("{} day", "{} days", age as usize),
            &[&format::count(age)],
        );
        set_label(days, &age_text);
        let late = age + 7 >= u64::from(self.grace.get());
        if late {
            days.add_css_class("hk-strong");
            days.remove_css_class("hk-t2");
        } else {
            days.remove_css_class("hk-strong");
            days.add_css_class("hk-t2");
        }
        let found_parts = children(found_cell);
        let found_text = match &at {
            Some(at) => match self.folder.borrow().as_ref() {
                Some(folder) => at
                    .strip_prefix(folder)
                    .map_or_else(|_| tilde(at), |p| p.to_string_lossy().into_owned()),
                None => tilde(at),
            },
            None => tr("Not found"),
        };
        if let [icon, words] = found_parts.as_slice() {
            icon.set_visible(at.is_some());
            set_label(words, &found_text);
            words.set_tooltip_text(at.as_ref().map(|a| tilde(a)).as_deref());
            if at.is_some() {
                words.add_css_class("hk-mono");
                words.add_css_class("hk-tool-path");
                words.remove_css_class("hk-t3");
            } else {
                words.remove_css_class("hk-mono");
                words.remove_css_class("hk-tool-path");
                words.add_css_class("hk-t3");
            }
        }
        item.set_accessible_label(&fill(
            &tr("{}, missing for {}, {}"),
            &[&title, &age_text, &found_text],
        ));
    }
}

fn children(widget: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.as_ref().first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        out.push(c);
    }
    out
}

fn set_label(widget: &gtk::Widget, text: &str) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        label.set_text(text);
    }
}

fn row_widget(missing: &std::rc::Weak<Missing>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    row.add_css_class("hk-organize-row");
    let check_cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    check_cell.set_size_request(CHECK - GAP, -1);
    let check = gtk::CheckButton::new();
    check.set_valign(gtk::Align::Center);
    check.update_property(&[gtk::accessible::Property::Label(&tr("Relink this track"))]);
    let weak = missing.clone();
    check.connect_toggled(move |check| {
        let Some(this) = weak.upgrade() else { return };
        if this.filling.get() {
            return;
        }
        let Some(id) = check
            .widget_name()
            .strip_prefix("relink-")
            .and_then(|n| n.parse::<i64>().ok())
            .map(FileId::from_raw)
        else {
            return;
        };
        if check.is_active() {
            this.ticked.borrow_mut().insert(id);
        } else {
            this.ticked.borrow_mut().remove(&id);
        }
        this.update_footer();
    });
    check_cell.append(&check);
    row.append(&check_cell);
    let track = gtk::Box::new(gtk::Orientation::Vertical, 2);
    track.set_hexpand(true);
    track.set_valign(gtk::Align::Center);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let name = label("", &["hk-medium"]);
    let by = label("", &["hk-t3"]);
    top.append(&name);
    top.append(&by);
    track.append(&top);
    let path = label("", &["hk-mono-meta", "hk-t3"]);
    path.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    path.set_max_width_chars(1);
    track.append(&path);
    row.append(&track);
    let days = label("", &["hk-numeric", "hk-t2"]);
    days.set_size_request(DAYS, -1);
    days.set_xalign(1.0);
    row.append(&days);
    let found = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    found.set_size_request(FOUND, -1);
    found.set_hexpand(false);
    found.set_valign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name("helikodu-check-symbolic");
    icon.add_css_class("hk-t2");
    found.append(&icon);
    let words = label("", &["hk-t3"]);
    words.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    words.set_max_width_chars(1);
    words.set_hexpand(true);
    found.append(&words);
    row.append(&found);
    row
}

/// Opens Missing files over `main`, or brings it to the front.
pub(crate) fn open(main: &MainWindow) -> Option<gtk::Window> {
    let parent = main.upcast_ref::<gtk::Window>();
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        return Some(window);
    }
    let backend = main.backend()?;
    let Frame {
        window,
        list,
        side_note,
        title,
        summary: header_summary,
        main: column,
    } = frame(parent, NAME, &tr("Missing files"), &tr("Where they were"));

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    toolbar.add_css_class("hk-toolbar");
    toolbar.add_css_class("hk-tool-toolbar");
    let filter = gtk::SearchEntry::new();
    filter.add_css_class("hk-input");
    filter.set_hexpand(true);
    filter.set_valign(gtk::Align::Center);
    filter.set_placeholder_text(Some(&tr("Filter by title, artist or folder")));
    filter.update_property(&[gtk::accessible::Property::Label(&tr(
        "Filter the missing tracks",
    ))]);
    toolbar.append(&filter);
    let look = gtk::Button::new();
    let look_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    look_content.append(&gtk::Image::from_icon_name("folder-symbolic"));
    look_content.append(&gtk::Label::new(Some(&tr("Look in a folder…"))));
    look.set_child(Some(&look_content));
    look.add_css_class("hk-btn");
    look.add_css_class("compact");
    look.set_valign(gtk::Align::Center);
    look.set_widget_name("missing-look");
    look.update_property(&[gtk::accessible::Property::Label(&tr("Look in a folder…"))]);
    toolbar.append(&look);
    let again = compact_button(&tr("Look again"));
    again.set_widget_name("missing-again");
    toolbar.append(&again);
    column.append(&toolbar);
    let found_head = label("", &["hk-section-label"]);
    found_head.set_size_request(FOUND, -1);
    let head = table_header(&[
        (String::new(), CHECK - GAP, false),
        (tr("Track, and where it was"), -1, false),
        (tr("Missing"), DAYS, true),
    ]);
    head.append(&found_head);
    column.append(&head);

    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let factory = gtk::SignalListItemFactory::new();
    let view = gtk::ListView::new(Some(selection.clone()), Some(factory.clone()));
    view.add_css_class("hk-dup-list");
    view.add_css_class("hk-organize-list");
    view.set_widget_name("missing-table");
    view.update_property(&[gtk::accessible::Property::Label(&tr("Missing tracks"))]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&view));
    let empty = label("", &["hk-t3", "hk-tool-empty"]);
    empty.set_xalign(0.5);
    empty.set_valign(gtk::Align::Start);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&scroller, Some("table"));
    stack.add_named(&empty, Some("empty"));
    column.append(&stack);
    let (bar, summary) = footer();
    let remove = compact_button(&tr("Remove from the library"));
    remove.set_widget_name("missing-remove");
    bar.append(&remove);
    let relink = primary_button(&tr("Relink"));
    relink.set_widget_name("missing-relink");
    bar.append(&relink);
    column.append(&bar);

    let missing = Rc::new(Missing {
        window: window.clone(),
        backend,
        files: RefCell::new(Vec::new()),
        found: RefCell::new(HashMap::new()),
        ticked: RefCell::new(HashSet::new()),
        pending: RefCell::new(HashSet::new()),
        roots: RefCell::new(Vec::new()),
        root: RefCell::new(None),
        folder: RefCell::new(None),
        shown: RefCell::new(Vec::new()),
        visible_cache: RefCell::new(Vec::new()),
        side_roots: RefCell::new(Vec::new()),
        grace: Cell::new(30),
        store,
        selection,
        list: list.clone(),
        filter: filter.clone(),
        title,
        header_summary,
        side_note,
        found_head,
        summary,
        remove: remove.clone(),
        relink: relink.clone(),
        look: look.clone(),
        again: again.clone(),
        empty,
        stack,
        filling: Cell::new(false),
        looking: Cell::new(false),
    });

    let weak = Rc::downgrade(&missing);
    factory.connect_setup(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&row_widget(&weak)));
        }
    });
    let weak = Rc::downgrade(&missing);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(this), Some(row)) = (weak.upgrade(), item.child().and_downcast::<gtk::Box>())
        else {
            return;
        };
        this.bind(&row, item, item.position());
    });
    let weak = Rc::downgrade(&missing);
    list.connect_row_selected(move |_, row| {
        let (Some(this), Some(row)) = (weak.upgrade(), row) else {
            return;
        };
        if this.filling.get() {
            return;
        }
        let index = usize::try_from(row.index()).unwrap_or(0);
        let root = index
            .checked_sub(1)
            .and_then(|i| this.side_roots.borrow().get(i).cloned());
        this.root.replace(root);
        this.rebuild();
    });
    let weak = Rc::downgrade(&missing);
    filter.connect_search_changed(move |_| {
        if let Some(this) = weak.upgrade() {
            this.rebuild();
        }
    });
    let weak = Rc::downgrade(&missing);
    look.connect_clicked(move |_| {
        let Some(this) = weak.upgrade() else { return };
        let weak = Rc::downgrade(&this);
        let start = this.folder.borrow().clone();
        choose_folder(
            &this.window,
            &tr("Look in a folder"),
            &tr("Look here"),
            start.as_deref(),
            move |folder| {
                if let Some(this) = weak.upgrade() {
                    this.folder.replace(Some(folder));
                    this.look();
                }
            },
        );
    });
    let weak = Rc::downgrade(&missing);
    again.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.load(true);
        }
    });
    let weak = Rc::downgrade(&missing);
    relink.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.relink();
        }
    });
    let weak = Rc::downgrade(&missing);
    remove.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.remove();
        }
    });
    let weak = Rc::downgrade(&missing);
    view.connect_activate(move |_, position| {
        if let Some(this) = weak.upgrade() {
            this.play(position);
        }
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&missing);
    keys.connect_key_pressed(move |_, key, _, state| {
        if key == gdk::Key::space
            && state.is_empty()
            && let Some(this) = weak.upgrade()
            && this.toggle_selected()
        {
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    view.add_controller(keys);

    let keep_alive = RefCell::new(Some(missing.clone()));
    window.connect_destroy(move |_| {
        keep_alive.borrow_mut().take();
    });
    missing.update_footer();
    missing.load(true);
    window.present();
    Some(window)
}
