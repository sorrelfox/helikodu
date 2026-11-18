// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The cover manager: which albums have a cover where, and putting pictures that
//! are already on this computer into the files or beside them. Nothing is ever
//! downloaded.
//!
//! The groups at the left with their counts; a filter and the order; the table
//! of albums with their folder and the picture found there; and the footer with
//! the selection, where the pictures are saved and the action for the ticked
//! rows. Each group has its own action: "Use it" for a picture found beside an
//! album without a cover, "Put it into the files", "Save as cover.jpg", and
//! "Remove" (the cover inside the files) in every group.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::tool_window::{GAP, cell, footer, frame, primary_button, shown_path, table_header};
use crate::backend::export::{CoverAction, CoverAlbum, CoverSave, CoverShow, ExportServices};
use crate::backend::{AlbumKey, Backend, ThumbnailState};
use crate::dialogs::common::{compact_button, dropdown, find_open, on_selected};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::{CoverArt, label};
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "cover-manager";

/// The width of the check box column.
const CHECK: i32 = 28;
/// The width of the Album column.
const ALBUM_COLUMN: i32 = 190;
/// The width of the Picture in the folder column.
const PICTURE_COLUMN: i32 = 176;
/// The width of the button column.
const BUTTON_COLUMN: i32 = 124;

/// The groups, in the order of the left pane.
const GROUPS: [CoverShow; 6] = CoverShow::ALL;

/// The places a picture is saved, in the order of the dropdown.
const SAVES: [CoverSave; 3] = [
    CoverSave::FilesAndFolder,
    CoverSave::FilesOnly,
    CoverSave::FolderOnly,
];

fn group_name(group: CoverShow) -> String {
    match group {
        CoverShow::All => tr("All albums"),
        CoverShow::NoCover => tr("No cover"),
        CoverShow::OnlyInFolder => tr("Only in the folder"),
        CoverShow::OnlyInFiles => tr("Only inside the files"),
        CoverShow::Small => tr("Smaller than 500 px"),
        CoverShow::NotSame => tr("Not the same in every file"),
    }
}

fn save_name(save: CoverSave) -> String {
    match save {
        CoverSave::FilesAndFolder => tr("In the files and as cover.jpg"),
        CoverSave::FilesOnly => tr("In the files only"),
        CoverSave::FolderOnly => tr("As cover.jpg only"),
    }
}

/// What the row button and the primary button do in a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    /// Use the picture found in the folder (or choose one).
    UsePicture,
    /// Put the folder's picture into the files.
    IntoFiles,
    /// Save the picture inside the files as a file.
    SaveEmbedded,
}

fn act_for(group: CoverShow, album: &CoverAlbum) -> Act {
    match group {
        CoverShow::OnlyInFolder => Act::IntoFiles,
        CoverShow::OnlyInFiles => Act::SaveEmbedded,
        CoverShow::NotSame => {
            if album.picture.is_some() {
                Act::IntoFiles
            } else {
                Act::UsePicture
            }
        }
        _ if album.is_in(CoverShow::OnlyInFolder) => Act::IntoFiles,
        _ => Act::UsePicture,
    }
}

struct Manager {
    window: gtk::Window,
    backend: Backend,
    export: Arc<dyn ExportServices>,
    group: Cell<CoverShow>,
    albums: RefCell<Vec<CoverAlbum>>,
    shown: RefCell<Vec<usize>>,
    ticked: RefCell<HashSet<AlbumKey>>,
    counts: Vec<(CoverShow, gtk::Label)>,
    title: gtk::Label,
    header_summary: gtk::Label,
    filter: gtk::SearchEntry,
    sort: gtk::DropDown,
    store: gio::ListStore,
    stack: gtk::Stack,
    empty: gtk::Label,
    summary: gtk::Label,
    save: gtk::DropDown,
    save_label: gtk::Label,
    remove: gtk::Button,
    primary: gtk::Button,
    filling: Cell<bool>,
}

impl Manager {
    fn album(&self, index: usize) -> Option<CoverAlbum> {
        self.albums.borrow().get(index).cloned()
    }

    fn load(self: &Rc<Self>) {
        self.side_note_loading();
        let export = self.export.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || export.cover_albums()).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(Ok(albums)) => {
                    this.albums.replace(albums);
                }
                Ok(Err(error)) => this.empty.set_text(&error.0),
                Err(_) => {}
            }
            for (group, label) in &this.counts {
                let n = this
                    .albums
                    .borrow()
                    .iter()
                    .filter(|a| a.is_in(*group))
                    .count();
                label.set_text(&format::count(n));
            }
            let still: HashSet<AlbumKey> = this.albums.borrow().iter().map(|a| a.album).collect();
            this.ticked.borrow_mut().retain(|k| still.contains(k));
            this.rebuild();
        });
    }

    fn side_note_loading(&self) {
        self.summary.set_text(&tr("Looking at the albums…"));
    }

    fn rebuild(&self) {
        let group = self.group.get();
        let text = self.filter.text().trim().to_lowercase();
        let albums = self.albums.borrow();
        let mut shown: Vec<usize> = albums
            .iter()
            .enumerate()
            .filter(|(_, a)| a.is_in(group))
            .filter(|(_, a)| {
                text.is_empty()
                    || a.title.to_lowercase().contains(&text)
                    || a.artist.to_lowercase().contains(&text)
            })
            .map(|(i, _)| i)
            .collect();
        match self.sort.selected() {
            1 => shown.sort_by_key(|&i| albums[i].title.to_lowercase()),
            2 => shown.sort_by_key(|&i| (albums[i].year, albums[i].title.to_lowercase())),
            _ => shown.sort_by_key(|&i| {
                (
                    albums[i].artist.to_lowercase(),
                    albums[i].title.to_lowercase(),
                )
            }),
        }
        drop(albums);
        let items: Vec<glib::BoxedAnyObject> = shown
            .iter()
            .map(|i| glib::BoxedAnyObject::new(*i))
            .collect();
        let n = shown.len();
        self.shown.replace(shown);
        self.store.splice(0, self.store.n_items(), &items);
        self.title.set_text(&group_name(group));
        self.header_summary.set_text(&fill(
            &ntr("{} album", "{} albums", n),
            &[&format::count(n)],
        ));
        self.stack
            .set_visible_child_name(if n == 0 { "empty" } else { "table" });
        if n == 0 {
            self.empty.set_text(&if text.is_empty() {
                tr("No album is in this group.")
            } else {
                tr("No album matches the filter.")
            });
        }
        self.update_footer();
    }

    /// The ticked albums that are shown.
    fn ticked_shown(&self) -> Vec<CoverAlbum> {
        let ticked = self.ticked.borrow();
        let albums = self.albums.borrow();
        self.shown
            .borrow()
            .iter()
            .filter_map(|&i| albums.get(i))
            .filter(|a| ticked.contains(&a.album))
            .cloned()
            .collect()
    }

    fn update_footer(&self) {
        let group = self.group.get();
        let ticked = self.ticked_shown();
        let n = ticked.len();
        let pictures = ticked.iter().filter(|a| a.picture.is_some()).count();
        let mut text = fill(
            &ntr("{} album selected", "{} albums selected", n),
            &[&format::count(n)],
        );
        if group == CoverShow::NoCover || group == CoverShow::All {
            text = fill(
                &tr("{} · {}"),
                &[
                    &text,
                    &fill(
                        &ntr(
                            "{} picture found in their folders",
                            "{} pictures found in their folders",
                            pictures,
                        ),
                        &[&format::count(pictures)],
                    ),
                ],
            );
        }
        self.summary.set_text(&text);
        let (label, count, using) = match group {
            CoverShow::OnlyInFolder => (
                ntr(
                    "Put {} picture into the files",
                    "Put {} pictures into the files",
                    n,
                ),
                n,
                false,
            ),
            CoverShow::OnlyInFiles => (
                ntr(
                    "Save {} picture as cover.jpg",
                    "Save {} pictures as cover.jpg",
                    n,
                ),
                n,
                false,
            ),
            _ => (
                ntr("Use the {} picture", "Use the {} pictures", pictures),
                pictures,
                true,
            ),
        };
        self.primary
            .set_label(&fill(&label, &[&format::count(count)]));
        self.primary.set_sensitive(count > 0);
        self.save.set_visible(using);
        self.save_label.set_visible(using);
        self.remove.set_sensitive(ticked.iter().any(|a| {
            a.is_in(CoverShow::OnlyInFiles)
                || a.is_in(CoverShow::NotSame)
                || a.is_in(CoverShow::Small)
        }));
    }

    fn save_choice(&self) -> CoverSave {
        SAVES
            .get(self.save.selected() as usize)
            .copied()
            .unwrap_or_default()
    }

    /// Does `act` to `albums` one after the other on a worker thread.
    fn run(self: &Rc<Self>, albums: Vec<(AlbumKey, CoverAction)>) {
        if albums.is_empty() {
            return;
        }
        let export = self.export.clone();
        let weak = Rc::downgrade(self);
        self.primary.set_sensitive(false);
        self.summary.set_text(&tr("Saving the pictures…"));
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                let mut failed = 0;
                let mut done = 0;
                for (album, action) in &albums {
                    match export.apply_cover(*album, action) {
                        Ok(results) => {
                            failed += results.iter().filter(|r| r.error.is_some()).count();
                            done += results.iter().filter(|r| r.error.is_none()).count();
                        }
                        Err(_) => failed += 1,
                    }
                }
                (done, failed)
            })
            .await;
            let Some(this) = weak.upgrade() else { return };
            let (done, failed) = result.unwrap_or((0, 1));
            if let Some(main) = this.window.transient_for().and_downcast::<MainWindow>() {
                let message = if failed == 0 {
                    fill(
                        &ntr("{} file changed", "{} files changed", done),
                        &[&format::count(done)],
                    )
                } else {
                    fill(
                        &ntr(
                            "{} file could not be changed",
                            "{} files could not be changed",
                            failed,
                        ),
                        &[&format::count(failed)],
                    )
                };
                main.toast(&message, failed > 0);
            }
            this.ticked.borrow_mut().clear();
            this.load();
        });
    }

    fn action_for(&self, album: &CoverAlbum, act: Act) -> Option<CoverAction> {
        match act {
            Act::UsePicture => album.picture.as_ref().map(|p| CoverAction::Use {
                picture: p.path.clone(),
                save: self.save_choice(),
            }),
            Act::IntoFiles => album.picture.as_ref().map(|p| CoverAction::Use {
                picture: p.path.clone(),
                save: CoverSave::FilesOnly,
            }),
            Act::SaveEmbedded => Some(CoverAction::SaveEmbedded),
        }
    }

    fn apply_ticked(self: &Rc<Self>) {
        let group = self.group.get();
        let work: Vec<(AlbumKey, CoverAction)> = self
            .ticked_shown()
            .iter()
            .filter_map(|a| Some((a.album, self.action_for(a, act_for(group, a))?)))
            .collect();
        self.run(work);
    }

    fn remove_ticked(self: &Rc<Self>) {
        let work = self
            .ticked_shown()
            .iter()
            .map(|a| (a.album, CoverAction::RemoveFromFiles))
            .collect();
        self.run(work);
    }

    /// The row button: does the group's action, or asks for an image.
    fn row_button(self: &Rc<Self>, index: usize) {
        let Some(album) = self.album(index) else {
            return;
        };
        let act = act_for(self.group.get(), &album);
        if matches!(act, Act::UsePicture | Act::IntoFiles) && album.picture.is_none() {
            self.choose_image(album);
            return;
        }
        if let Some(action) = self.action_for(&album, act) {
            self.run(vec![(album.album, action)]);
        }
    }

    fn choose_image(self: &Rc<Self>, album: CoverAlbum) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&tr("Pictures")));
        for suffix in ["jpg", "jpeg", "png"] {
            filter.add_suffix(suffix);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(tr("Choose a picture for the album"))
            .filters(&filters)
            .modal(true)
            .build();
        if let Some(folder) = &album.folder {
            dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
        }
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |file| {
            let (Some(this), Ok(Some(path))) = (weak.upgrade(), file.map(|f| f.path())) else {
                return;
            };
            let save = this.save_choice();
            this.run(vec![(
                album.album,
                CoverAction::Use {
                    picture: path,
                    save,
                },
            )]);
        });
    }

    fn bind(&self, row: &gtk::Box, item: &gtk::ListItem, index: usize) {
        let Some(album) = self.album(index) else {
            return;
        };
        let cells = children(row.upcast_ref());
        let [check_cell, album_cell, folder, picture, button] = cells.as_slice() else {
            return;
        };
        let ticked = self.ticked.borrow().contains(&album.album);
        if ticked {
            row.add_css_class("hk-ticked");
        } else {
            row.remove_css_class("hk-ticked");
        }
        if let Some(check) = check_cell.downcast_ref::<gtk::CheckButton>() {
            self.filling.set(true);
            check.set_widget_name(&format!("album-{}", album.album.0));
            check.set_active(ticked);
            check.update_property(&[gtk::accessible::Property::Label(&fill(
                &tr("Choose {}"),
                &[&album.title],
            ))]);
            self.filling.set(false);
        }
        if let Some(album_box) = album_cell.downcast_ref::<gtk::Box>() {
            let parts = children(album_box.upcast_ref());
            if let [art_stack, text] = parts.as_slice()
                && let Some(stack) = art_stack.downcast_ref::<gtk::Stack>()
            {
                let texture = album.cover.and_then(|cover| {
                    match self.backend.library().thumbnail(&cover, 96) {
                        ThumbnailState::Ready(path) => gdk::Texture::from_filename(path).ok(),
                        _ => None,
                    }
                });
                if album.cover.is_some() {
                    stack.set_visible_child_name("art");
                    if let Some(art) = stack.child_by_name("art").and_downcast::<CoverArt>() {
                        art.show(album.cover_seed, texture);
                    }
                } else {
                    stack.set_visible_child_name("none");
                }
                let lines = children(text);
                if let [title, by] = lines.as_slice() {
                    set_label(title, &album.title);
                    let mut line = album.artist.clone();
                    if let Some(year) = album.year {
                        line = fill(&tr("{} · {}"), &[&line, &year.to_string()]);
                    }
                    set_label(by, &line);
                }
            }
        }
        let folder_text = album.folder.as_deref().map(shown_path).unwrap_or_default();
        set_label(folder, &folder_text);
        folder.set_tooltip_text(Some(&folder_text));
        if let Some(picture_box) = picture.downcast_ref::<gtk::Box>() {
            let lines = children(picture_box.upcast_ref());
            if let [name, facts] = lines.as_slice() {
                match &album.picture {
                    Some(p) => {
                        let shown = p
                            .path
                            .strip_prefix(album.folder.as_deref().unwrap_or(&PathBuf::new()))
                            .map(|r| r.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| shown_path(&p.path));
                        set_label(name, &shown);
                        name.remove_css_class("hk-t3");
                        name.add_css_class("hk-mono");
                        set_label(
                            facts,
                            &fill(
                                &tr("{} × {} · {}"),
                                &[
                                    &p.width.to_string(),
                                    &p.height.to_string(),
                                    &format::size(p.bytes),
                                ],
                            ),
                        );
                        facts.set_visible(true);
                    }
                    None => {
                        set_label(name, &tr("Nothing in the folder"));
                        name.remove_css_class("hk-mono");
                        name.add_css_class("hk-t3");
                        facts.set_visible(false);
                    }
                }
            }
        }
        if let Some(button) = button.downcast_ref::<gtk::Button>() {
            let act = act_for(self.group.get(), &album);
            button.set_label(&match act {
                Act::UsePicture if album.picture.is_some() => tr("Use it"),
                Act::UsePicture => tr("Choose image…"),
                Act::IntoFiles if album.picture.is_some() => tr("Put it into the files"),
                Act::IntoFiles => tr("Choose image…"),
                Act::SaveEmbedded => tr("Save as cover.jpg"),
            });
            button.set_widget_name(&format!("row-{index}"));
        }
        item.set_accessible_label(&fill(
            &tr("{}, {}, {}"),
            &[
                &album.title,
                &album.artist,
                &album
                    .picture
                    .as_ref()
                    .map_or_else(|| tr("nothing in the folder"), |p| shown_path(&p.path)),
            ],
        ));
    }
}

fn children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.first_child();
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

fn row_widget(manager: &std::rc::Weak<Manager>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    row.add_css_class("hk-cover-row");
    let check = gtk::CheckButton::new();
    check.set_size_request(CHECK, -1);
    check.set_valign(gtk::Align::Center);
    let weak = manager.clone();
    let weak_row = row.downgrade();
    check.connect_toggled(move |check| {
        let Some(this) = weak.upgrade() else { return };
        if this.filling.get() {
            return;
        }
        let Some(key) = check
            .widget_name()
            .strip_prefix("album-")
            .and_then(|n| n.parse::<u64>().ok())
            .map(AlbumKey)
        else {
            return;
        };
        if check.is_active() {
            this.ticked.borrow_mut().insert(key);
        } else {
            this.ticked.borrow_mut().remove(&key);
        }
        if let Some(row) = weak_row.upgrade() {
            if check.is_active() {
                row.add_css_class("hk-ticked");
            } else {
                row.remove_css_class("hk-ticked");
            }
        }
        this.update_footer();
    });
    row.append(&check);

    let album = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    album.set_size_request(ALBUM_COLUMN, -1);
    album.set_hexpand(false);
    let art_stack = gtk::Stack::new();
    art_stack.set_valign(gtk::Align::Center);
    art_stack.set_halign(gtk::Align::Start);
    art_stack.set_hexpand(false);
    let art = CoverArt::new(32, 4.0);
    art_stack.add_named(&art, Some("art"));
    let none = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    none.add_css_class("hk-cover-none");
    none.set_size_request(32, 32);
    none.set_hexpand(false);
    let note = gtk::Image::from_icon_name("audio-x-generic-symbolic");
    note.set_halign(gtk::Align::Center);
    note.set_valign(gtk::Align::Center);
    note.set_size_request(32, 32);
    none.append(&note);
    art_stack.add_named(&none, Some("none"));
    album.append(&art_stack);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_valign(gtk::Align::Center);
    let title = label("", &["hk-medium"]);
    title.set_max_width_chars(1);
    title.set_hexpand(true);
    text.append(&title);
    let by = label("", &["hk-t3", "hk-side-line"]);
    by.set_max_width_chars(1);
    text.append(&by);
    text.set_hexpand(true);
    album.append(&text);
    row.append(&album);

    let folder = cell(-1, &["hk-mono-meta", "hk-t3"], false);
    folder.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    folder.set_max_width_chars(1);
    row.append(&folder);

    let picture = gtk::Box::new(gtk::Orientation::Vertical, 1);
    picture.set_size_request(PICTURE_COLUMN, -1);
    picture.set_valign(gtk::Align::Center);
    let name = label("", &["hk-mono"]);
    name.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    name.set_max_width_chars(1);
    picture.append(&name);
    picture.append(&label("", &["hk-t3", "hk-side-line"]));
    row.append(&picture);

    let button = compact_button("");
    button.set_size_request(BUTTON_COLUMN, -1);
    button.set_halign(gtk::Align::End);
    button.set_valign(gtk::Align::Center);
    let weak = manager.clone();
    button.connect_clicked(move |button| {
        let Some(this) = weak.upgrade() else { return };
        if let Some(index) = button
            .widget_name()
            .strip_prefix("row-")
            .and_then(|n| n.parse::<usize>().ok())
        {
            this.row_button(index);
        }
    });
    row.append(&button);
    row
}

/// Opens the cover manager over `main`, or brings it to the front.
pub(crate) fn open(main: &MainWindow) -> Option<gtk::Window> {
    let parent = main.upcast_ref::<gtk::Window>();
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        return Some(window);
    }
    let backend = main.backend()?;
    let export = backend.services().export();
    let frame = frame(parent, NAME, &tr("Cover manager"), &tr("Show"));
    frame.side_note.set_text(&tr(
        "Helikodu never downloads covers. It uses pictures that are already on this computer.",
    ));
    let mut counts = Vec::new();
    for group in GROUPS {
        let (row, count) = super::tool_window::side_row(&group_name(group), "");
        // One line: the second line is left out.
        if let Some(content) = row.child().and_downcast::<gtk::Box>()
            && let Some(text) = content.first_child().and_downcast::<gtk::Box>()
            && let Some(line) = text.last_child()
        {
            line.set_visible(false);
        }
        row.remove_css_class("two-line");
        frame.list.append(&row);
        counts.push((group, count));
    }
    let column = frame.main.clone();

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    toolbar.add_css_class("hk-toolbar");
    toolbar.add_css_class("hk-tool-toolbar");
    let filter = gtk::SearchEntry::new();
    filter.add_css_class("hk-input");
    filter.set_hexpand(true);
    filter.set_valign(gtk::Align::Center);
    filter.set_placeholder_text(Some(&tr("Filter by album or artist")));
    filter.update_property(&[gtk::accessible::Property::Label(&tr("Filter the albums"))]);
    toolbar.append(&filter);
    let sort = dropdown(
        &[tr("By album artist"), tr("By album"), tr("By year")],
        0,
        170,
    );
    sort.set_widget_name("cover-sort");
    sort.update_property(&[gtk::accessible::Property::Label(&tr("Order"))]);
    toolbar.append(&sort);
    column.append(&toolbar);
    column.append(&table_header(&[
        (String::new(), CHECK, false),
        (tr("Album"), ALBUM_COLUMN, false),
        (tr("Folder"), -1, false),
        (tr("Picture in the folder"), PICTURE_COLUMN, false),
        (String::new(), BUTTON_COLUMN, false),
    ]));
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::NoSelection::new(Some(store.clone()));
    let factory = gtk::SignalListItemFactory::new();
    let view = gtk::ListView::new(Some(selection), Some(factory.clone()));
    view.add_css_class("hk-dup-list");
    view.set_widget_name("cover-table");
    view.update_property(&[gtk::accessible::Property::Label(&tr("Albums"))]);
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
    let remove = compact_button(&tr("Remove"));
    remove.set_widget_name("remove-covers");
    remove.set_tooltip_text(Some(&tr(
        "Remove the cover inside the files of the chosen albums; pictures in the folders stay",
    )));
    bar.append(&remove);
    let save_label = label(&tr("Save"), &["hk-t2"]);
    bar.append(&save_label);
    let saves: Vec<String> = SAVES.iter().map(|s| save_name(*s)).collect();
    let save = dropdown(&saves, 0, 210);
    save.set_widget_name("cover-save");
    save.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        save_label.upcast_ref()
    ])]);
    bar.append(&save);
    let primary = primary_button("");
    primary.set_widget_name("use-pictures");
    bar.append(&primary);
    column.append(&bar);

    let manager = Rc::new(Manager {
        window: frame.window.clone(),
        backend,
        export,
        group: Cell::new(CoverShow::NoCover),
        albums: RefCell::new(Vec::new()),
        shown: RefCell::new(Vec::new()),
        ticked: RefCell::new(HashSet::new()),
        counts,
        title: frame.title.clone(),
        header_summary: frame.summary.clone(),
        filter: filter.clone(),
        sort: sort.clone(),
        store,
        stack,
        empty,
        summary,
        save,
        save_label,
        remove: remove.clone(),
        primary: primary.clone(),
        filling: Cell::new(false),
    });

    let weak = Rc::downgrade(&manager);
    factory.connect_setup(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&row_widget(&weak)));
        }
    });
    let weak = Rc::downgrade(&manager);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(this), Some(row), Some(object)) = (
            weak.upgrade(),
            item.child().and_downcast::<gtk::Box>(),
            item.item().and_downcast::<glib::BoxedAnyObject>(),
        ) else {
            return;
        };
        let index = *object.borrow::<usize>();
        this.bind(&row, item, index);
    });
    let weak = Rc::downgrade(&manager);
    frame.list.connect_row_selected(move |_, row| {
        let (Some(this), Some(row)) = (weak.upgrade(), row) else {
            return;
        };
        if let Some(group) = usize::try_from(row.index())
            .ok()
            .and_then(|i| GROUPS.get(i))
        {
            this.group.set(*group);
            this.ticked.borrow_mut().clear();
            this.rebuild();
        }
    });
    let weak = Rc::downgrade(&manager);
    filter.connect_search_changed(move |_| {
        if let Some(this) = weak.upgrade() {
            this.rebuild();
        }
    });
    let weak = Rc::downgrade(&manager);
    on_selected(&sort, move |_| {
        if let Some(this) = weak.upgrade() {
            this.rebuild();
        }
    });
    let weak = Rc::downgrade(&manager);
    primary.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.apply_ticked();
        }
    });
    let weak = Rc::downgrade(&manager);
    remove.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.remove_ticked();
        }
    });
    let keep_alive = RefCell::new(Some(manager.clone()));
    frame.window.connect_destroy(move |_| {
        keep_alive.borrow_mut().take();
    });
    let start = GROUPS
        .iter()
        .position(|g| *g == CoverShow::NoCover)
        .and_then(|i| frame.list.row_at_index(i32::try_from(i).unwrap_or(0)));
    frame.list.select_row(start.as_ref());
    manager.load();
    frame.window.present();
    Some(frame.window)
}
