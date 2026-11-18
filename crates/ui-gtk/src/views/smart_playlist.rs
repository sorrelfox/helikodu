// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A smart playlist: the hero (the funnel tile, the name, a summary, Play,
//! Shuffle, Edit rules and more), then either the rule line and the table of
//! its tracks, or, while editing, the rule builder beside the tracks that
//! match.
//!
//! The builder edits the rules of [`smart_rules`](super::smart_rules); every
//! change that reads as a query is saved at once, and the readout shows the
//! same rules as query text. Editing the text reads it again; text that cannot
//! be read keeps the last rules and says why under the box. The tracks follow
//! the library: they are read again whenever it changes. A click on a header
//! sorts the table for this visit; the rules keep their order.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use helikodu_model::query::{Field, Limit, LimitUnit};
use helikodu_service::library::{parse_query, print_query};

use super::smart_rules::{Cond, FIELDS, Node, Rule, Rules, UNITS, field_label, orders, unit_label};
use crate::backend::{
    Backend, PlayerRequest, PlaylistKey, PlaylistRow, Query, SmartTrack, TrackKey,
};
use crate::dialogs::common::dropdown;
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::{CoverCache, TrackItem};
use crate::views::actions::{self, Target};
use crate::views::artists::button;
use crate::views::table::{TableCell, cell_at, selected_positions};
use crate::widgets::{CoverArt, label};

/// The columns of a table: label, width (0 shares the rest), right-aligned.
fn columns(editing: bool) -> Vec<(String, i32, bool)> {
    if editing {
        vec![
            ("#".to_owned(), 20, true),
            (tr("Title"), 0, false),
            (tr("Artist"), 0, false),
            (tr("Format"), 84, false),
            (tr("Added"), 86, true),
        ]
    } else {
        vec![
            ("#".to_owned(), 22, true),
            (tr("Title"), 0, false),
            (tr("Artist"), 0, false),
            (tr("Album"), 0, false),
            (tr("Format"), 92, false),
            (tr("Added"), 96, true),
            (tr("Length"), 48, true),
        ]
    }
}

/// "9 Nov 2026".
fn date(time: Option<SystemTime>) -> String {
    let Some(seconds) = time
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
    else {
        return String::new();
    };
    glib::DateTime::from_unix_local(seconds)
        .ok()
        .and_then(|d| d.format("%-d %b %Y").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// One table: its header, list and the rows it shows.
struct Table {
    header: gtk::Box,
    list: gtk::ListView,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
}

/// The smart playlist page.
pub(crate) struct SmartPlaylistView {
    pub(crate) root: gtk::Box,
    name: gtk::Label,
    summary: gtk::Label,
    edit: gtk::Button,
    stack: gtk::Stack,
    chips: gtk::Box,
    order: gtk::Label,
    view: Table,
    status_left: gtk::Label,
    status_right: gtk::Label,
    rules_box: gtk::Box,
    match_any: gtk::DropDown,
    limit_on: gtk::CheckButton,
    limit_amount: gtk::Entry,
    limit_unit: gtk::DropDown,
    sort: gtk::DropDown,
    readout: gtk::Label,
    text: gtk::Entry,
    text_button: gtk::Button,
    problem: gtk::Label,
    matched: gtk::Label,
    of_total: gtk::Label,
    results: Table,
    backend: Backend,
    covers: Rc<CoverCache>,
    key: Cell<Option<PlaylistKey>>,
    info: RefCell<Option<PlaylistRow>>,
    tracks: RefCell<Vec<SmartTrack>>,
    /// The table's order for this visit: column and descending.
    sorted: Cell<Option<(usize, bool)>>,
    rules: RefCell<Rules>,
    query: RefCell<Query>,
    /// The rules cannot be shown as rows and are edited as text.
    text_only: Cell<bool>,
    editing: Cell<bool>,
    /// Set while the controls are filled in, so their signals change nothing.
    filling: Cell<bool>,
    generation: Cell<u64>,
}

impl std::fmt::Debug for SmartPlaylistView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmartPlaylistView")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

fn table(editing: bool) -> Table {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    header.add_css_class("hk-table-header");
    header.add_css_class("hk-playlist-row");
    let store = gio::ListStore::new::<TrackItem>();
    let selection = gtk::MultiSelection::new(Some(store.clone()));
    let list = gtk::ListView::new(Some(selection.clone()), None::<gtk::ListItemFactory>);
    list.add_css_class("hk-playlist");
    if editing {
        list.add_css_class("compact");
    }
    list.set_vexpand(true);
    list.update_property(&[gtk::accessible::Property::Label(&tr(
        "Tracks of the smart playlist",
    ))]);
    Table {
        header,
        list,
        store,
        selection,
    }
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(child));
    scroller
}

/// A small button with an icon and text.
fn small(text: &str, icon: &str) -> gtk::Button {
    let b = button(text, Some(icon), false);
    b.add_css_class("small");
    b
}

/// The 6 px dot and "Live".
fn live() -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("hk-live-dot");
    dot.set_valign(gtk::Align::Center);
    line.append(&dot);
    line.append(&label(&tr("Live"), &["hk-t3"]));
    line.set_tooltip_text(Some(&tr("The tracks follow the library as it changes")));
    line
}

impl SmartPlaylistView {
    /// The page over `backend`.
    pub(crate) fn new(backend: &Backend, covers: &Rc<CoverCache>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-smart-playlist");
        let hero = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        hero.add_css_class("hk-playlist-hero");
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
        tile.add_css_class("hk-smart-tile");
        tile.set_valign(gtk::Align::Center);
        let funnel = gtk::Image::from_icon_name("helikodu-funnel-symbolic");
        funnel.set_pixel_size(32);
        tile.set_size_request(88, 88);
        tile.set_valign(gtk::Align::Center);
        funnel.set_valign(gtk::Align::Center);
        funnel.set_halign(gtk::Align::Center);
        tile.set_homogeneous(true);
        tile.append(&funnel);
        hero.append(&tile);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        titles.set_valign(gtk::Align::Center);
        titles.append(&label(&tr("Smart playlist"), &["hk-section-label"]));
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
        let edit = button(&tr("Edit rules"), Some("document-edit-symbolic"), false);
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
            edit.upcast_ref(),
            more.upcast_ref(),
        ] {
            buttons.append(b);
        }
        hero.append(&buttons);
        root.append(&hero);

        // Viewing: the rule line, the table, the status bar.
        let viewing = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let rule_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        rule_line.add_css_class("hk-rule-line");
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        rule_line.append(&chips);
        let order = label("", &["hk-t3"]);
        order.set_margin_start(6);
        order.set_hexpand(true);
        rule_line.append(&order);
        rule_line.append(&live());
        viewing.append(&rule_line);
        let view = table(false);
        viewing.append(&view.header);
        viewing.append(&scrolled(&view.list));
        let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        status.add_css_class("hk-statusbar");
        let status_left = label("", &[]);
        status_left.set_hexpand(true);
        status.append(&status_left);
        let status_right = label("", &[]);
        status.append(&status_right);
        viewing.append(&status);

        // Editing: the rules pane and the results pane.
        let editing = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        editing.add_css_class("hk-rule-editor");
        let pane = gtk::Box::new(gtk::Orientation::Vertical, 8);
        pane.add_css_class("hk-rules-pane");
        let match_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        match_line.append(&label(&tr("Match"), &[]));
        let match_any = dropdown(&[tr("all"), tr("any")], 0, 72);
        match_any.update_property(&[gtk::accessible::Property::Label(&tr("Match"))]);
        match_line.append(&match_any);
        match_line.append(&label(&tr("of these rules"), &[]));
        pane.append(&match_line);
        let rules_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        pane.append(&rules_box);
        pane.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let limit_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let limit_on = gtk::CheckButton::with_label(&tr("Limit to"));
        limit_line.append(&limit_on);
        let limit_amount = gtk::Entry::new();
        limit_amount.add_css_class("hk-input");
        limit_amount.set_width_chars(4);
        limit_amount.set_max_width_chars(5);
        limit_amount.set_size_request(56, -1);
        limit_amount.set_input_purpose(gtk::InputPurpose::Digits);
        limit_amount.update_property(&[gtk::accessible::Property::Label(&tr("Limit"))]);
        limit_line.append(&limit_amount);
        let units: Vec<String> = UNITS.iter().map(|u| unit_label(*u)).collect();
        let limit_unit = dropdown(&units, 0, 92);
        limit_unit.update_property(&[gtk::accessible::Property::Label(&tr("Limit unit"))]);
        limit_line.append(&limit_unit);
        limit_line.append(&label(&tr("sorted by"), &[]));
        let mut order_names: Vec<String> = orders().into_iter().map(|(n, _, _)| n).collect();
        order_names.push(tr("As written in the text"));
        let sort = dropdown(&order_names, 0, 0);
        sort.set_hexpand(true);
        sort.set_halign(gtk::Align::Fill);
        sort.update_property(&[gtk::accessible::Property::Label(&tr("Order"))]);
        limit_line.append(&sort);
        pane.append(&limit_line);
        let readout = gtk::Box::new(gtk::Orientation::Vertical, 8);
        readout.add_css_class("hk-query-readout");
        let readout_head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let micro = label(&tr("The same rules as query text"), &["hk-section-label"]);
        micro.set_hexpand(true);
        readout_head.append(&micro);
        let text_button = gtk::Button::with_label(&tr("Edit as text"));
        text_button.add_css_class("hk-btn");
        text_button.add_css_class("small");
        readout_head.append(&text_button);
        readout.append(&readout_head);
        let readout_text = label("", &["hk-mono", "hk-query-text"]);
        readout_text.set_wrap(true);
        readout_text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        readout_text.set_ellipsize(gtk::pango::EllipsizeMode::None);
        readout_text.set_selectable(true);
        readout.append(&readout_text);
        let text = gtk::Entry::new();
        text.add_css_class("hk-input");
        text.add_css_class("hk-mono");
        text.set_visible(false);
        text.update_property(&[gtk::accessible::Property::Label(&tr("Query text"))]);
        readout.append(&text);
        pane.append(&readout);
        let problem = label("", &["hk-query-problem"]);
        problem.set_wrap(true);
        problem.set_visible(false);
        pane.append(&problem);
        let pane_scroller = scrolled(&pane);
        pane_scroller.set_size_request(560, -1);
        pane_scroller.set_hexpand(false);
        pane_scroller.add_css_class("hk-rules-scroller");
        editing.append(&pane_scroller);
        let results_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        results_box.set_hexpand(true);
        let results_head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        results_head.add_css_class("hk-results-head");
        let matched = label("", &["hk-strong"]);
        results_head.append(&matched);
        let of_total = label("", &["hk-t3"]);
        of_total.set_hexpand(true);
        results_head.append(&of_total);
        results_head.append(&live());
        results_box.append(&results_head);
        let results = table(true);
        results_box.append(&results.header);
        results_box.append(&scrolled(&results.list));
        editing.append(&results_box);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.add_named(&viewing, Some("view"));
        stack.add_named(&editing, Some("edit"));
        root.append(&stack);

        let view = Rc::new(Self {
            root,
            name,
            summary,
            edit,
            stack,
            chips,
            order,
            view,
            status_left,
            status_right,
            rules_box,
            match_any,
            limit_on,
            limit_amount,
            limit_unit,
            sort,
            readout: readout_text,
            text,
            text_button,
            problem,
            matched,
            of_total,
            results,
            backend: backend.clone(),
            covers: covers.clone(),
            key: Cell::new(None),
            info: RefCell::new(None),
            tracks: RefCell::new(Vec::new()),
            sorted: Cell::new(None),
            rules: RefCell::new(Rules::default()),
            query: RefCell::new(Query::default()),
            text_only: Cell::new(false),
            editing: Cell::new(false),
            filling: Cell::new(false),
            generation: Cell::new(0),
        });
        view.fill_header(false);
        view.fill_header(true);
        view.view.list.set_factory(Some(&view.factory(false)));
        view.results.list.set_factory(Some(&view.factory(true)));
        view.connect(&play, &shuffle, &more);
        view
    }

    fn table(&self, editing: bool) -> &Table {
        if editing { &self.results } else { &self.view }
    }

    /// The header of a table: buttons that sort it for this visit.
    fn fill_header(self: &Rc<Self>, editing: bool) {
        let header = &self.table(editing).header;
        while let Some(child) = header.first_child() {
            header.remove(&child);
        }
        let sorted = self.sorted.get();
        let rules_sort_added = self
            .rules
            .borrow()
            .sort
            .first()
            .is_some_and(|k| k.by == helikodu_model::query::SortBy::Field(Field::Added));
        for (i, (text, width, right)) in columns(editing).into_iter().enumerate() {
            let arrow = match sorted {
                Some((column, descending)) if column == i => {
                    Some(if descending { " ↓" } else { " ↑" })
                }
                None if text == tr("Added") && rules_sort_added => Some(" ↓"),
                _ => None,
            };
            let l = label(
                &format!("{text}{}", arrow.unwrap_or("")),
                &["hk-section-label"],
            );
            if arrow.is_some() {
                l.add_css_class("hk-sort-key");
            }
            if right {
                l.set_xalign(1.0);
            }
            let b = gtk::Button::new();
            b.add_css_class("hk-header-button");
            b.set_child(Some(&l));
            if width > 0 {
                b.set_size_request(width, -1);
            } else {
                b.set_hexpand(true);
                l.set_max_width_chars(1);
            }
            if i == 0 {
                b.set_sensitive(false);
            }
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.sort_by(i);
                }
            });
            if i == 1 && !editing {
                // The rows' title cell starts with a cover.
                let cell = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                cell.set_hexpand(true);
                let room = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                room.set_size_request(24, -1);
                cell.append(&room);
                cell.append(&b);
                header.append(&cell);
            } else {
                header.append(&b);
            }
        }
    }

    fn factory(self: &Rc<Self>, editing: bool) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            line.add_css_class("hk-playlist-row");
            for (i, (_, width, right)) in columns(editing).into_iter().enumerate() {
                let mut classes = vec!["hk-t2"];
                if i == 0 {
                    classes = vec!["hk-t3", "hk-numeric"];
                } else if i == 1 {
                    classes = vec!["hk-medium"];
                } else if columns(editing)[i].0 == tr("Format") {
                    classes = vec!["hk-t3", "hk-mono-meta"];
                } else if right {
                    classes = vec!["hk-t2", "hk-numeric"];
                }
                let l = label("", &classes);
                if right {
                    l.set_xalign(1.0);
                }
                if width > 0 {
                    l.set_size_request(width, -1);
                } else {
                    l.set_hexpand(true);
                    l.set_max_width_chars(1);
                }
                if i == 1 && !editing {
                    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                    cell.set_hexpand(true);
                    let cover = CoverArt::new(24, 4.0);
                    cover.add_css_class("small");
                    cell.append(&cover);
                    cell.append(&l);
                    line.append(&cell);
                } else {
                    line.append(&l);
                }
            }
            item.set_child(Some(&TableCell::new(&line)));
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
            view.show_row(&cell, &track, editing);
        });
        factory
    }

    fn show_row(&self, cell: &TableCell, track: &TrackItem, editing: bool) {
        let Some(row) = track.row() else { return };
        let Some(line) = cell.child() else { return };
        let position = track.position() as usize;
        let added = self
            .tracks
            .borrow()
            .iter()
            .find(|t| t.row.key == row.key)
            .and_then(|t| t.added);
        let mut texts = vec![
            (position + 1).to_string(),
            row.title.clone(),
            row.artist.clone(),
        ];
        if !editing {
            texts.push(row.album_title.clone());
        }
        texts.push(format::short(&row.format));
        texts.push(date(added));
        if !editing {
            texts.push(row.duration.map(format::duration).unwrap_or_default());
        }
        let mut child = line.first_child();
        let mut i = 0;
        while let Some(widget) = child {
            child = widget.next_sibling();
            let text = texts.get(i).cloned().unwrap_or_default();
            if let Some(l) = widget.downcast_ref::<gtk::Label>() {
                l.set_text(&text);
            } else if let Some(cell_box) = widget.downcast_ref::<gtk::Box>() {
                if let Some(cover) = cell_box.first_child().and_downcast::<CoverArt>() {
                    self.covers.show(&cover, row.cover, row.cover_seed, 24);
                }
                if let Some(l) = cell_box.last_child().and_downcast::<gtk::Label>() {
                    l.set_text(&text);
                }
            }
            i += 1;
        }
        if row.missing {
            cell.add_css_class("hk-missing");
        } else {
            cell.remove_css_class("hk-missing");
        }
    }

    fn connect(self: &Rc<Self>, play: &gtk::Button, shuffle: &gtk::Button, more: &gtk::MenuButton) {
        for (b, random) in [(play, false), (shuffle, true)] {
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.play_from(0, random);
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.edit.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.set_editing(!view.editing.get());
            }
        });
        let weak = Rc::downgrade(self);
        more.set_create_popup_func(move |more| {
            let Some(view) = weak.upgrade() else { return };
            let (Some(window), Some(key)) = (view.window(), view.key.get()) else {
                return;
            };
            more.set_menu_model(Some(&window.playlist_menu_model(key)));
        });
        for editing in [false, true] {
            let weak = Rc::downgrade(self);
            let list = &self.table(editing).list;
            list.connect_activate(move |_, position| {
                if let Some(view) = weak.upgrade() {
                    view.play_from(position as usize, false);
                }
            });
            let click = gtk::GestureClick::new();
            click.set_button(gdk::BUTTON_SECONDARY);
            let weak = Rc::downgrade(self);
            click.connect_pressed(move |gesture, _, x, y| {
                let Some(view) = weak.upgrade() else { return };
                let table = view.table(editing);
                if let Some(position) = cell_at(&table.list, x, y).and_then(|c| c.position()) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    if !table.selection.is_selected(position) {
                        table.selection.select_item(position, true);
                    }
                    if let Some(target) = view.target_in(editing) {
                        actions::popup(&table.list, target, Some((x, y)));
                    }
                }
            });
            list.add_controller(click);
            let keys = gtk::EventControllerKey::new();
            let weak = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, state| {
                let Some(view) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if actions::is_menu_key(key, state)
                    && let Some(target) = view.target_in(editing)
                {
                    actions::popup(&view.table(editing).list, target, None);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            list.add_controller(keys);
        }
        let weak = Rc::downgrade(self);
        self.view
            .selection
            .connect_selection_changed(move |_, _, _| {
                if let Some(view) = weak.upgrade() {
                    view.show_status();
                }
            });

        // The rule builder.
        let weak = Rc::downgrade(self);
        self.match_any.connect_selected_notify(move |d| {
            if let Some(view) = weak.upgrade()
                && !view.filling.get()
            {
                view.rules.borrow_mut().any = d.selected() == 1;
                view.rules_changed(false, true);
            }
        });
        let weak = Rc::downgrade(self);
        self.limit_on.connect_toggled(move |_| {
            if let Some(view) = weak.upgrade()
                && !view.filling.get()
            {
                view.limit_changed();
            }
        });
        let weak = Rc::downgrade(self);
        self.limit_unit.connect_selected_notify(move |_| {
            if let Some(view) = weak.upgrade()
                && !view.filling.get()
            {
                view.limit_changed();
            }
        });
        let weak = Rc::downgrade(self);
        self.limit_amount.connect_activate(move |_| {
            if let Some(view) = weak.upgrade() {
                view.limit_changed();
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        focus.connect_leave(move |_| {
            if let Some(view) = weak.upgrade() {
                view.limit_changed();
            }
        });
        self.limit_amount.add_controller(focus);
        let weak = Rc::downgrade(self);
        self.sort.connect_selected_notify(move |d| {
            let Some(view) = weak.upgrade() else { return };
            if view.filling.get() {
                return;
            }
            if let Some((_, _, keys)) = orders().into_iter().nth(d.selected() as usize) {
                view.rules.borrow_mut().sort = keys;
                view.rules_changed(false, true);
            }
        });
        let weak = Rc::downgrade(self);
        self.text_button.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.toggle_text();
            }
        });
        let weak = Rc::downgrade(self);
        self.text.connect_activate(move |entry| {
            if let Some(view) = weak.upgrade() {
                view.text_entered(&entry.text());
            }
        });
    }

    fn window(&self) -> Option<crate::window::MainWindow> {
        self.root.root().and_downcast::<crate::window::MainWindow>()
    }

    /// Shows the smart playlist `key`, viewing or editing its rules.
    pub(crate) fn show(self: &Rc<Self>, key: PlaylistKey, editing: bool) {
        if self.key.replace(Some(key)) != Some(key) {
            self.sorted.set(None);
            self.view.store.remove_all();
            self.results.store.remove_all();
            self.tracks.borrow_mut().clear();
            self.name.set_text("");
            self.summary.set_text("");
            self.problem.set_visible(false);
            self.text.set_visible(false);
            self.load_rules();
        }
        self.set_editing(editing);
        self.reload();
    }

    /// The smart playlist shown.
    pub(crate) fn key(&self) -> Option<PlaylistKey> {
        self.key.get()
    }

    /// Switches between viewing and editing the rules.
    pub(crate) fn set_editing(self: &Rc<Self>, editing: bool) {
        self.editing.set(editing);
        self.stack
            .set_visible_child_name(if editing { "edit" } else { "view" });
        let (text, icon) = if editing {
            (tr("Done editing"), "object-select-symbolic")
        } else {
            (tr("Edit rules"), "document-edit-symbolic")
        };
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.append(&gtk::Image::from_icon_name(icon));
        line.append(&gtk::Label::new(Some(&text)));
        self.edit.set_child(Some(&line));
        self.edit
            .update_property(&[gtk::accessible::Property::Label(&text)]);
        if editing {
            self.edit.add_css_class("hk-pressed");
        } else {
            self.edit.remove_css_class("hk-pressed");
        }
        self.show_rows();
    }

    /// Whether the rules are being edited.
    pub(crate) fn is_editing(&self) -> bool {
        self.editing.get()
    }

    fn load_rules(self: &Rc<Self>) {
        let Some(key) = self.key.get() else { return };
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend.query(move |l| l.smart_query(key)).await;
            let Some(view) = weak.upgrade() else { return };
            if view.key.get() != Some(key) {
                return;
            }
            match result {
                Ok(query) => view.set_query(query),
                Err(error) => {
                    if let Some(window) = view.window() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    /// Takes `query` as the rules, without saving it.
    fn set_query(self: &Rc<Self>, query: Query) {
        match Rules::from_query(&query) {
            Some(rules) => {
                self.rules.replace(rules);
                self.text_only.set(false);
            }
            None => self.text_only.set(true),
        }
        self.query.replace(query);
        self.fill_header(false);
        self.fill_header(true);
        self.fill_controls();
        self.show_rule_line();
        self.count_matches();
    }

    /// Fills the builder's controls from the rules.
    fn fill_controls(self: &Rc<Self>) {
        self.filling.set(true);
        let rules = self.rules.borrow().clone();
        self.match_any.set_selected(u32::from(rules.any));
        self.limit_on.set_active(rules.limit.is_some());
        let limit = rules.limit.unwrap_or(Limit::tracks(100));
        self.limit_amount.set_text(&limit.amount.to_string());
        let unit = UNITS.iter().position(|u| *u == limit.unit).unwrap_or(0);
        self.limit_unit
            .set_selected(u32::try_from(unit).unwrap_or(0));
        let order = orders()
            .iter()
            .position(|(_, _, keys)| *keys == rules.sort)
            .unwrap_or(orders().len());
        self.sort.set_selected(u32::try_from(order).unwrap_or(0));
        self.readout.set_text(&print_query(&self.query.borrow()));
        for widget in [
            self.match_any.upcast_ref::<gtk::Widget>(),
            self.limit_on.upcast_ref(),
            self.limit_amount.upcast_ref(),
            self.limit_unit.upcast_ref(),
            self.sort.upcast_ref(),
        ] {
            widget.set_sensitive(!self.text_only.get());
        }
        self.filling.set(false);
        self.build_rule_rows();
    }

    /// Builds the rows of the rules pane.
    fn build_rule_rows(self: &Rc<Self>) {
        while let Some(child) = self.rules_box.first_child() {
            self.rules_box.remove(&child);
        }
        if self.text_only.get() {
            let note = label(&tr("These rules can only be edited as text."), &["hk-t2"]);
            note.set_wrap(true);
            self.rules_box.append(&note);
            return;
        }
        let nodes = self.rules.borrow().nodes.clone();
        for (n, node) in nodes.iter().enumerate() {
            match node {
                Node::Rule(rule) => {
                    let row = self.rule_row(rule, n, None);
                    self.rules_box.append(&row);
                }
                Node::Group { any, rules } => {
                    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    head.add_css_class("hk-rule-row");
                    let kind = dropdown(&[tr("all"), tr("any")], usize::from(*any), 72);
                    kind.update_property(&[gtk::accessible::Property::Label(&tr(
                        "Match in group",
                    ))]);
                    let weak = Rc::downgrade(self);
                    kind.connect_selected_notify(move |d| {
                        if let Some(view) = weak.upgrade()
                            && let Some(Node::Group { any, .. }) =
                                view.rules.borrow_mut().nodes.get_mut(n)
                        {
                            *any = d.selected() == 1;
                        }
                        if let Some(view) = weak.upgrade() {
                            view.rules_changed(false, true);
                        }
                    });
                    head.append(&kind);
                    let words = label(&tr("of these"), &[]);
                    words.set_hexpand(true);
                    head.append(&words);
                    head.append(&self.remove_button(n, None));
                    self.rules_box.append(&head);
                    let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
                    inner.add_css_class("hk-rule-group");
                    for (r, rule) in rules.iter().enumerate() {
                        inner.append(&self.rule_row(rule, n, Some(r)));
                    }
                    let add = gtk::Button::new();
                    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    line.append(&gtk::Image::from_icon_name("list-add-symbolic"));
                    line.append(&gtk::Label::new(Some(&tr("Add rule to group"))));
                    add.set_child(Some(&line));
                    add.add_css_class("hk-link-button");
                    add.set_halign(gtk::Align::Start);
                    let weak = Rc::downgrade(self);
                    add.connect_clicked(move |_| {
                        let Some(view) = weak.upgrade() else { return };
                        if let Some(Node::Group { rules, .. }) =
                            view.rules.borrow_mut().nodes.get_mut(n)
                        {
                            rules.push(Rule::new(Field::Genre));
                        }
                        view.rules_changed(true, false);
                    });
                    inner.append(&add);
                    self.rules_box.append(&inner);
                }
            }
        }
        let adds = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let add_rule = small(&tr("Add rule"), "list-add-symbolic");
        let weak = Rc::downgrade(self);
        add_rule.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.rules
                    .borrow_mut()
                    .nodes
                    .push(Node::Rule(Rule::new(Field::Artist)));
                view.rules_changed(true, false);
            }
        });
        adds.append(&add_rule);
        let add_group = small(&tr("Add group"), "list-add-symbolic");
        let weak = Rc::downgrade(self);
        add_group.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.rules.borrow_mut().nodes.push(Node::Group {
                    any: true,
                    rules: vec![Rule::new(Field::Genre)],
                });
                view.rules_changed(true, false);
            }
        });
        adds.append(&add_group);
        self.rules_box.append(&adds);
    }

    /// Changes the rule at `node` (and `inner` in a group) with `change`.
    fn change_rule(&self, node: usize, inner: Option<usize>, change: impl FnOnce(&mut Rule)) {
        let mut rules = self.rules.borrow_mut();
        let rule = match (rules.nodes.get_mut(node), inner) {
            (Some(Node::Rule(rule)), None) => Some(rule),
            (Some(Node::Group { rules, .. }), Some(i)) => rules.get_mut(i),
            _ => None,
        };
        if let Some(rule) = rule {
            change(rule);
        }
    }

    fn remove_button(self: &Rc<Self>, node: usize, inner: Option<usize>) -> gtk::Button {
        let remove = gtk::Button::from_icon_name("list-remove-symbolic");
        remove.add_css_class("hk-icon");
        remove.set_tooltip_text(Some(&tr("Remove")));
        remove.update_property(&[gtk::accessible::Property::Label(&tr("Remove"))]);
        let weak = Rc::downgrade(self);
        remove.connect_clicked(move |_| {
            let Some(view) = weak.upgrade() else { return };
            {
                let mut rules = view.rules.borrow_mut();
                match inner {
                    None if node < rules.nodes.len() => {
                        rules.nodes.remove(node);
                    }
                    Some(i) => {
                        if let Some(Node::Group { rules: group, .. }) = rules.nodes.get_mut(node)
                            && i < group.len()
                        {
                            group.remove(i);
                            if group.is_empty() {
                                rules.nodes.remove(node);
                            }
                        }
                    }
                    None => {}
                }
            }
            view.rules_changed(true, true);
        });
        remove
    }

    fn rule_row(self: &Rc<Self>, rule: &Rule, node: usize, inner: Option<usize>) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("hk-rule-row");
        let mut fields: Vec<Field> = FIELDS.to_vec();
        if !fields.contains(&rule.field) {
            fields.push(rule.field.clone());
        }
        let names: Vec<String> = fields.iter().map(field_label).collect();
        let at = fields.iter().position(|f| *f == rule.field).unwrap_or(0);
        let field = dropdown(&names, at, 132);
        field.update_property(&[gtk::accessible::Property::Label(&tr("Field"))]);
        let weak = Rc::downgrade(self);
        field.connect_selected_notify(move |d| {
            let Some(view) = weak.upgrade() else { return };
            let Some(chosen) = fields.get(d.selected() as usize).cloned() else {
                return;
            };
            view.change_rule(node, inner, |rule| {
                let value = std::mem::take(&mut rule.value);
                let same_kind = rule.field.kind() == chosen.kind();
                *rule = Rule::new(chosen);
                if same_kind {
                    rule.value = value;
                }
            });
            view.rules_changed(true, true);
        });
        row.append(&field);
        let conds = Cond::for_kind(rule.field.kind());
        let cond_names: Vec<String> = conds.iter().map(|c| c.label_for(&rule.field)).collect();
        let at = conds.iter().position(|c| *c == rule.cond).unwrap_or(0);
        let cond = dropdown(&cond_names, at, 124);
        cond.update_property(&[gtk::accessible::Property::Label(&tr("Condition"))]);
        let weak = Rc::downgrade(self);
        cond.connect_selected_notify(move |d| {
            let Some(view) = weak.upgrade() else { return };
            let Some(chosen) = conds.get(d.selected() as usize).copied() else {
                return;
            };
            let rebuild = chosen.takes_value() != view.rule_takes_value(node, inner);
            view.change_rule(node, inner, |rule| rule.cond = chosen);
            view.rules_changed(rebuild, true);
        });
        row.append(&cond);
        if rule.cond.takes_value() {
            let value = gtk::Entry::new();
            value.add_css_class("hk-input");
            value.set_hexpand(true);
            value.set_text(&rule.value);
            value.update_property(&[gtk::accessible::Property::Label(&tr("Value"))]);
            if rule.expr().is_none() {
                value.add_css_class("error");
            }
            let weak = Rc::downgrade(self);
            value.connect_changed(move |entry| {
                let Some(view) = weak.upgrade() else { return };
                let text = entry.text().to_string();
                view.change_rule(node, inner, |rule| rule.value = text);
                let readable = view.rule_readable(node, inner);
                if readable {
                    entry.remove_css_class("error");
                } else {
                    entry.add_css_class("error");
                }
                view.rules_changed(false, false);
            });
            let weak = Rc::downgrade(self);
            value.connect_activate(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.save();
                }
            });
            let focus = gtk::EventControllerFocus::new();
            let weak = Rc::downgrade(self);
            focus.connect_leave(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.save();
                }
            });
            value.add_controller(focus);
            row.append(&value);
        } else {
            let room = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            room.set_hexpand(true);
            row.append(&room);
        }
        row.append(&self.remove_button(node, inner));
        row
    }

    fn rule_at(&self, node: usize, inner: Option<usize>) -> Option<Rule> {
        let rules = self.rules.borrow();
        match (rules.nodes.get(node), inner) {
            (Some(Node::Rule(rule)), None) => Some(rule.clone()),
            (Some(Node::Group { rules, .. }), Some(i)) => rules.get(i).cloned(),
            _ => None,
        }
    }

    fn rule_takes_value(&self, node: usize, inner: Option<usize>) -> bool {
        self.rule_at(node, inner)
            .is_some_and(|r| r.cond.takes_value())
    }

    fn rule_readable(&self, node: usize, inner: Option<usize>) -> bool {
        self.rule_at(node, inner)
            .is_some_and(|r| r.expr().is_some())
    }

    fn limit_changed(self: &Rc<Self>) {
        if self.filling.get() || self.text_only.get() {
            return;
        }
        let limit = if self.limit_on.is_active() {
            let amount = self.limit_amount.text().trim().parse::<u32>().ok();
            let unit = UNITS
                .get(self.limit_unit.selected() as usize)
                .copied()
                .unwrap_or(LimitUnit::Tracks);
            match amount {
                Some(amount) if amount > 0 => Some(Limit { amount, unit }),
                _ => {
                    self.limit_amount.add_css_class("error");
                    return;
                }
            }
        } else {
            None
        };
        self.limit_amount.remove_css_class("error");
        if self.rules.borrow().limit == limit {
            return;
        }
        self.rules.borrow_mut().limit = limit;
        self.rules_changed(false, true);
    }

    /// After a change of the rules: rebuilds the rows if asked, shows the
    /// text, and saves when `save` is set.
    fn rules_changed(self: &Rc<Self>, rebuild: bool, save: bool) {
        if rebuild {
            self.build_rule_rows();
        }
        if let Some(query) = self.rules.borrow().query() {
            self.readout.set_text(&print_query(&query));
            self.query.replace(query);
        }
        self.show_rule_line();
        if save {
            self.save();
        }
    }

    /// Saves the rules when they read as a query that differs from the saved one.
    fn save(self: &Rc<Self>) {
        let Some(key) = self.key.get() else { return };
        let query = if self.text_only.get() {
            self.query.borrow().clone()
        } else {
            match self.rules.borrow().query() {
                Some(query) => query,
                None => return,
            }
        };
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| {
                    if l.smart_query(key)? == query {
                        return Ok(false);
                    }
                    l.set_smart_query(key, &query).map(|()| true)
                })
                .await;
            let Some(view) = weak.upgrade() else { return };
            match result {
                Ok(true) => {
                    view.reload();
                    if let Some(window) = view.window() {
                        window.playlists_changed(None);
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    if let Some(window) = view.window() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    fn toggle_text(self: &Rc<Self>) {
        let editing = !WidgetExt::is_visible(&self.text);
        self.text.set_visible(editing);
        self.readout.set_visible(!editing);
        self.text_button.set_label(&if editing {
            tr("Show as rules")
        } else {
            tr("Edit as text")
        });
        if editing {
            self.text.set_text(&print_query(&self.query.borrow()));
            self.text.grab_focus();
        } else {
            self.problem.set_visible(false);
        }
    }

    /// Reads the text typed in the readout; text that cannot be read keeps
    /// the last rules and says why.
    fn text_entered(self: &Rc<Self>, text: &str) {
        match parse_query(text) {
            Ok(query) => {
                if let Some((_, problem)) = query.problem() {
                    self.problem.set_text(&fill(
                        &tr("These rules cannot run: {}"),
                        &[&problem.to_string()],
                    ));
                    self.problem.set_visible(true);
                    return;
                }
                self.problem.set_visible(false);
                self.set_query(query.normalized());
                self.save();
            }
            Err(error) => {
                self.problem.set_text(&fill(
                    &tr("The text cannot be read: {}"),
                    &[&error.to_string()],
                ));
                self.problem.set_visible(true);
            }
        }
    }

    fn show_rule_line(&self) {
        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        let rules = self.rules.borrow();
        if self.text_only.get() {
            let chip = gtk::Button::with_label(&print_query(&self.query.borrow()));
            chip.add_css_class("hk-rule-chip");
            chip.set_action_name(Some("smart-page.edit"));
            self.chips.append(&chip);
            self.order.set_text("");
            return;
        }
        for node in &rules.nodes {
            // Chips are text; activating one opens the editor.
            let chip = gtk::Button::with_label(&node.sentence());
            chip.add_css_class("hk-rule-chip");
            chip.set_action_name(Some("smart-page.edit"));
            self.chips.append(&chip);
        }
        if rules.nodes.is_empty() {
            self.chips
                .append(&label(&tr("Every track of the library"), &["hk-t2"]));
        }
        self.order.set_text(&rules.order_words());
    }

    /// Asks for how many tracks the rules select.
    fn count_matches(self: &Rc<Self>) {
        let query = self.query.borrow().clone();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend.query(move |l| l.query_match_count(&query)).await;
            let Some(view) = weak.upgrade() else { return };
            if let Ok((matched, total)) = result {
                view.show_count(matched, total);
            }
        });
    }

    fn show_count(&self, matched: usize, total: usize) {
        let shown = matched;
        self.matched.set_text(&fill(
            &ntr("{} track matches", "{} tracks match", shown),
            &[&format::count(shown)],
        ));
        self.of_total
            .set_text(&fill(&tr("of {}"), &[&format::count(total)]));
    }

    /// Fetches the playlist and its tracks again (after an edit, or when the
    /// library changed).
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
                    let query = l.smart_query(key)?;
                    let count = l.query_match_count(&query)?;
                    Ok((info, l.smart_tracks(key)?, count))
                })
                .await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            match result {
                Ok((info, tracks, (matched, total))) => {
                    view.show_info(info);
                    view.tracks.replace(tracks);
                    view.show_rows();
                    view.show_count(matched, total);
                }
                Err(error) => {
                    if let Some(window) = view.window() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    fn show_info(&self, info: Option<PlaylistRow>) {
        if let Some(info) = &info {
            self.name.set_text(&info.name);
            self.summary.set_text(
                &[
                    fill(
                        &ntr("{} track", "{} tracks", info.items),
                        &[&format::count(info.items)],
                    ),
                    format::minutes(info.duration),
                    tr("updates as the library changes"),
                ]
                .join(" · "),
            );
        }
        self.info.replace(info);
    }

    /// The tracks in the order shown: the rules' order, or the column the
    /// table is sorted by for this visit.
    fn shown(&self) -> Vec<SmartTrack> {
        let mut tracks = self.tracks.borrow().clone();
        let editing = self.editing.get();
        if let Some((column, descending)) = self.sorted.get() {
            let name = columns(editing)
                .get(column)
                .map(|c| c.0.clone())
                .unwrap_or_default();
            let key = |t: &SmartTrack| -> (String, Option<SystemTime>, Duration) {
                let text = if name == tr("Title") {
                    t.row.title.to_lowercase()
                } else if name == tr("Artist") {
                    t.row.artist.to_lowercase()
                } else if name == tr("Album") {
                    t.row.album_title.to_lowercase()
                } else if name == tr("Format") {
                    format::short(&t.row.format)
                } else {
                    String::new()
                };
                (text, t.added, t.row.duration.unwrap_or_default())
            };
            tracks.sort_by(|a, b| {
                let (ka, kb) = (key(a), key(b));
                let order = if name == tr("Added") {
                    ka.1.cmp(&kb.1)
                } else if name == tr("Length") {
                    ka.2.cmp(&kb.2)
                } else {
                    ka.0.cmp(&kb.0)
                };
                if descending { order.reverse() } else { order }
            });
        }
        tracks
    }

    fn sort_by(self: &Rc<Self>, column: usize) {
        let next = match self.sorted.get() {
            Some((c, false)) if c == column => Some((column, true)),
            Some((c, true)) if c == column => None,
            _ => Some((column, false)),
        };
        self.sorted.set(next);
        self.fill_header(self.editing.get());
        self.show_rows();
    }

    fn show_rows(&self) {
        let editing = self.editing.get();
        let table = self.table(editing);
        let items: Vec<TrackItem> = self
            .shown()
            .iter()
            .enumerate()
            .map(|(i, t)| TrackItem::new(t.row.key, i as u32, Some(Rc::new(t.row.clone()))))
            .collect();
        table.store.splice(0, table.store.n_items(), &items);
        self.show_status();
    }

    fn show_status(&self) {
        let tracks = self.tracks.borrow();
        let length: Duration = tracks.iter().filter_map(|t| t.row.duration).sum();
        let selected = selected_positions(&self.view.selection);
        let count = if selected.is_empty() {
            fill(
                &ntr("{} track", "{} tracks", tracks.len()),
                &[&format::count(tracks.len())],
            )
        } else {
            fill(
                &ntr("{} selected", "{} selected", selected.len()),
                &[&format::count(selected.len())],
            )
        };
        self.status_left
            .set_text(&fill(&tr("{} · {}"), &[&count, &format::minutes(length)]));
        self.status_right.set_text(&tr(
            "The rules set the order. To reorder by hand, save a copy as a playlist.",
        ));
    }

    fn play_from(&self, start: usize, shuffle: bool) {
        let tracks: Vec<TrackKey> = self
            .shown()
            .iter()
            .filter(|t| !t.row.missing)
            .map(|t| t.row.key)
            .collect();
        let title = self
            .info
            .borrow()
            .as_ref()
            .map(|i| i.name.clone())
            .unwrap_or_default();
        let start = start.min(tracks.len().saturating_sub(1));
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

    fn target_in(&self, editing: bool) -> Option<Target> {
        let table = self.table(editing);
        let positions = selected_positions(&table.selection);
        if positions.is_empty() {
            return None;
        }
        let shown = self.shown();
        let rows = positions
            .iter()
            .filter_map(|p| shown.get(*p as usize))
            .map(|t| Rc::new(t.row.clone()))
            .collect();
        Some(Target::tracks(rows))
    }

    /// The selected tracks as a context menu target.
    pub(crate) fn target(&self) -> Option<Target> {
        self.target_in(self.editing.get())
    }

    /// The anchor for asking a new name: the name in the hero.
    pub(crate) fn name_label(&self) -> gtk::Label {
        self.name.clone()
    }

    /// Installs the page's own actions (activating a chip edits the rules).
    pub(crate) fn install_actions(self: &Rc<Self>) {
        let group = gio::SimpleActionGroup::new();
        let edit = gio::SimpleAction::new("edit", None);
        let weak = Rc::downgrade(self);
        edit.connect_activate(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.set_editing(true);
            }
        });
        group.add_action(&edit);
        self.root.insert_action_group("smart-page", Some(&group));
    }

    /// The list, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.table(self.editing.get()).list.clone().upcast()
    }

    /// The rule line's chips and the query text (for tests).
    pub(crate) fn rule_texts(&self) -> (Vec<String>, String) {
        let mut chips = Vec::new();
        let mut child = self.chips.first_child();
        while let Some(widget) = child {
            if let Some(b) = widget.downcast_ref::<gtk::Button>() {
                chips.push(b.label().map(|l| l.to_string()).unwrap_or_default());
            }
            child = widget.next_sibling();
        }
        (chips, print_query(&self.query.borrow()))
    }

    /// The number of tracks listed (for tests).
    pub(crate) fn track_count(&self) -> usize {
        self.tracks.borrow().len()
    }

    /// Types `text` into the readout as if edited as text (for tests).
    pub(crate) fn enter_text(self: &Rc<Self>, text: &str) {
        self.text_entered(text);
    }
}
