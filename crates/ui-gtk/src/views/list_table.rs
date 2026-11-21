// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A table of tracks made by the library rather than by hand: the tracks of a
//! generator playlist and the tracks that sound alike. Rows 36 high (34 when
//! compact) with zebra stripes, a header whose buttons sort the table for the
//! visit, the playing mark, the track context menu, play on double click or
//! Enter, and dragging rows to the queue or a playlist.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::backend::TrackRow;
use crate::models::{CoverCache, TrackItem};
use crate::views::actions::{self, Target};
use crate::views::drag::DraggedTracks;
use crate::views::table::{PlayingRows, TableCell, cell_at, selected_positions};
use crate::widgets::{CoverArt, PlayingIndicator, label};

/// What a column shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColumnKind {
    /// The place in the list, or the playing mark.
    Number,
    /// The title after a 24 px cover.
    TitleWithCover,
    /// The title alone.
    Title,
    /// Text in `t2`.
    Text,
    /// A bar from 0 to 1 and a word.
    Likeness,
}

/// One column of a table.
#[derive(Debug, Clone)]
pub(crate) struct Column {
    /// Its name, which the value function receives.
    pub(crate) id: &'static str,
    /// Its header.
    pub(crate) title: String,
    /// Its width; 0 shares the room left in proportion to `share`.
    pub(crate) width: i32,
    /// Aligned to the right.
    pub(crate) right: bool,
    /// What it shows.
    pub(crate) kind: ColumnKind,
    /// The column the list is ordered by: its header in `t1`, with this arrow.
    pub(crate) arrow: Option<&'static str>,
}

impl Column {
    pub(crate) fn new(id: &'static str, title: String, width: i32, kind: ColumnKind) -> Self {
        Self {
            id,
            title,
            width,
            right: false,
            kind,
            arrow: None,
        }
    }

    pub(crate) fn right(mut self) -> Self {
        self.right = true;
        self
    }
}

/// The value of a cell.
pub(crate) enum Value {
    Text(String),
    /// The share of the bar, its word, and the accessible name.
    Likeness(f64, String, String),
}

type Values = Rc<dyn Fn(u32, &TrackRow, &'static str) -> Value>;

/// A table of tracks.
pub(crate) struct ListTable {
    pub(crate) header: gtk::Box,
    pub(crate) list: gtk::ListView,
    pub(crate) store: gio::ListStore,
    pub(crate) selection: gtk::MultiSelection,
    pub(crate) scroller: gtk::ScrolledWindow,
    columns: RefCell<Vec<Column>>,
    covers: Rc<CoverCache>,
    playing: Rc<PlayingRows>,
}

impl ListTable {
    /// An empty table; `name` is what a screen reader calls the list.
    pub(crate) fn new(
        name: &str,
        compact: bool,
        covers: &Rc<CoverCache>,
        playing: &Rc<PlayingRows>,
    ) -> Rc<Self> {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        header.add_css_class("hk-table-header");
        header.add_css_class("hk-playlist-row");
        let store = gio::ListStore::new::<TrackItem>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let list = gtk::ListView::new(Some(selection.clone()), None::<gtk::ListItemFactory>);
        list.set_tab_behavior(gtk::ListTabBehavior::Item);
        list.add_css_class("hk-playlist");
        if compact {
            list.add_css_class("compact");
        }
        list.set_vexpand(true);
        list.update_property(&[gtk::accessible::Property::Label(name)]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        Rc::new(Self {
            header,
            list,
            store,
            selection,
            scroller,
            columns: RefCell::new(Vec::new()),
            covers: covers.clone(),
            playing: playing.clone(),
        })
    }

    /// Shows `columns`, with `values` giving each cell; `sort` is called with
    /// the id of a header clicked.
    pub(crate) fn set_columns(
        self: &Rc<Self>,
        columns: Vec<Column>,
        values: impl Fn(u32, &TrackRow, &'static str) -> Value + 'static,
        sort: impl Fn(&'static str) + Clone + 'static,
    ) {
        while let Some(child) = self.header.first_child() {
            self.header.remove(&child);
        }
        for column in &columns {
            let text = format!("{}{}", column.title, column.arrow.unwrap_or(""));
            let l = label(&text, &["hk-section-label"]);
            if column.arrow.is_some() {
                l.add_css_class("hk-sort-key");
            }
            if column.right {
                l.set_xalign(1.0);
            }
            let b = gtk::Button::new();
            b.add_css_class("hk-header-button");
            b.set_child(Some(&l));
            if column.width > 0 {
                b.set_size_request(column.width, -1);
            } else {
                b.set_hexpand(true);
                l.set_max_width_chars(1);
            }
            if column.kind == ColumnKind::Number {
                b.set_sensitive(false);
            }
            let id = column.id;
            let sort = sort.clone();
            b.connect_clicked(move |_| sort(id));
            if column.kind == ColumnKind::TitleWithCover {
                let cell = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                cell.set_hexpand(true);
                let room = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                room.set_size_request(24, -1);
                cell.append(&room);
                cell.append(&b);
                self.header.append(&cell);
            } else {
                self.header.append(&b);
            }
        }
        self.columns.replace(columns);
        self.list.set_factory(Some(&self.factory(Rc::new(values))));
    }

    fn factory(self: &Rc<Self>, values: Values) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let columns = self.columns.borrow().clone();
        let weak = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            line.add_css_class("hk-playlist-row");
            for column in &columns {
                let widget: gtk::Widget = match column.kind {
                    ColumnKind::Number => {
                        let l = label("", &["hk-t3", "hk-numeric"]);
                        l.set_xalign(1.0);
                        let lead = gtk::Stack::new();
                        lead.add_named(&l, Some("number"));
                        let indicator = PlayingIndicator::default();
                        indicator.set_halign(gtk::Align::End);
                        lead.add_named(&indicator, Some("playing"));
                        lead.upcast()
                    }
                    ColumnKind::TitleWithCover => {
                        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                        let cover = CoverArt::new(24, 4.0);
                        cover.add_css_class("small");
                        cell.append(&cover);
                        let l = label("", &["hk-medium"]);
                        l.set_max_width_chars(1);
                        l.set_hexpand(true);
                        cell.append(&l);
                        cell.upcast()
                    }
                    ColumnKind::Likeness => {
                        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                        cell.add_css_class("hk-likeness");
                        let bar = gtk::LevelBar::for_interval(0.0, 1.0);
                        bar.add_css_class("hk-meter");
                        bar.set_mode(gtk::LevelBarMode::Continuous);
                        bar.set_size_request(56, -1);
                        bar.set_valign(gtk::Align::Center);
                        // The bar is drawn in t2, never in a warning colour.
                        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
                        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
                        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));
                        cell.append(&bar);
                        cell.append(&label("", &["hk-t2"]));
                        cell.upcast()
                    }
                    ColumnKind::Title => label("", &["hk-medium"]).upcast(),
                    ColumnKind::Text => {
                        let l = label("", &["hk-t2"]);
                        if column.right {
                            l.add_css_class("hk-numeric");
                            l.set_xalign(1.0);
                        }
                        l.upcast()
                    }
                };
                if column.width > 0 {
                    widget.set_size_request(column.width, -1);
                } else {
                    widget.set_hexpand(true);
                    if let Some(l) = widget.downcast_ref::<gtk::Label>() {
                        l.set_max_width_chars(1);
                    }
                }
                line.append(&widget);
            }
            let cell = TableCell::new(&line);
            if let Some(table) = weak.upgrade() {
                table.playing.add(&cell);
            }
            item.set_child(Some(&cell));
        });
        let weak = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(cell), Some(track), Some(table)) = (
                item.child().and_downcast::<TableCell>(),
                item.item().and_downcast::<TrackItem>(),
                weak.upgrade(),
            ) else {
                return;
            };
            cell.set_position(Some(track.position()));
            let w = Rc::downgrade(&table);
            let values = values.clone();
            cell.follow_named(item, &track, move |cell, track| {
                if let Some(table) = w.upgrade() {
                    table.show_row(cell, track, &values);
                }
            });
        });
        factory.connect_unbind(|_, item| {
            if let Some(cell) = item
                .downcast_ref::<gtk::ListItem>()
                .and_then(|i| i.child().and_downcast::<TableCell>())
            {
                cell.forget();
            }
        });
        factory
    }

    fn show_row(&self, cell: &TableCell, track: &TrackItem, values: &Values) {
        let Some(row) = track.row() else { return };
        let Some(line) = cell.child() else { return };
        let position = track.position();
        let playing = !row.missing && self.playing.contains(Some(row.key));
        cell.set_playing(playing);
        let columns = self.columns.borrow();
        let mut child = line.first_child();
        for column in columns.iter() {
            let Some(widget) = child else { break };
            child = widget.next_sibling();
            let value = match column.kind {
                ColumnKind::Number => Value::Text((position + 1).to_string()),
                _ => values(position, &row, column.id),
            };
            match (column.kind, value) {
                (ColumnKind::Number, Value::Text(text)) => {
                    if let Some(lead) = widget.downcast_ref::<gtk::Stack>() {
                        lead.set_visible_child_name(if playing { "playing" } else { "number" });
                        if let Some(l) = lead.child_by_name("number").and_downcast::<gtk::Label>() {
                            l.set_text(&text);
                        }
                    }
                }
                (ColumnKind::TitleWithCover, Value::Text(text)) => {
                    if let Some(cover) = widget.first_child().and_downcast::<CoverArt>() {
                        self.covers.show(&cover, row.cover, row.cover_seed, 24);
                    }
                    if let Some(l) = widget.last_child().and_downcast::<gtk::Label>() {
                        l.set_text(&text);
                    }
                }
                (ColumnKind::Likeness, Value::Likeness(share, word, name)) => {
                    if let Some(bar) = widget.first_child().and_downcast::<gtk::LevelBar>() {
                        bar.set_value(share.clamp(0.0, 1.0));
                    }
                    if let Some(l) = widget.last_child().and_downcast::<gtk::Label>() {
                        l.set_text(&word);
                    }
                    widget.update_property(&[gtk::accessible::Property::Label(&name)]);
                }
                (_, Value::Text(text)) => {
                    if let Some(l) = widget.downcast_ref::<gtk::Label>() {
                        l.set_text(&text);
                    }
                }
                _ => {}
            }
        }
    }

    /// Shows `rows`, in order.
    pub(crate) fn show(&self, rows: &[TrackRow]) {
        let items: Vec<TrackItem> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| TrackItem::new(r.key, i as u32, Some(Rc::new(r.clone()))))
            .collect();
        self.store.splice(0, self.store.n_items(), &items);
    }

    /// The selected rows, in list order.
    pub(crate) fn selected_rows(&self) -> Vec<Rc<TrackRow>> {
        selected_positions(&self.selection)
            .into_iter()
            .filter_map(|p| self.store.item(p).and_downcast::<TrackItem>())
            .filter_map(|item| item.row())
            .collect()
    }

    /// The headers (for tests).
    pub(crate) fn header_texts(&self) -> Vec<String> {
        self.columns
            .borrow()
            .iter()
            .map(|c| format!("{}{}", c.title, c.arrow.unwrap_or("")))
            .collect()
    }

    /// Plays on activation, opens the context menu on a right click or the
    /// menu key, and drags the selected rows.
    pub(crate) fn connect(self: &Rc<Self>, play: impl Fn(usize) + 'static) {
        self.list
            .connect_activate(move |_, position| play(position as usize));
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(table) = weak.upgrade() else { return };
            if let Some(position) = cell_at(&table.list, x, y).and_then(|c| c.position()) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if !table.selection.is_selected(position) {
                    table.selection.select_item(position, true);
                }
                table.popup(Some((x, y)));
            }
        });
        self.list.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(table) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if actions::is_menu_key(key, state) {
                table.popup(None);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.list.add_controller(keys);
        let weak = Rc::downgrade(self);
        DraggedTracks::source(&self.list, move |x, y| {
            let table = weak.upgrade()?;
            let position = cell_at(&table.list, x, y)?.position()?;
            if !table.selection.is_selected(position) {
                table.selection.select_item(position, true);
            }
            let rows = table.selected_rows();
            let label = match rows.as_slice() {
                [one] => one.title.clone(),
                _ => crate::i18n::fill(
                    &crate::i18n::ntr("{} track", "{} tracks", rows.len()),
                    &[&crate::format::count(rows.len())],
                ),
            };
            Some((
                DraggedTracks {
                    keys: rows.iter().map(|r| r.key).collect(),
                    ..DraggedTracks::default()
                },
                label,
            ))
        });
    }

    /// The selected tracks as a context menu target.
    pub(crate) fn target(&self) -> Option<Target> {
        let rows = self.selected_rows();
        (!rows.is_empty()).then(|| Target::tracks(rows))
    }

    fn popup(&self, at: Option<(f64, f64)>) {
        if let Some(target) = self.target() {
            actions::popup(&self.list, target, at);
        }
    }
}

/// Marks the playing rows of the tables that share `playing`.
pub(crate) fn follow_playing(playing: &Rc<PlayingRows>, playback: &Rc<crate::playback::Playback>) {
    let weak = Rc::downgrade(playing);
    playback.connect(move |player, change| {
        if let Some(playing) = weak.upgrade()
            && change == crate::playback::Change::Playing
        {
            playing.update(player.playing, |cell| {
                // The cell's own `follow` callback shows it again.
                if let Some(item) = cell.item() {
                    item.emit_by_name::<()>("loaded", &[]);
                }
            });
        }
    });
}
