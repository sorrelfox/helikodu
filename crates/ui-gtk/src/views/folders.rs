// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library, Folders: the tree of library folders at the left, and the audio
//! files of the chosen folder at the right. A level of the tree is read when it
//! is opened. File names are shown from their bytes, lossily where they are not
//! valid UTF-8.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::backend::{
    Backend, FolderListing, FolderRow, FolderState, PlayerRequest, TrackKey, TrackRow,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::playback::{Change, Playback};
use crate::views::actions::{self, Target};
use crate::views::artists::button;
use crate::widgets::empty_state::empty_state;
use crate::widgets::{PlayingIndicator, label};

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct FolderItem {
        pub(super) row: RefCell<Option<FolderRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FolderItem {
        const NAME: &'static str = "HkFolderItem";
        type Type = super::FolderItem;
    }

    impl ObjectImpl for FolderItem {}
}

glib::wrapper! {
    /// One folder of the tree.
    pub(crate) struct FolderItem(ObjectSubclass<imp::FolderItem>);
}

impl FolderItem {
    fn new(row: FolderRow) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().row.replace(Some(row));
        obj
    }

    fn row(&self) -> Option<FolderRow> {
        self.imp().row.borrow().clone()
    }
}

/// The Folders view.
pub(crate) struct FoldersView {
    pub(crate) root: gtk::Box,
    tree: gtk::ListView,
    roots: gio::ListStore,
    selection: gtk::SingleSelection,
    detail: gtk::Stack,
    parent: gtk::Label,
    name: gtk::Label,
    summary: gtk::Label,
    files: gtk::ListBox,
    status_left: gtk::Label,
    status_right: gtk::Label,
    backend: Backend,
    listing: RefCell<FolderListing>,
    folder: RefCell<Option<PathBuf>>,
    loaded: Cell<bool>,
    generation: Cell<u64>,
    playing: Cell<Option<TrackKey>>,
}

impl std::fmt::Debug for FoldersView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FoldersView").finish_non_exhaustive()
    }
}

/// The parent of `path` as shown above the folder name.
fn parent_text(path: &Path) -> String {
    let home = glib::home_dir();
    let parent = path.parent().unwrap_or(path);
    let shown = match parent.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => parent.display().to_string(),
    };
    format!("{} /", shown.replace('/', " / "))
}

impl FoldersView {
    /// The view over `backend`.
    pub(crate) fn new(backend: &Backend, playback: &Rc<Playback>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pane.add_css_class("hk-list-pane");
        pane.set_size_request(300, -1);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.add_css_class("hk-pane-header");
        let heading = label(&tr("Folders"), &["hk-view-title"]);
        heading.set_hexpand(true);
        heading.set_accessible_role(gtk::AccessibleRole::Heading);
        head.append(&heading);
        let add = crate::widgets::icon_button("list-add-symbolic", &tr("Add a library folder"));
        add.set_action_name(Some("app.add-folder"));
        head.append(&add);
        pane.append(&head);
        let roots = gio::ListStore::new::<FolderItem>();
        let roots_for_tree = roots.clone();
        let backend_for_tree = backend.clone();
        let tree_model = gtk::TreeListModel::new(roots_for_tree, false, false, move |item| {
            let row = item.downcast_ref::<FolderItem>()?.row()?;
            if !row.has_children || row.state == FolderState::Absent {
                return None;
            }
            let children = gio::ListStore::new::<FolderItem>();
            let store = children.downgrade();
            let backend = backend_for_tree.clone();
            let path = row.path.clone();
            glib::spawn_future_local(async move {
                let rows = backend
                    .query(move |l| l.subfolders(&path))
                    .await
                    .unwrap_or_default();
                if let Some(store) = store.upgrade() {
                    let items: Vec<FolderItem> = rows.into_iter().map(FolderItem::new).collect();
                    store.splice(0, 0, &items);
                }
            });
            Some(children.upcast())
        });
        let selection = gtk::SingleSelection::new(Some(tree_model));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let tree = gtk::ListView::new(Some(selection.clone()), Some(Self::tree_factory()));
        tree.add_css_class("hk-list");
        tree.add_css_class("hk-tree");
        tree.update_property(&[gtk::accessible::Property::Label(&tr("Folders"))]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&tree));
        pane.append(&scroller);
        root.append(&pane);

        let detail = gtk::Stack::new();
        detail.set_hexpand(true);
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("hk-folder-header");
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        let parent = label("", &["hk-mono-meta", "hk-t3", "hk-path"]);
        titles.append(&parent);
        let name = label("", &["hk-folder-title"]);
        name.set_accessible_role(gtk::AccessibleRole::Heading);
        titles.append(&name);
        let summary = label("", &["hk-t3"]);
        titles.append(&summary);
        header.append(&titles);
        let play = button(&tr("Play"), Some("media-playback-start-symbolic"), true);
        let enqueue = button(&tr("Add to queue"), Some("list-add-symbolic"), false);
        let open = button(&tr("Open in file manager"), None, false);
        for b in [&play, &enqueue, &open] {
            b.set_valign(gtk::Align::Center);
            header.append(b);
        }
        page.append(&header);
        let table_header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        table_header.add_css_class("hk-table-header");
        table_header.add_css_class("hk-file-row");
        for (text, width, right) in [
            ("#".to_owned(), 20, true),
            (tr("File name"), 0, false),
            (tr("Title"), 0, false),
            (tr("Length"), 48, true),
            (tr("Size"), 64, true),
        ] {
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
            table_header.append(&l);
        }
        page.append(&table_header);
        let files = gtk::ListBox::new();
        files.add_css_class("hk-files");
        files.set_selection_mode(gtk::SelectionMode::Multiple);
        files.update_property(&[gtk::accessible::Property::Label(&tr("Audio files"))]);
        let files_scroller = gtk::ScrolledWindow::new();
        files_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        files_scroller.set_vexpand(true);
        files_scroller.set_child(Some(&files));
        page.append(&files_scroller);
        let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        status.add_css_class("hk-statusbar");
        let status_left = label("", &[]);
        status_left.set_hexpand(true);
        status.append(&status_left);
        let status_right = label("", &[]);
        status.append(&status_right);
        page.append(&status);
        detail.add_named(&page, Some("folder"));
        detail.add_named(
            &empty_state(
                "folder-symbolic",
                &tr("No folder chosen"),
                &tr("Choose a folder to see its audio files."),
                &[],
            ),
            Some("none"),
        );
        detail.set_visible_child_name("none");
        root.append(&detail);

        let view = Rc::new(Self {
            root,
            tree,
            roots,
            selection,
            detail,
            parent,
            name,
            summary,
            files,
            status_left,
            status_right,
            backend: backend.clone(),
            listing: RefCell::new(FolderListing::default()),
            folder: RefCell::new(None),
            loaded: Cell::new(false),
            generation: Cell::new(0),
            playing: Cell::new(None),
        });
        view.connect(playback, &play, &enqueue, &open);
        view
    }

    fn tree_factory() -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let expander = gtk::TreeExpander::new();
            expander.set_indent_for_icon(true);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            line.append(&gtk::Image::from_icon_name("folder-symbolic"));
            let name = label("", &[]);
            name.set_hexpand(true);
            name.set_max_width_chars(1);
            line.append(&name);
            let tag = label("", &["hk-mono-meta", "hk-t3"]);
            line.append(&tag);
            expander.set_child(Some(&line));
            item.set_child(Some(&expander));
        });
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(tree_row) = item.item().and_downcast::<gtk::TreeListRow>() else {
                return;
            };
            let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else {
                return;
            };
            expander.set_list_row(Some(&tree_row));
            let Some(row) = tree_row
                .item()
                .and_downcast::<FolderItem>()
                .and_then(|f| f.row())
            else {
                return;
            };
            let Some(line) = expander.child() else { return };
            let icon = line.first_child().and_downcast::<gtk::Image>();
            let name = icon
                .as_ref()
                .and_then(|i| i.next_sibling())
                .and_downcast::<gtk::Label>();
            let tag = line.last_child().and_downcast::<gtk::Label>();
            let absent = row.state == FolderState::Absent;
            if let Some(icon) = icon {
                icon.set_icon_name(Some(if absent {
                    "drive-harddisk-symbolic"
                } else {
                    "folder-symbolic"
                }));
            }
            if let Some(name) = name {
                name.set_text(&row.name);
                for (class, on) in [("hk-medium", row.root), ("hk-t3", absent)] {
                    if on {
                        name.add_css_class(class);
                    } else {
                        name.remove_css_class(class);
                    }
                }
            }
            if let Some(tag) = tag {
                tag.set_text(&match row.state {
                    FolderState::Polled if row.root => tr("NFS"),
                    FolderState::Absent => tr("not connected"),
                    _ => String::new(),
                });
            }
            item.set_selectable(!absent);
            item.set_activatable(!absent);
            let text = if absent {
                fill(&tr("{}, not connected"), &[&row.name])
            } else {
                row.name.clone()
            };
            expander.update_property(&[gtk::accessible::Property::Label(&text)]);
        });
        factory
    }

    fn connect(
        self: &Rc<Self>,
        playback: &Rc<Playback>,
        play: &gtk::Button,
        enqueue: &gtk::Button,
        open: &gtk::Button,
    ) {
        let weak = Rc::downgrade(self);
        self.selection
            .connect_selected_item_notify(move |selection| {
                let Some(view) = weak.upgrade() else { return };
                let row = selection
                    .selected_item()
                    .and_downcast::<gtk::TreeListRow>()
                    .and_then(|r| r.item())
                    .and_downcast::<FolderItem>()
                    .and_then(|f| f.row());
                if let Some(row) = row {
                    view.open_folder(&row.path);
                }
            });
        for (button, add) in [(play, false), (enqueue, true)] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.play_from(0, add);
                }
            });
        }
        let weak = Rc::downgrade(self);
        open.connect_clicked(move |_| {
            let Some(view) = weak.upgrade() else { return };
            let Some(window) = view.root.root().and_downcast::<crate::window::MainWindow>() else {
                return;
            };
            let first = view.listing.borrow().files.first().map(|f| f.path.clone());
            actions::open_folder(&window, first.or_else(|| view.folder.borrow().clone()));
        });
        let weak = Rc::downgrade(self);
        self.files.connect_row_activated(move |_, row| {
            if let Some(view) = weak.upgrade() {
                view.play_from(row.index().max(0) as usize, false);
            }
        });
        let weak = Rc::downgrade(self);
        playback.connect(move |player, change| {
            if let Some(view) = weak.upgrade()
                && change == Change::Track
            {
                let key = player.now.as_ref().map(|n| n.track.key);
                if view.playing.replace(key) != key {
                    view.show_files();
                }
            }
        });
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            let Some(row) = view.files.row_at_y(y as i32) else {
                return;
            };
            if !row.is_selected() {
                view.files.unselect_all();
                view.files.select_row(Some(&row));
            }
            if let Some(target) = view.target() {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                actions::popup(&view.files, target, Some((x, y)));
            }
        });
        self.files.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if actions::is_menu_key(key, state)
                && let Some(target) = view.target()
            {
                actions::popup(&view.files, target, None);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.files.add_controller(keys);
    }

    /// Shows the tree, reading the library folders the first time.
    pub(crate) fn show(self: &Rc<Self>) {
        if !self.loaded.get() {
            self.load();
        }
    }

    fn load(self: &Rc<Self>) {
        self.loaded.set(true);
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let roots = backend
                .query(|l| l.folder_roots())
                .await
                .unwrap_or_default();
            let Some(view) = weak.upgrade() else { return };
            let items: Vec<FolderItem> = roots.into_iter().map(FolderItem::new).collect();
            view.roots.splice(0, view.roots.n_items(), &items);
            // Open the first library folder.
            if let Some(model) = view.selection.model()
                && let Some(first) = model.item(0).and_downcast::<gtk::TreeListRow>()
            {
                first.set_expanded(true);
            }
        });
    }

    fn open_folder(self: &Rc<Self>, path: &Path) {
        self.folder.replace(Some(path.to_owned()));
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        let folder = path.to_owned();
        glib::spawn_future_local(async move {
            let listing = backend.query(move |l| l.folder(&folder)).await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            let listing = listing.unwrap_or_default();
            view.listing.replace(listing);
            view.show_header();
            view.show_files();
        });
    }

    fn show_header(&self) {
        let Some(path) = self.folder.borrow().clone() else {
            return;
        };
        let listing = self.listing.borrow();
        self.detail.set_visible_child_name("folder");
        self.parent.set_text(&parent_text(&path));
        self.name.set_text(
            &path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        );
        let count = listing.files.len();
        let mut parts = vec![fill(
            &ntr("{} audio file", "{} audio files", count),
            &[&crate::format::count(count)],
        )];
        if count > 0 {
            parts.push(format::size(listing.files.iter().map(|f| f.size).sum()));
        }
        if let Some(track) = listing.files.iter().find_map(|f| f.track.as_ref()) {
            let f = &track.format;
            let detail = match (&f.codec, f.sample_rate, f.bits) {
                (Some(codec), Some(rate), Some(bits)) => fill(
                    &tr("{} {} kHz / {}-bit"),
                    &[codec, &format::khz(rate), &bits.to_string()],
                ),
                _ => format::short(f),
            };
            if !detail.is_empty() {
                parts.push(detail);
            }
        }
        self.summary.set_text(&parts.join(" · "));
        self.status_left.set_text(&match listing.state {
            Some(FolderState::Watched) => tr("In the library · watched for changes"),
            Some(FolderState::Polled) => tr("In the library · rescanned now and then"),
            Some(FolderState::Absent) => tr("In the library · not connected"),
            None => tr("Not in the library"),
        });
        self.status_right.set_text(&if listing.other_files > 0 {
            fill(
                &ntr(
                    "{} other file not shown",
                    "{} other files not shown",
                    listing.other_files,
                ),
                &[&crate::format::count(listing.other_files)],
            )
        } else {
            String::new()
        });
    }

    fn show_files(&self) {
        while let Some(child) = self.files.first_child() {
            self.files.remove(&child);
        }
        let playing = self.playing.get();
        for (i, file) in self.listing.borrow().files.iter().enumerate() {
            let row = gtk::ListBoxRow::new();
            if i % 2 == 1 {
                row.add_css_class("hk-alt");
            }
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            line.add_css_class("hk-file-row");
            let on = file.track.as_ref().is_some_and(|t| Some(t.key) == playing);
            if on {
                row.add_css_class("hk-playing");
                let indicator = PlayingIndicator::default();
                indicator.set_size_request(20, -1);
                indicator.set_halign(gtk::Align::End);
                line.append(&indicator);
            } else {
                let number = label(&(i + 1).to_string(), &["hk-t3", "hk-numeric"]);
                number.set_xalign(1.0);
                number.set_size_request(20, -1);
                line.append(&number);
            }
            let name = label(&file.name, &["hk-mono", "hk-file-name"]);
            name.set_hexpand(true);
            name.set_max_width_chars(1);
            line.append(&name);
            let title = label(
                file.track.as_ref().map_or("", |t| t.title.as_str()),
                &["hk-t2"],
            );
            title.set_hexpand(true);
            title.set_max_width_chars(1);
            line.append(&title);
            for (text, width, class) in [
                (
                    file.track
                        .as_ref()
                        .and_then(|t| t.duration)
                        .map(format::duration)
                        .unwrap_or_default(),
                    48,
                    "hk-t2",
                ),
                (format::size(file.size), 64, "hk-t3"),
            ] {
                let l = label(&text, &[class, "hk-numeric"]);
                l.set_xalign(1.0);
                l.set_size_request(width, -1);
                line.append(&l);
            }
            row.set_child(Some(&line));
            row.update_property(&[gtk::accessible::Property::Label(&file.name)]);
            self.files.append(&row);
        }
    }

    fn play_from(&self, start: usize, enqueue: bool) {
        let listing = self.listing.borrow();
        let all_known = listing.files.iter().all(|f| f.track.is_some());
        if all_known && !listing.files.is_empty() {
            let tracks: Vec<TrackKey> = listing
                .files
                .iter()
                .filter_map(|f| f.track.as_ref().map(|t| t.key))
                .collect();
            if enqueue {
                self.backend.send(PlayerRequest::Enqueue {
                    tracks,
                    next: false,
                });
            } else {
                self.backend.send(PlayerRequest::PlayTracks {
                    tracks,
                    start,
                    title: self.name.text().to_string(),
                    shuffle: false,
                });
            }
        } else {
            let files: Vec<PathBuf> = listing
                .files
                .iter()
                .skip(if enqueue { 0 } else { start })
                .map(|f| f.path.clone())
                .collect();
            if !files.is_empty() {
                self.backend
                    .send(PlayerRequest::PlayFiles { files, enqueue });
            }
        }
    }

    /// The selected files that are library tracks, as a menu target.
    pub(crate) fn target(&self) -> Option<Target> {
        let listing = self.listing.borrow();
        let rows: Vec<Rc<TrackRow>> = self
            .files
            .selected_rows()
            .iter()
            .filter_map(|r| listing.files.get(r.index().max(0) as usize))
            .filter_map(|f| {
                f.track.clone().map(|mut t| {
                    t.path.get_or_insert_with(|| f.path.clone());
                    Rc::new(t)
                })
            })
            .collect();
        (!rows.is_empty()).then(|| Target::tracks(rows))
    }

    /// Reads the library folders again after the library changed.
    pub(crate) fn library_changed(self: &Rc<Self>) {
        if self.loaded.get() {
            self.load();
            if let Some(folder) = self.folder.borrow().clone() {
                self.open_folder(&folder);
            }
        }
    }

    /// Opens the first folder of the first library folder, then the first
    /// folder in it, step by step as the levels arrive (for screenshots).
    pub(crate) fn demo_step(&self) {
        let Some(model) = self.selection.model() else {
            return;
        };
        for position in 0..model.n_items() {
            let Some(row) = model.item(position).and_downcast::<gtk::TreeListRow>() else {
                continue;
            };
            if row.depth() < 2 && row.is_expandable() && !row.is_expanded() {
                row.set_expanded(true);
                return;
            }
            if row.depth() == 2 {
                self.selection.set_selected(position);
                return;
            }
        }
    }

    /// The tree, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.tree.clone().upcast()
    }

    /// The number of library folders (for tests).
    pub(crate) fn count(&self) -> usize {
        self.roots.n_items() as usize
    }
}
