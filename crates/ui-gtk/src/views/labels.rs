// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library, Labels: the labels at the left with the number of tracks each
//! covers, and the chosen label at the right: what it was given to, as artists,
//! albums and tracks, with Play all and Shuffle. Before any label exists the
//! view says what a label is.
//!
//! - A row's context menu (and the more button of the detail) renames the label
//!   in place (also F2), merges it into another (the merge dialog) or removes it
//!   from everything; each shows an Undo toast. Renaming to another label's name
//!   asks whether to merge.
//! - The context menu of an artist, album or track of the detail adds "Remove
//!   the label".
//! - A label that nothing has any more leaves the list, and the view chooses the
//!   next one.
//!
//! The list and the detail are read from the library; nothing is filtered here.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::backend::{
    AlbumRow, ArtistRow, Backend, LabelContents, LabelEdit, LabelEditError, LabelFor, LabelKey,
    LabelRow, PlayerRequest, TrackRow,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::{AlbumItem, CoverCache};
use crate::views::actions::{self, Target};
use crate::views::album_tile::AlbumTile;
use crate::views::genres::{list_and_detail, list_pane};
use crate::widgets::avatar::Avatar;
use crate::widgets::empty_state::empty_state;
use crate::widgets::pane_layouts::HeadLayout;
use crate::widgets::{CoverArt, WrapBox, label};
use crate::window::MainWindow;
use crate::window::labels::Scope;

/// The cover size of the album tiles.
const TILE: i32 = 132;

/// "A, B and C".
fn listing(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => fill(&tr("{} and {}"), &[&rest.join(", "), last]),
    }
}

/// "Given to 1 artist, 2 albums and 14 tracks · 63 tracks in all".
fn summary_text(row: &LabelRow) -> String {
    let mut parts = Vec::new();
    if row.artists > 0 {
        parts.push(fill(
            &ntr("{} artist", "{} artists", row.artists),
            &[&format::count(row.artists)],
        ));
    }
    if row.albums > 0 {
        parts.push(fill(
            &ntr("{} album", "{} albums", row.albums),
            &[&format::count(row.albums)],
        ));
    }
    if row.own_tracks > 0 {
        parts.push(fill(
            &ntr("{} track", "{} tracks", row.own_tracks),
            &[&format::count(row.own_tracks)],
        ));
    }
    let all = fill(
        &ntr("{} track in all", "{} tracks in all", row.tracks),
        &[&format::count(row.tracks)],
    );
    if parts.is_empty() {
        return all;
    }
    let given = fill(&tr("Given to {}"), &[&listing(&parts)]);
    fill(&tr("{} · {}"), &[&given, &all])
}

/// The Labels view.
pub(crate) struct LabelsView {
    pub(crate) root: gtk::Stack,
    count: gtk::Label,
    list: gtk::ListBox,
    detail: gtk::Stack,
    name: gtk::Label,
    summary: gtk::Label,
    artists_section: gtk::Box,
    artists: gtk::FlowBox,
    albums_section: gtk::Box,
    albums: gtk::FlowBox,
    tracks_section: gtk::Box,
    tracks_note: gtk::Label,
    tracks: gtk::ListBox,
    columns: [gtk::SizeGroup; 3],
    window: glib::WeakRef<MainWindow>,
    backend: Backend,
    covers: Rc<CoverCache>,
    rows: RefCell<Vec<LabelRow>>,
    contents: RefCell<Option<LabelContents>>,
    current: Cell<Option<LabelKey>>,
    wanted: Cell<Option<LabelKey>>,
    /// The position of the label chosen, to choose the next one when it goes.
    position: Cell<usize>,
    loaded: Cell<bool>,
    quiet: Cell<bool>,
    by_count: Cell<bool>,
    generation: Cell<u64>,
    /// What "Remove the label" of an item's context menu takes it from.
    item: RefCell<Option<(LabelFor, Scope)>>,
}

impl std::fmt::Debug for LabelsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelsView").finish_non_exhaustive()
    }
}

/// A section of the detail: a 40 px row with its section label, and `right`.
fn section(title: &str, right: Option<&gtk::Label>) -> (gtk::Box, gtk::Box) {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("hk-label-section");
    let title = label(title, &["hk-section-label"]);
    title.set_hexpand(true);
    title.set_valign(gtk::Align::Center);
    head.append(&title);
    if let Some(right) = right {
        right.set_valign(gtk::Align::Center);
        head.append(right);
    }
    section.append(&head);
    (section, head)
}

/// The menu of a label: Rename, Merge into, Remove from everything.
fn label_menu(key: LabelKey) -> gio::Menu {
    let menu = gio::Menu::new();
    let entry = |text: &str, action: &str| {
        let item = gio::MenuItem::new(Some(text), None);
        item.set_action_and_target_value(Some(action), Some(&key.0.to_variant()));
        item
    };
    let rename = entry(&tr("Rename…"), "labels.rename");
    rename.set_attribute_value("accel", Some(&"F2".to_variant()));
    menu.append_item(&rename);
    menu.append_item(&entry(&tr("Merge into…"), "labels.merge"));
    let remove = gio::Menu::new();
    remove.append_item(&entry(&tr("Remove from everything"), "labels.delete"));
    menu.append_section(None, &remove);
    menu
}

impl LabelsView {
    /// The view of `window` over `backend`.
    pub(crate) fn new(
        window: &MainWindow,
        backend: &Backend,
        covers: &Rc<CoverCache>,
        _playback: &Rc<crate::playback::Playback>,
    ) -> Rc<Self> {
        let root = gtk::Stack::new();
        root.set_hhomogeneous(false);
        let main = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let order = gio::Menu::new();
        order.append(Some(&tr("Name")), Some("labels.order::name"));
        order.append(Some(&tr("Number of tracks")), Some("labels.order::count"));
        let order_button = gtk::MenuButton::new();
        order_button.set_icon_name("view-sort-descending-symbolic");
        order_button.add_css_class("hk-menu");
        crate::widgets::menu_names::set_menu_later(&order_button, &order);
        order_button.set_tooltip_text(Some(&tr("Sort the labels")));
        order_button.update_property(&[gtk::accessible::Property::Label(&tr("Sort the labels"))]);
        let (pane, count, list) = list_pane(&tr("Labels"), order_button.upcast_ref());
        list.add_css_class("hk-label-list");
        main.append(&pane);

        let detail = gtk::Stack::new();
        detail.set_hexpand(true);
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("hk-label-head");
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        titles.append(&label(&tr("Label"), &["hk-section-label"]));
        let name = label("", &["hk-hero-title", "small"]);
        name.set_accessible_role(gtk::AccessibleRole::Heading);
        name.set_max_width_chars(1);
        titles.append(&name);
        let summary = label("", &["hk-t3"]);
        summary.set_max_width_chars(1);
        titles.append(&summary);
        header.append(&titles);
        let buttons = WrapBox::new(8);
        buttons.set_valign(gtk::Align::End);
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
        let more = gtk::MenuButton::new();
        more.set_icon_name("view-more-symbolic");
        more.add_css_class("hk-more");
        more.set_tooltip_text(Some(&tr("More")));
        more.update_property(&[gtk::accessible::Property::Label(&tr("More"))]);
        for w in [
            play.upcast_ref::<gtk::Widget>(),
            shuffle.upcast_ref(),
            more.upcast_ref(),
        ] {
            w.set_valign(gtk::Align::End);
            buttons.append(w);
        }
        header.append(&buttons);
        HeadLayout::install(&header);
        page.append(&header);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("hk-label-body");
        let (artists_section, _) = section(&tr("Artists"), None);
        let artists = gtk::FlowBox::new();
        artists.set_selection_mode(gtk::SelectionMode::None);
        artists.set_column_spacing(10);
        artists.set_row_spacing(10);
        artists.set_homogeneous(false);
        artists.set_halign(gtk::Align::Start);
        artists.set_max_children_per_line(12);
        artists_section.append(&artists);
        artists_section.add_css_class("hk-label-part");
        body.append(&artists_section);
        let (albums_section, _) = section(&tr("Albums"), None);
        let albums = gtk::FlowBox::new();
        albums.set_selection_mode(gtk::SelectionMode::None);
        albums.set_column_spacing(20);
        albums.set_row_spacing(20);
        albums.set_homogeneous(true);
        albums.set_halign(gtk::Align::Start);
        albums.set_valign(gtk::Align::Start);
        albums_section.append(&albums);
        albums_section.add_css_class("hk-label-part");
        body.append(&albums_section);
        let tracks_note = label("", &["hk-count"]);
        let (tracks_section, _) = section(&tr("Tracks"), Some(&tracks_note));
        tracks_section.add_css_class("hk-label-tracks");
        let table_head = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        table_head.add_css_class("hk-table-header");
        // Each column is as wide in the header as in every row.
        let columns = [
            gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
        ];
        for (i, (text, share)) in [(tr("Title"), 16), (tr("Artist"), 11), (tr("Album"), 13)]
            .into_iter()
            .enumerate()
        {
            let cell = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            cell.set_hexpand(true);
            if i == 0 {
                // Over the covers of the rows.
                let space = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                space.set_size_request(22, -1);
                cell.append(&space);
            }
            let l = label(&text, &["hk-section-label"]);
            l.set_max_width_chars(share);
            l.set_hexpand(true);
            cell.append(&l);
            columns[i].add_widget(&cell);
            table_head.append(&cell);
        }
        let length = label(&tr("Length"), &["hk-section-label"]);
        length.set_size_request(48, -1);
        length.set_xalign(1.0);
        table_head.append(&length);
        tracks_section.append(&table_head);
        let tracks = gtk::ListBox::new();
        tracks.add_css_class("hk-label-track-list");
        tracks.set_activate_on_single_click(false);
        tracks.update_property(&[gtk::accessible::Property::Label(&tr("Tracks"))]);
        tracks_section.append(&tracks);
        body.append(&tracks_section);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&body));
        page.append(&scroller);
        detail.add_named(&page, Some("label"));
        detail.add_named(&gtk::Box::new(gtk::Orientation::Vertical, 0), Some("none"));
        detail.set_visible_child_name("none");
        main.append(&detail);
        list_and_detail(&main);
        root.add_named(&main, Some("list"));

        let empty = empty_state(
            "helikodu-label-symbolic",
            &tr("No labels yet"),
            &tr(
                "A label is a word of your own, such as “late night” or “to sort”, that you give to tracks, albums and artists. Labels are kept in the library, not in the files.",
            ),
            &[],
        );
        let how = gtk::Label::new(None);
        how.add_css_class("hk-t2");
        how.set_margin_top(8);
        how.set_wrap(true);
        how.set_max_width_chars(52);
        how.set_justify(gtk::Justification::Center);
        let t1 = crate::style::current().palette.t1.hex();
        let menu_item = format!(
            "<span weight=\"600\" foreground=\"{t1}\">{}</span>",
            glib::markup_escape_text(&tr("Labels…"))
        );
        how.set_markup(&fill(
            &glib::markup_escape_text(&tr(
                "To give one, choose {} in the menu of a track, an album or an artist.",
            )),
            &[&menu_item],
        ));
        empty.append(&how);
        empty.set_margin_start(24);
        empty.set_margin_end(24);
        root.add_named(&empty, Some("empty"));
        root.set_visible_child_name("list");

        let view = Rc::new(Self {
            root,
            count,
            list,
            detail,
            name,
            summary,
            artists_section,
            artists,
            albums_section,
            albums,
            tracks_section,
            tracks_note,
            tracks,
            columns,
            window: window.downgrade(),
            backend: backend.clone(),
            covers: covers.clone(),
            rows: RefCell::new(Vec::new()),
            contents: RefCell::new(None),
            current: Cell::new(None),
            wanted: Cell::new(None),
            position: Cell::new(0),
            loaded: Cell::new(false),
            quiet: Cell::new(false),
            by_count: Cell::new(false),
            generation: Cell::new(0),
            item: RefCell::new(None),
        });
        view.connect(&play, &shuffle, &more);
        view
    }

    fn connect(self: &Rc<Self>, play: &gtk::Button, shuffle: &gtk::Button, more: &gtk::MenuButton) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_selected(move |_, row| {
            let (Some(view), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            if view.quiet.get() {
                return;
            }
            if let Some(key) = row_key(row)
                && let Some(window) = view.window.upgrade()
            {
                window.open_label(key);
            }
        });
        for (button, random) in [(play, false), (shuffle, true)] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.play(random);
                }
            });
        }
        let weak = Rc::downgrade(self);
        more.set_create_popup_func(move |button| {
            if let Some(view) = weak.upgrade()
                && let Some(key) = view.current.get()
            {
                button.set_menu_model(Some(&label_menu(key)));
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
            view.by_count.set(name == "count");
            view.fill();
        });
        group.add_action(&order);
        for name in ["rename", "merge", "delete"] {
            let action = gio::SimpleAction::new(name, Some(glib::VariantTy::UINT64));
            let weak = Rc::downgrade(self);
            action.connect_activate(move |action, target| {
                let (Some(view), Some(key)) = (weak.upgrade(), target.and_then(|t| t.get::<u64>()))
                else {
                    return;
                };
                let key = LabelKey(key);
                match action.name().as_str() {
                    "rename" => view.rename_in_place(key),
                    "merge" => view.ask_merge(key, None),
                    _ => view.remove_everywhere(key),
                }
            });
            group.add_action(&action);
        }
        let remove_item = gio::SimpleAction::new("remove-item", None);
        let weak = Rc::downgrade(self);
        remove_item.connect_activate(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.remove_from_item();
            }
        });
        group.add_action(&remove_item);
        self.root.insert_action_group("labels", Some(&group));
        // The context menu of a row.
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            if let Some(row) = view.list.row_at_y(y as i32)
                && let Some(key) = row_key(&row)
            {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                view.popup_label_menu(key, Some((x, y)));
            }
        });
        self.list.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let Some(label) = view.list.selected_row().and_then(|r| row_key(&r)) else {
                return glib::Propagation::Proceed;
            };
            if key == gdk::Key::F2 {
                view.rename_in_place(label);
            } else if actions::is_menu_key(key, state) {
                view.popup_label_menu(label, None);
            } else {
                return glib::Propagation::Proceed;
            }
            glib::Propagation::Stop
        });
        self.list.add_controller(keys);
        // Tracks: a double click or Enter plays from the row.
        let weak = Rc::downgrade(self);
        self.tracks.connect_row_activated(move |_, row| {
            if let Some(view) = weak.upgrade()
                && let Ok(index) = row.widget_name().parse::<usize>()
            {
                view.play_tracks_from(index);
            }
        });
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            if let Some(row) = view.tracks.row_at_y(y as i32)
                && let Ok(index) = row.widget_name().parse::<usize>()
            {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                view.tracks.select_row(Some(&row));
                view.popup_track_menu(index, Some((x, y)));
            }
        });
        self.tracks.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if actions::is_menu_key(key, state)
                && let Some(index) = view
                    .tracks
                    .selected_row()
                    .and_then(|r| r.widget_name().parse::<usize>().ok())
            {
                view.popup_track_menu(index, None);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.tracks.add_controller(keys);
    }

    fn popup_label_menu(&self, key: LabelKey, at: Option<(f64, f64)>) {
        let popover = gtk::PopoverMenu::from_model(Some(&label_menu(key)));
        crate::widgets::menu_names::name_menus(&popover);
        popover.add_css_class("hk-context-menu");
        let anchor: gtk::Widget = self
            .row_of(key)
            .map_or_else(|| self.list.clone().upcast(), Cast::upcast);
        popover.set_parent(&anchor);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        let (x, y) = match at {
            Some((x, y)) => self
                .list
                .compute_point(&anchor, &gtk::graphene::Point::new(x as f32, y as f32))
                .map_or((x, y), |p| (f64::from(p.x()), f64::from(p.y()))),
            None => (
                f64::from(anchor.width()) / 2.0,
                f64::from(anchor.height()) / 2.0,
            ),
        };
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        actions::popup_when_built(&popover);
    }

    /// The menu entry that takes the label shown from an item.
    fn remove_menu() -> gio::Menu {
        let menu = gio::Menu::new();
        menu.append(Some(&tr("Remove the label")), Some("labels.remove-item"));
        menu
    }

    fn popup_track_menu(&self, index: usize, at: Option<(f64, f64)>) {
        let Some(track) = self
            .contents
            .borrow()
            .as_ref()
            .and_then(|c| c.tracks.get(index).cloned())
        else {
            return;
        };
        self.item
            .replace(Some((LabelFor::Tracks(vec![track.key]), Scope::Tracks(1))));
        let anchor = self.track_row(index);
        let at = at.and_then(|(x, y)| {
            let anchor = anchor.as_ref()?;
            self.tracks
                .compute_point(anchor, &gtk::graphene::Point::new(x as f32, y as f32))
                .map(|p| (f64::from(p.x()), f64::from(p.y())))
        });
        let anchor: gtk::Widget = anchor.map_or_else(|| self.tracks.clone().upcast(), Cast::upcast);
        actions::popup_with(
            &anchor,
            Target::tracks(vec![Rc::new(track)]),
            at,
            Some(&Self::remove_menu()),
        );
    }

    fn track_row(&self, index: usize) -> Option<gtk::ListBoxRow> {
        self.tracks.row_at_index(i32::try_from(index).ok()?)
    }

    fn row_of(&self, key: LabelKey) -> Option<gtk::ListBoxRow> {
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && row_key(row) == Some(key)
            {
                return Some(row.clone());
            }
        }
        None
    }

    /// Shows the list, with `label` chosen (or the one chosen before).
    pub(crate) fn show(self: &Rc<Self>, label: Option<LabelKey>) {
        if label.is_some() {
            self.wanted.set(label);
        }
        if self.loaded.get() {
            self.select(self.wanted.get());
        } else {
            self.reload();
        }
    }

    /// Reads the labels again, and the chosen one's detail.
    pub(crate) fn reload(self: &Rc<Self>) {
        self.loaded.set(true);
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let rows = backend.query(|l| l.labels()).await.unwrap_or_default();
            if let Some(view) = weak.upgrade() {
                view.rows.replace(rows);
                // The detail is read again too.
                view.current.set(None);
                view.fill();
            }
        });
    }

    fn sorted(&self) -> Vec<LabelRow> {
        let mut rows = self.rows.borrow().clone();
        if self.by_count.get() {
            rows.sort_by_key(|r| std::cmp::Reverse(r.tracks));
        }
        rows
    }

    fn fill(self: &Rc<Self>) {
        let rows = self.sorted();
        if rows.is_empty() {
            self.root.set_visible_child_name("empty");
        } else {
            self.root.set_visible_child_name("list");
        }
        self.quiet.set(true);
        crate::widgets::remove_rows(&self.list);
        self.count.set_text(&format::count(rows.len()));
        for row_data in &rows {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&row_data.key.0.to_string());
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let name = label(&row_data.name, &[]);
            name.set_hexpand(true);
            line.append(&name);
            line.append(&label(&format::count(row_data.tracks), &["hk-count"]));
            row.set_child(Some(&line));
            row.update_property(&[gtk::accessible::Property::Label(&fill(
                &ntr("{}, {} track", "{}, {} tracks", row_data.tracks),
                &[&row_data.name, &row_data.tracks.to_string()],
            ))]);
            self.list.append(&row);
        }
        self.quiet.set(false);
        // The label wanted, or the one now where the gone one was.
        let wanted = self
            .wanted
            .get()
            .filter(|w| rows.iter().any(|r| r.key == *w))
            .or_else(|| {
                rows.get(self.position.get().min(rows.len().saturating_sub(1)))
                    .map(|r| r.key)
            });
        self.select(wanted);
    }

    fn select(self: &Rc<Self>, label: Option<LabelKey>) {
        let rows = self.sorted();
        let Some(label) = label.filter(|l| rows.iter().any(|r| r.key == *l)) else {
            self.detail.set_visible_child_name("none");
            self.current.set(None);
            return;
        };
        self.wanted.set(Some(label));
        if let Some(position) = rows.iter().position(|r| r.key == label) {
            self.position.set(position);
        }
        self.quiet.set(true);
        if let Some(row) = self.row_of(label) {
            self.list.select_row(Some(&row));
        }
        self.quiet.set(false);
        if self.current.get() == Some(label) {
            return;
        }
        self.current.set(Some(label));
        if let Some(row) = rows.iter().find(|r| r.key == label) {
            self.name.set_text(&row.name);
            self.name.set_tooltip_text(Some(&row.name));
            self.summary.set_text(&summary_text(row));
        }
        self.detail.set_visible_child_name("label");
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let contents = backend.query(move |l| l.label_contents(label)).await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() == generation {
                view.show_contents(contents.ok().flatten());
            }
        });
    }

    fn show_contents(self: &Rc<Self>, contents: Option<LabelContents>) {
        while let Some(child) = self.artists.first_child() {
            self.artists.remove(&child);
        }
        while let Some(child) = self.albums.first_child() {
            self.albums.remove(&child);
        }
        crate::widgets::remove_rows(&self.tracks);
        let Some(contents) = contents else {
            self.contents.replace(None);
            for section in [
                &self.artists_section,
                &self.albums_section,
                &self.tracks_section,
            ] {
                section.set_visible(false);
            }
            return;
        };
        self.artists_section
            .set_visible(!contents.artists.is_empty());
        for artist in &contents.artists {
            self.artists.append(&self.artist_chip(artist));
        }
        self.albums_section.set_visible(!contents.albums.is_empty());
        self.albums.set_max_children_per_line(
            u32::try_from(contents.albums.len().clamp(1, 8)).unwrap_or(1),
        );
        for album in &contents.albums {
            self.albums.append(&self.tile(album));
        }
        self.tracks_section.set_visible(!contents.tracks.is_empty());
        self.tracks_note.set_text(&fill(
            &ntr(
                "{} labelled one by one",
                "{} labelled one by one",
                contents.tracks.len(),
            ),
            &[&format::count(contents.tracks.len())],
        ));
        for (index, track) in contents.tracks.iter().enumerate() {
            self.tracks.append(&self.track_line(index, track));
        }
        self.contents.replace(Some(contents));
    }

    /// An artist: a 44 px chip with the avatar, the name and their tracks.
    fn artist_chip(self: &Rc<Self>, artist: &ArtistRow) -> gtk::FlowBoxChild {
        let button = gtk::Button::new();
        button.add_css_class("hk-label-artist");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let avatar = Avatar::new(28);
        avatar.set_name(&artist.name);
        avatar.set_photo(&self.covers, artist.photo);
        line.append(&avatar);
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 0);
        texts.set_valign(gtk::Align::Center);
        texts.append(&label(&artist.name, &[]));
        texts.append(&label(
            &fill(
                &ntr("{} track", "{} tracks", artist.track_count),
                &[&format::count(artist.track_count)],
            ),
            &["hk-t3"],
        ));
        line.append(&texts);
        button.set_child(Some(&line));
        button.update_property(&[gtk::accessible::Property::Label(&artist.name)]);
        let key = artist.key;
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(window) = weak.upgrade().and_then(|v| v.window.upgrade()) {
                window.open_artist(key);
            }
        });
        let name = artist.name.clone();
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let (Some(view), Some(widget)) = (weak.upgrade(), gesture.widget()) else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            view.item
                .replace(Some((LabelFor::Artist(key), Scope::Named(name.clone()))));
            actions::popup_with(
                &widget,
                Target::Artist(key),
                Some((x, y)),
                Some(&Self::remove_menu()),
            );
        });
        button.add_controller(click);
        let cell = gtk::FlowBoxChild::new();
        cell.set_focusable(false);
        cell.set_child(Some(&button));
        cell
    }

    /// An album tile at a cover size of 132.
    fn tile(self: &Rc<Self>, album: &AlbumRow) -> gtk::FlowBoxChild {
        let tile = AlbumTile::default();
        tile.cover().set_shape(TILE, 6.0);
        tile.set_hexpand(false);
        tile.set_size_request(TILE, -1);
        let item = AlbumItem::new(album.key, 0, Some(Rc::new(album.clone())));
        tile.bind(&item, &self.covers);
        tile.hide_meta();
        let cell = gtk::FlowBoxChild::new();
        cell.set_child(Some(&tile));
        cell.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{} by {}"),
            &[&album.title, &album.artist],
        ))]);
        let key = album.key;
        let title = album.title.clone();
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
                    view.item
                        .replace(Some((LabelFor::Album(key), Scope::Named(title.clone()))));
                    actions::popup_with(
                        &tile,
                        Target::Album(key),
                        Some((x, y)),
                        Some(&Self::remove_menu()),
                    );
                }
                gdk::BUTTON_PRIMARY if presses == 2 => {
                    view.backend.send(PlayerRequest::PlayAlbum {
                        album: key,
                        start: 0,
                        shuffle: false,
                    });
                }
                gdk::BUTTON_PRIMARY => {
                    if let Some(window) = view.window.upgrade() {
                        window.open_album(key);
                    }
                }
                _ => {}
            }
        });
        tile.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, pressed, _, _| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if matches!(pressed, gdk::Key::Return | gdk::Key::KP_Enter) {
                if let Some(window) = view.window.upgrade() {
                    window.open_album(key);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        cell.add_controller(keys);
        cell
    }

    /// A row of the table: a 22 px cover and the title, the artist, the album
    /// and the length; every other row is shaded.
    fn track_line(&self, index: usize, track: &TrackRow) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&index.to_string());
        if index % 2 == 1 {
            row.add_css_class("hk-alt");
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        let first = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        first.set_hexpand(true);
        let cover = CoverArt::new(22, 3.0);
        self.covers.show(&cover, track.cover, track.cover_seed, 22);
        first.append(&cover);
        let title = label(&track.title, &[]);
        title.set_max_width_chars(16);
        first.append(&title);
        self.columns[0].add_widget(&first);
        line.append(&first);
        for (i, (text, share)) in [(&track.artist, 11), (&track.album_title, 13)]
            .into_iter()
            .enumerate()
        {
            let l = label(text, &["hk-t2"]);
            l.set_hexpand(true);
            l.set_max_width_chars(share);
            self.columns[i + 1].add_widget(&l);
            line.append(&l);
        }
        let length = label(
            &track.duration.map(format::duration).unwrap_or_default(),
            &["hk-t2", "hk-numeric"],
        );
        length.set_xalign(1.0);
        length.set_size_request(48, -1);
        line.append(&length);
        row.set_child(Some(&line));
        row.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{} · {}"),
            &[&track.title, &track.artist],
        ))]);
        row
    }

    fn label_row(&self, key: LabelKey) -> Option<LabelRow> {
        self.rows.borrow().iter().find(|r| r.key == key).cloned()
    }

    /// Plays every track that has the label shown.
    fn play(&self, shuffle: bool) {
        let Some(key) = self.current.get() else {
            return;
        };
        let title = self.label_row(key).map(|r| r.name).unwrap_or_default();
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let tracks = backend
                .query(move |l| l.label_track_keys(key))
                .await
                .unwrap_or_default();
            if !tracks.is_empty() {
                backend.send(PlayerRequest::PlayTracks {
                    tracks,
                    start: 0,
                    title,
                    shuffle,
                });
            }
        });
    }

    /// Plays the tracks labelled one by one, from `index`.
    fn play_tracks_from(&self, index: usize) {
        let Some(contents) = self.contents.borrow().clone() else {
            return;
        };
        self.backend.send(PlayerRequest::PlayTracks {
            tracks: contents.tracks.iter().map(|t| t.key).collect(),
            start: index,
            title: contents.row.name.clone(),
            shuffle: false,
        });
    }

    /// Renames the label `key` in its row: Enter keeps the name, Escape or
    /// leaving the row keeps the old one.
    fn rename_in_place(self: &Rc<Self>, key: LabelKey) {
        let (Some(row), Some(data)) = (self.row_of(key), self.label_row(key)) else {
            return;
        };
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.add_css_class("hk-rename");
        entry.set_text(&data.name);
        entry.set_max_length(100);
        entry.update_property(&[gtk::accessible::Property::Label(&tr("Name of the label"))]);
        let old = row.child();
        row.set_child(Some(&entry));
        entry.grab_focus();
        entry.select_region(0, -1);
        let done = Rc::new(Cell::new(false));
        let restore = {
            let row = row.clone();
            move || {
                row.set_child(old.as_ref());
                row.grab_focus();
            }
        };
        let restore = Rc::new(restore);
        let weak = Rc::downgrade(self);
        let finished = done.clone();
        let put_back = restore.clone();
        entry.connect_activate(move |entry| {
            if finished.replace(true) {
                return;
            }
            let new_name = entry.text().trim().to_owned();
            put_back();
            let Some(view) = weak.upgrade() else { return };
            if new_name.is_empty() || new_name == data.name {
                return;
            }
            view.rename(&data, &new_name);
        });
        let keys = gtk::EventControllerKey::new();
        let finished = done.clone();
        let put_back = restore.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if !finished.replace(true) {
                    put_back();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |_| {
            if !done.replace(true) {
                restore();
            }
        });
        entry.add_controller(focus);
    }

    /// Renames `label` to `new_name`; the name of another label asks whether to
    /// merge the two.
    fn rename(self: &Rc<Self>, label: &LabelRow, new_name: &str) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let edit = LabelEdit::Rename {
            name: label.name.clone(),
            new_name: new_name.to_owned(),
        };
        let text = crate::window::labels::change_text(&edit, &Scope::Tracks(0));
        let key = label.key;
        let weak = Rc::downgrade(self);
        window.change_labels(edit, move |window, result| match result {
            Ok(done) => {
                window.toast_label_undo(&text, Rc::new(RefCell::new(done.undo)), None);
            }
            Err(LabelEditError::Exists(other)) => {
                if let Some(view) = weak.upgrade() {
                    view.ask_merge(key, Some(other));
                }
            }
            Err(error) => window.label_problem(&error),
        });
    }

    /// Asks into which label `key` is merged, `chosen` chosen first.
    fn ask_merge(self: &Rc<Self>, key: LabelKey, chosen: Option<String>) {
        let (Some(window), Some(from)) = (self.window.upgrade(), self.label_row(key)) else {
            return;
        };
        let others: Vec<LabelRow> = self
            .rows
            .borrow()
            .iter()
            .filter(|r| r.key != key)
            .cloned()
            .collect();
        let name = from.name.clone();
        let weak = window.downgrade();
        crate::dialogs::label_merge::ask(
            Some(window.upcast_ref()),
            &from,
            &others,
            chosen.as_deref(),
            move |into| {
                if let (Some(window), Some(into)) = (weak.upgrade(), into) {
                    window.change_labels_with_undo(
                        LabelEdit::Merge { from: name, into },
                        Scope::Tracks(0),
                    );
                }
            },
        );
    }

    /// Removes the label `key` from everything at once, with Undo.
    fn remove_everywhere(&self, key: LabelKey) {
        let (Some(window), Some(row)) = (self.window.upgrade(), self.label_row(key)) else {
            return;
        };
        window.change_labels_with_undo(LabelEdit::Delete { name: row.name }, Scope::Tracks(0));
    }

    /// Takes the label shown from the item whose menu was opened.
    fn remove_from_item(&self) {
        let (Some(window), Some((target, scope)), Some(key)) =
            (self.window.upgrade(), self.item.take(), self.current.get())
        else {
            return;
        };
        let Some(row) = self.label_row(key) else {
            return;
        };
        window.change_labels_with_undo(
            LabelEdit::Take {
                target,
                name: row.name,
            },
            scope,
        );
    }

    /// The list, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        if self.root.visible_child_name().as_deref() == Some("empty") {
            return self.root.clone().upcast();
        }
        self.list
            .selected_row()
            .map_or_else(|| self.list.clone().upcast(), Cast::upcast)
    }

    /// The labels listed and their tracks (for tests).
    pub(crate) fn listed(&self) -> Vec<(String, usize)> {
        self.sorted()
            .into_iter()
            .map(|r| (r.name, r.tracks))
            .collect()
    }

    /// The detail's name, summary, section labels and the names in them (for
    /// tests).
    pub(crate) fn detail_texts(&self) -> Vec<String> {
        let mut out = vec![
            self.name.text().to_string(),
            self.summary.text().to_string(),
        ];
        if let Some(contents) = self
            .contents
            .borrow()
            .as_ref()
            .filter(|c| Some(c.row.key) == self.current.get())
        {
            out.extend(
                contents
                    .artists
                    .iter()
                    .map(|a| format!("artist:{}", a.name)),
            );
            out.extend(contents.albums.iter().map(|a| format!("album:{}", a.title)));
            out.extend(contents.tracks.iter().map(|t| format!("track:{}", t.title)));
        }
        out
    }

    /// Whether the empty state is shown (for tests).
    pub(crate) fn empty_shown(&self) -> bool {
        self.root.visible_child_name().as_deref() == Some("empty")
    }

    /// Chooses the row at `index` as a click would (for tests).
    pub(crate) fn choose(&self, index: usize) {
        if let Some(row) = self.list.row_at_index(i32::try_from(index).unwrap_or(0)) {
            self.list.select_row(Some(&row));
        }
    }

    /// Runs the action `name` of a label's menu on the row at `index` (for
    /// tests).
    pub(crate) fn act(&self, name: &str, index: usize) {
        let Some(key) = self.sorted().get(index).map(|r| r.key) else {
            return;
        };
        let _ = self
            .root
            .activate_action(&format!("labels.{name}"), Some(&key.0.to_variant()));
    }

    /// The entry of a rename in place, if one is open (for tests).
    pub(crate) fn rename_entry(&self) -> Option<gtk::Entry> {
        self.list
            .selected_row()
            .and_then(|r| r.child())
            .and_downcast::<gtk::Entry>()
    }
}

/// The label of a row of the list.
fn row_key(row: &gtk::ListBoxRow) -> Option<LabelKey> {
    row.widget_name().parse::<u64>().ok().map(LabelKey)
}
