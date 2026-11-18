// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library, Tracks: the toolbar (title, count, filter, columns), the track
//! table over a [`TrackListModel`], and the status bar.
//!
//! The table is a `GtkColumnView` whose columns can be shown, hidden, moved and
//! resized. A header click sorts by that column (ascending, descending, off);
//! the keys clicked before stay as further keys. Sorting and filtering run as
//! queries in the backend, which returns the keys in the new order; the model
//! never sorts. Cells are one drawn widget each ([`TableCell`]); the rating
//! stars, which set the rating with a click, and the playing indicator are
//! their only children.
//!
//! The same view, without its toolbar, lists the tracks of an artist.

mod edit;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::backend::{Backend, TrackColumn, TrackKey, TrackQuery, TrackRow, TrackScope, TrackSort};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::{TrackItem, TrackListModel};
use crate::playback::{Change, Playback};
use crate::views::actions::{self, Target};
use crate::views::drag::DraggedTracks;
use crate::views::facet_browser::FacetBrowser;
use crate::views::table::{PlayingRows, TableCell, cell_at, selected_positions};
use crate::widgets::{PlayingIndicator, RatingStars, label};

/// How many tracks from the activated one a play request takes along.
const PLAY_WINDOW: usize = 1000;

/// The columns, their default visibility, fixed width, and whether they expand.
const COLUMNS: &[(TrackColumn, bool, i32, bool)] = &[
    (TrackColumn::Number, true, 58, false),
    (TrackColumn::Title, true, 220, true),
    (TrackColumn::Artist, true, 120, true),
    (TrackColumn::Album, true, 160, true),
    (TrackColumn::Genre, false, 104, false),
    (TrackColumn::Year, true, 58, false),
    (TrackColumn::Format, true, 120, false),
    (TrackColumn::Rating, true, 86, false),
    (TrackColumn::Plays, false, 62, false),
    (TrackColumn::Added, false, 96, false),
    (TrackColumn::Length, true, 80, false),
];

/// The name of a column as its header shows it.
pub(crate) fn column_title(column: TrackColumn) -> String {
    match column {
        TrackColumn::Number => "#".to_owned(),
        TrackColumn::Title => tr("Title"),
        TrackColumn::Artist => tr("Artist"),
        TrackColumn::Album => tr("Album"),
        TrackColumn::Year => tr("Year"),
        TrackColumn::Format => tr("Format"),
        TrackColumn::Rating => tr("Rating"),
        TrackColumn::Plays => tr("Plays"),
        TrackColumn::Length => tr("Length"),
        TrackColumn::Genre => tr("Genre"),
        TrackColumn::Added => tr("Added"),
    }
}

/// A short machine name for a column, for action names.
fn column_id(column: TrackColumn) -> &'static str {
    match column {
        TrackColumn::Number => "number",
        TrackColumn::Title => "title",
        TrackColumn::Artist => "artist",
        TrackColumn::Album => "album",
        TrackColumn::Year => "year",
        TrackColumn::Format => "format",
        TrackColumn::Rating => "rating",
        TrackColumn::Plays => "plays",
        TrackColumn::Length => "length",
        TrackColumn::Genre => "genre",
        TrackColumn::Added => "added",
    }
}

/// The text of a plain cell.
fn cell_text(row: &TrackRow, column: TrackColumn) -> String {
    match column {
        TrackColumn::Number => row.number.map(|n| n.to_string()).unwrap_or_default(),
        TrackColumn::Title => row.title.clone(),
        TrackColumn::Artist => row.artist.clone(),
        TrackColumn::Album => row.album_title.clone(),
        TrackColumn::Year => row.year.map(|y| y.to_string()).unwrap_or_default(),
        TrackColumn::Format => format::short(&row.format),
        TrackColumn::Rating => String::new(),
        TrackColumn::Plays => row.plays.to_string(),
        TrackColumn::Length => row.duration.map(format::duration).unwrap_or_default(),
        TrackColumn::Genre => row.genre.clone(),
        TrackColumn::Added => crate::tools::tool_window::date(row.added),
    }
}

/// The new key list after a header click made `column` the primary key in
/// `descending` order: a new column goes first; the first column turns from
/// ascending to descending, and from descending to off.
pub(crate) fn next_sort(
    keys: &[TrackSort],
    column: TrackColumn,
    descending: bool,
) -> Vec<TrackSort> {
    let mut keys: Vec<TrackSort> = keys.to_vec();
    if let Some(first) = keys.first()
        && first.column == column
    {
        if first.descending && !descending {
            keys.remove(0);
        } else {
            keys[0].descending = descending;
        }
        return keys;
    }
    keys.retain(|k| k.column != column);
    keys.insert(0, TrackSort { column, descending });
    keys.truncate(4);
    keys
}

/// What is called with the number of tracks after a load.
type Loaded = Box<dyn Fn(usize)>;

/// The track table and its toolbar.
pub(crate) struct TracksView {
    pub(crate) root: gtk::Box,
    count: gtk::Label,
    filter: gtk::Entry,
    table: gtk::ColumnView,
    selection: gtk::MultiSelection,
    model: TrackListModel,
    status_selection: gtk::Label,
    status_sort: gtk::Label,
    columns: Vec<(TrackColumn, gtk::ColumnViewColumn)>,
    backend: Backend,
    scope: Cell<TrackScope>,
    sort: RefCell<Vec<TrackSort>>,
    total: Cell<usize>,
    generation: Cell<u64>,
    filter_generation: Cell<u64>,
    syncing: Cell<bool>,
    playing: PlayingRows,
    cells: RefCell<Vec<glib::WeakRef<TableCell>>>,
    title: RefCell<String>,
    loaded: Cell<bool>,
    /// Set while the filter text is changed from code, which reloads at once.
    quiet: Cell<bool>,
    /// Whether the order without keys of the user's goes unmarked (it is the
    /// query's own).
    unmarked: Cell<bool>,
    /// Called with the number of tracks after each load.
    on_loaded: RefCell<Option<Loaded>>,
    /// The cell being edited (F2).
    editing: RefCell<Option<edit::CellEdit>>,
    /// The column and row of the last click, where F2 edits.
    clicked_column: Cell<Option<TrackColumn>>,
    clicked_row: Cell<Option<u32>>,
    /// The facet browser and its button (not on the artist page).
    browser: Option<Rc<FacetBrowser>>,
    browse: gtk::ToggleButton,
}

impl std::fmt::Debug for TracksView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TracksView")
            .field("sort", &self.sort)
            .finish_non_exhaustive()
    }
}

impl TracksView {
    /// The view over `backend`; `embedded` leaves out the toolbar (on the
    /// artist page).
    pub(crate) fn new(backend: &Backend, playback: &Rc<Playback>, embedded: bool) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-tracks-view");
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        toolbar.add_css_class("hk-toolbar");
        let title = label(&tr("Tracks"), &["hk-view-title"]);
        title.set_hexpand(false);
        title.set_accessible_role(gtk::AccessibleRole::Heading);
        toolbar.append(&title);
        let count = label("", &["hk-t3", "hk-numeric"]);
        count.set_valign(gtk::Align::Baseline);
        count.set_hexpand(true);
        toolbar.append(&count);
        // The facet browser's button, before the filter.
        let browse = gtk::ToggleButton::new();
        browse.add_css_class("hk-browse");
        browse.set_valign(gtk::Align::Center);
        {
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            content.append(&gtk::Image::from_icon_name("helikodu-browse-symbolic"));
            content.append(&gtk::Label::new(Some(&tr("Browse"))));
            browse.set_child(Some(&content));
        }
        browse.set_tooltip_text(Some(&tr("Browse by genre, artist and album")));
        browse.update_property(&[gtk::accessible::Property::Label(&tr("Browse"))]);
        toolbar.append(&browse);
        let filter = gtk::Entry::new();
        filter.add_css_class("hk-input");
        filter.add_css_class("hk-filter");
        filter.set_placeholder_text(Some(&tr("Filter")));
        filter.set_primary_icon_name(Some("helikodu-funnel-symbolic"));
        filter.set_width_chars(24);
        filter.set_valign(gtk::Align::Center);
        filter.update_property(&[gtk::accessible::Property::Label(&tr("Filter the tracks"))]);
        toolbar.append(&filter);
        let columns_button = gtk::MenuButton::new();
        columns_button.set_label(&tr("Columns"));
        columns_button.add_css_class("hk-sort");
        columns_button.set_valign(gtk::Align::Center);
        columns_button.set_tooltip_text(Some(&tr("Show or hide columns")));
        toolbar.append(&columns_button);
        let browser = (!embedded).then(|| FacetBrowser::new(backend));
        if !embedded {
            root.append(&toolbar);
        }
        if let Some(browser) = &browser {
            browser.root.set_visible(false);
            root.append(&browser.root);
        }

        let model = TrackListModel::new(backend);
        let selection = gtk::MultiSelection::new(Some(model.clone()));
        let table = gtk::ColumnView::new(Some(selection.clone()));
        // Tab leaves the list after the focused row, instead of visiting
        // every row: Tab and Shift+Tab reach the rest of the window.
        table.set_tab_behavior(gtk::ListTabBehavior::Item);
        table.add_css_class("hk-table");
        table.set_reorderable(true);
        table.set_show_row_separators(false);
        table.set_show_column_separators(false);
        table.set_vexpand(true);
        table.update_property(&[gtk::accessible::Property::Label(&tr("Tracks"))]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&table));
        root.append(&scroller);

        let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        status.add_css_class("hk-statusbar");
        let status_selection = label("", &[]);
        status_selection.set_hexpand(true);
        status.append(&status_selection);
        let status_sort = label("", &[]);
        status_sort.set_xalign(1.0);
        status.append(&status_sort);
        root.append(&status);

        let view = Rc::new_cyclic(|weak: &std::rc::Weak<Self>| {
            let columns = COLUMNS
                .iter()
                .map(|(column, visible, width, expand)| {
                    let factory = Self::factory(weak.clone(), *column);
                    let c = gtk::ColumnViewColumn::new(Some(&column_title(*column)), Some(factory));
                    c.set_id(Some(column_id(*column)));
                    c.set_visible(*visible);
                    c.set_resizable(true);
                    c.set_fixed_width(*width);
                    c.set_expand(*expand);
                    if backend.library().sortable(*column) {
                        c.set_sorter(Some(&gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal)));
                    }
                    table.append_column(&c);
                    (*column, c)
                })
                .collect();
            Self {
                root,
                count,
                filter,
                table,
                selection,
                model,
                status_selection,
                status_sort,
                columns,
                backend: backend.clone(),
                scope: Cell::new(TrackScope::All),
                sort: RefCell::new(Vec::new()),
                total: Cell::new(0),
                generation: Cell::new(0),
                filter_generation: Cell::new(0),
                syncing: Cell::new(false),
                playing: PlayingRows::default(),
                cells: RefCell::new(Vec::new()),
                title: RefCell::new(tr("Tracks")),
                loaded: Cell::new(false),
                quiet: Cell::new(false),
                unmarked: Cell::new(false),
                on_loaded: RefCell::new(None),
                editing: RefCell::new(None),
                clicked_column: Cell::new(None),
                clicked_row: Cell::new(None),
                browser,
                browse,
            }
        });
        columns_button.set_menu_model(Some(&view.columns_menu()));
        crate::widgets::menu_names::name_menus(&columns_button);
        view.connect(playback);
        view.connect_edges();
        view.connect_browser();
        view.show_sort();
        view
    }

    /// The first and the last shown column, in the table's order.
    fn edge_columns(&self) -> (Option<TrackColumn>, Option<TrackColumn>) {
        let columns = self.table.columns();
        let shown: Vec<TrackColumn> = (0..columns.n_items())
            .filter_map(|i| columns.item(i).and_downcast::<gtk::ColumnViewColumn>())
            .filter(gtk::ColumnViewColumn::is_visible)
            .filter_map(|c| {
                self.columns
                    .iter()
                    .find(|(_, own)| *own == c)
                    .map(|(column, _)| *column)
            })
            .collect();
        (shown.first().copied(), shown.last().copied())
    }

    /// Keeps the cells of the first and the last shown column marked as the
    /// columns are shown, hidden and moved.
    fn connect_edges(self: &Rc<Self>) {
        let mark = {
            let weak = Rc::downgrade(self);
            move || {
                let Some(view) = weak.upgrade() else { return };
                let (first, last) = view.edge_columns();
                for cell in view
                    .cells
                    .borrow()
                    .iter()
                    .filter_map(glib::WeakRef::upgrade)
                {
                    let column = view
                        .columns
                        .iter()
                        .find(|(c, _)| cell.widget_name() == column_id(*c))
                        .map(|(c, _)| *c);
                    cell.set_edges(
                        column.is_some() && column == first,
                        column.is_some() && column == last,
                    );
                }
            }
        };
        for (_, c) in &self.columns {
            let mark = mark.clone();
            c.connect_visible_notify(move |_| mark());
        }
        self.table
            .columns()
            .connect_items_changed(move |_, _, _, _| mark());
    }

    fn connect_browser(self: &Rc<Self>) {
        let Some(browser) = &self.browser else {
            self.browse.set_visible(false);
            return;
        };
        let weak = Rc::downgrade(self);
        browser.connect_changed(move || {
            if let Some(view) = weak.upgrade() {
                view.reload();
            }
        });
        let weak = Rc::downgrade(self);
        self.browse.connect_toggled(move |button| {
            if let Some(view) = weak.upgrade() {
                view.show_browser(button.is_active());
            }
        });
    }

    fn show_browser(self: &Rc<Self>, shown: bool) {
        let Some(browser) = &self.browser else { return };
        if browser.root.is_visible() == shown {
            return;
        }
        browser.root.set_visible(shown);
        if shown {
            browser.set_text(self.filter.text().trim());
        }
        if browser.narrows() {
            self.reload();
        }
    }

    /// The facet browser when it is shown.
    fn shown_browser(&self) -> Option<&Rc<FacetBrowser>> {
        self.browser.as_ref().filter(|b| b.root.is_visible())
    }

    /// Opens or closes the facet browser (for tests).
    pub(crate) fn set_browsing(&self, shown: bool) {
        self.browse.set_active(shown);
    }

    /// The facet browser (for tests).
    pub(crate) fn facet_browser(&self) -> Option<Rc<FacetBrowser>> {
        self.browser.clone()
    }

    /// The number of rows and the count in the toolbar (for tests).
    pub(crate) fn shown_count(&self) -> (usize, String) {
        (self.model.n_items() as usize, self.count.text().to_string())
    }

    fn columns_menu(self: &Rc<Self>) -> gio::Menu {
        let group = gio::SimpleActionGroup::new();
        let menu = gio::Menu::new();
        for (column, c) in &self.columns {
            let name = column_id(*column);
            let action = gio::PropertyAction::new(name, c, "visible");
            group.add_action(&action);
            menu.append(
                Some(&column_title(*column)),
                Some(&format!("columns.{name}")),
            );
        }
        self.root.insert_action_group("columns", Some(&group));
        menu
    }

    fn factory(view: std::rc::Weak<Self>, column: TrackColumn) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let weak = view.clone();
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ColumnViewCell>() else {
                return;
            };
            let cell = match column {
                TrackColumn::Rating => {
                    let stars = RatingStars::default();
                    stars.set_halign(gtk::Align::Start);
                    TableCell::new(&stars)
                }
                // The playing indicator is added when the row plays.
                TrackColumn::Number => TableCell::text(1.0, &["hk-t3", "hk-numeric"]),
                TrackColumn::Year
                | TrackColumn::Plays
                | TrackColumn::Length
                | TrackColumn::Added => TableCell::text(1.0, &["hk-numeric", "hk-t2"]),
                TrackColumn::Format => TableCell::text(0.0, &["hk-mono-meta"]),
                _ => TableCell::text(0.0, &[]),
            };
            cell.set_widget_name(column_id(column));
            if let Some(view) = weak.upgrade() {
                let (first, last) = view.edge_columns();
                cell.set_edges(first == Some(column), last == Some(column));
                view.cells.borrow_mut().push(cell.downgrade());
                view.playing.add(&cell);
                if let Some(stars) = cell.child().and_downcast::<RatingStars>() {
                    let weak_view = Rc::downgrade(&view);
                    let weak_cell = cell.downgrade();
                    stars.set_editable(move |stars| {
                        let (Some(view), Some(cell)) = (weak_view.upgrade(), weak_cell.upgrade())
                        else {
                            return;
                        };
                        if let Some(item) = cell.item() {
                            view.rate(&item, stars);
                        }
                    });
                }
            }
            item.set_child(Some(&cell));
        });
        let weak = view.clone();
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ColumnViewCell>() else {
                return;
            };
            let (Some(cell), Some(track)) = (
                item.child().and_downcast::<TableCell>(),
                item.item().and_downcast::<TrackItem>(),
            ) else {
                return;
            };
            let Some(view) = weak.upgrade() else { return };
            cell.set_position(Some(track.position()));
            let w = Rc::downgrade(&view);
            cell.follow(&track, move |cell, track| {
                if let Some(view) = w.upgrade() {
                    view.show_cell(cell, track, column);
                }
            });
            view.model.ensure_row(&track);
        });
        let weak = view.clone();
        factory.connect_unbind(move |_, item| {
            if let Some(cell) = item
                .downcast_ref::<gtk::ColumnViewCell>()
                .and_then(|i| i.child().and_downcast::<TableCell>())
            {
                if let Some(view) = weak.upgrade() {
                    view.cell_unbound(&cell);
                }
                cell.forget();
                cell.set_position(None);
            }
        });
        factory
    }

    fn show_cell(&self, cell: &TableCell, track: &TrackItem, column: TrackColumn) {
        let row = track.row();
        let playing = self.playing.contains(Some(track.key()));
        cell.set_playing(playing);
        let text =
            |row: Option<&Rc<TrackRow>>| row.map(|r| cell_text(r, column)).unwrap_or_default();
        match column {
            TrackColumn::Rating => {
                if let Some(stars) = cell.child().and_downcast::<RatingStars>() {
                    stars.set_rating(row.as_ref().and_then(|r| r.rating));
                    stars.set_visible(row.is_some());
                }
            }
            TrackColumn::Number => {
                // The indicator replaces the number of the playing row.
                if playing && cell.child().is_none() {
                    let indicator = PlayingIndicator::default();
                    indicator.set_halign(gtk::Align::End);
                    cell.set_child(&indicator);
                }
                if let Some(indicator) = cell.child() {
                    indicator.set_visible(playing);
                }
                cell.set_text(&if playing {
                    String::new()
                } else {
                    text(row.as_ref())
                });
            }
            TrackColumn::Title => {
                cell.set_text(&text(row.as_ref()));
                match row.as_ref() {
                    Some(row) => cell.set_copies(row.key, row.copies),
                    None => cell.set_copies(TrackKey(0), 0),
                }
            }
            _ => cell.set_text(&text(row.as_ref())),
        }
        self.decorate_cell(cell, track.key(), column);
    }

    fn connect(self: &Rc<Self>, playback: &Rc<Playback>) {
        let weak = Rc::downgrade(self);
        playback.connect(move |view, change| {
            if let Some(this) = weak.upgrade()
                && change == Change::Playing
            {
                this.playing.update(view.playing, |cell| {
                    // The cell's own `follow` callback redraws it.
                    if let Some(item) = cell.item() {
                        item.emit_by_name::<()>("loaded", &[]);
                    }
                });
            }
        });
        let weak = Rc::downgrade(self);
        self.table.connect_activate(move |_, position| {
            if let Some(view) = weak.upgrade() {
                view.play_from(position as usize);
            }
        });
        let weak = Rc::downgrade(self);
        self.selection.connect_selection_changed(move |_, _, _| {
            if let Some(view) = weak.upgrade() {
                view.show_selection();
            }
        });
        if let Some(sorter) = self.table.sorter() {
            let weak = Rc::downgrade(self);
            sorter.connect_changed(move |sorter, _| {
                if let Some(view) = weak.upgrade() {
                    view.header_clicked(sorter);
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.filter.connect_changed(move |filter| {
            let Some(view) = weak.upgrade() else { return };
            crate::query_text::mark(filter, &crate::query_text::read(&filter.text()));
            if view.quiet.get() {
                return;
            }
            let generation = view.filter_generation.get() + 1;
            view.filter_generation.set(generation);
            let weak = Rc::downgrade(&view);
            glib::timeout_add_local_once(Duration::from_millis(180), move || {
                if let Some(view) = weak.upgrade()
                    && view.filter_generation.get() == generation
                {
                    if let Some(browser) = view.shown_browser() {
                        browser.set_text(view.filter.text().trim());
                    }
                    view.reload();
                }
            });
        });
        // The context menu: right click, the Menu key or Shift+F10.
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            if let Some(position) = cell_at(&view.table, x, y).and_then(|c| c.position()) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if !view.selection.is_selected(position) {
                    view.selection.select_item(position, true);
                }
                view.popup(Some((x, y)));
            }
        });
        self.table.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if actions::is_menu_key(key, state) {
                view.popup(None);
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::F2 && !view.is_editing() {
                view.start_edit();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.table.add_controller(keys);
        // Where F2 edits: the column and row last clicked.
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |_, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            if let Some(column) = view.column_at(x, y) {
                view.clicked_column.set(Some(column));
            }
            view.clicked_row
                .set(cell_at(&view.table, x, y).and_then(|c| c.position()));
        });
        self.table.add_controller(click);
        let weak = Rc::downgrade(self);
        DraggedTracks::source(&self.table, move |x, y| {
            let view = weak.upgrade()?;
            let position = cell_at(&view.table, x, y)?.position()?;
            if !view.selection.is_selected(position) {
                view.selection.select_item(position, true);
            }
            let keys = view.selected_keys();
            let label = match keys.len() {
                1 => view
                    .model
                    .cached(keys[0])
                    .map(|r| r.title.clone())
                    .unwrap_or_default(),
                n => fill(&ntr("{} track", "{} tracks", n), &[&format::count(n)]),
            };
            Some((
                DraggedTracks {
                    keys,
                    ..DraggedTracks::default()
                },
                label,
            ))
        });
    }

    /// The keys of the selected rows, in order.
    fn selected_keys(&self) -> Vec<TrackKey> {
        let keys = self.model.keys();
        selected_positions(&self.selection)
            .into_iter()
            .filter_map(|p| keys.get(p as usize).copied())
            .collect()
    }

    /// The selection as a context menu target.
    pub(crate) fn target(&self) -> Option<Target> {
        let keys = self.selected_keys();
        if keys.is_empty() {
            return None;
        }
        let rows = keys
            .iter()
            .map(|k| {
                self.model.cached(*k).unwrap_or_else(|| {
                    Rc::new(TrackRow {
                        key: *k,
                        ..TrackRow::default()
                    })
                })
            })
            .collect();
        Some(Target::tracks(rows))
    }

    fn popup(&self, at: Option<(f64, f64)>) {
        if let Some(target) = self.target() {
            actions::popup(&self.table, target, at);
        }
    }

    fn header_clicked(self: &Rc<Self>, sorter: &gtk::Sorter) {
        if self.syncing.get() {
            return;
        }
        let Some(sorter) = sorter.downcast_ref::<gtk::ColumnViewSorter>() else {
            return;
        };
        let Some(primary) = sorter.primary_sort_column() else {
            return;
        };
        let Some(column) = self
            .columns
            .iter()
            .find(|(_, c)| *c == primary)
            .map(|(column, _)| *column)
        else {
            return;
        };
        let descending = sorter.primary_sort_order() == gtk::SortType::Descending;
        let keys = next_sort(&self.sort.borrow(), column, descending);
        if keys.first().map(|k| k.column) != Some(column) {
            // The column was turned off: start the header's cycle again.
            self.syncing.set(true);
            self.table.sort_by_column(None, gtk::SortType::Ascending);
            self.syncing.set(false);
        }
        self.sort.replace(keys);
        self.show_sort();
        self.reload();
    }

    /// Sorts by `keys` as if their headers had been clicked (for tests).
    pub(crate) fn set_sort(self: &Rc<Self>, keys: Vec<TrackSort>) {
        self.sort.replace(keys);
        self.show_sort();
        self.reload();
    }

    /// The headers carry an arrow and, with several keys, the key's number.
    fn show_sort(&self) {
        let keys = self.sort.borrow();
        // Without keys of the user's the library sorts by artist, album, disc
        // and number; the first two have columns and carry their marks.
        let marks: Vec<(TrackColumn, bool)> = if keys.is_empty() && self.unmarked.get() {
            Vec::new()
        } else if keys.is_empty() {
            vec![(TrackColumn::Artist, false), (TrackColumn::Album, false)]
        } else {
            keys.iter().map(|k| (k.column, k.descending)).collect()
        };
        let numbered = keys.is_empty() || keys.len() > 1;
        for (column, c) in &self.columns {
            let mut title = column_title(*column);
            if let Some(index) = marks.iter().position(|(m, _)| m == column) {
                let arrow = if marks[index].1 { "↓" } else { "↑" };
                title = if numbered {
                    format!("{title} {arrow} {}", index + 1)
                } else {
                    format!("{title} {arrow}")
                };
            }
            c.set_title(Some(&title));
        }
        let names: Vec<String> = if keys.is_empty() {
            vec![tr("Artist"), tr("Album"), tr("Disc"), "#".to_owned()]
        } else {
            keys.iter().map(|k| column_title(k.column)).collect()
        };
        self.status_sort
            .set_text(&fill(&tr("Sorted by {}"), &[&names.join(", ")]));
    }

    fn show_selection(&self) {
        let positions = selected_positions(&self.selection);
        if positions.is_empty() {
            self.status_selection.set_text("");
            return;
        }
        let keys = self.model.keys();
        let length: Duration = positions
            .iter()
            .filter_map(|p| keys.get(*p as usize))
            .filter_map(|k| self.model.cached(*k))
            .filter_map(|r| r.duration)
            .sum();
        let count = fill(
            &ntr("{} selected", "{} selected", positions.len()),
            &[&format::count(positions.len())],
        );
        self.status_selection
            .set_text(&fill(&tr("{} · {}"), &[&count, &format::duration(length)]));
    }

    /// Shows the tracks of `scope`, as titled.
    pub(crate) fn set_scope(self: &Rc<Self>, scope: TrackScope, title: &str) {
        self.title.replace(title.to_owned());
        if self.scope.replace(scope) != scope || !self.loaded.get() {
            self.reload();
        }
    }

    /// Asks the backend for the keys again.
    pub(crate) fn reload(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let query = TrackQuery {
            scope: self.scope.get(),
            text: self.filter.text().trim().to_owned(),
            sort: self.sort.borrow().clone(),
        };
        let facets = self
            .shown_browser()
            .filter(|b| b.narrows())
            .map(|b| b.chosen());
        let filtered = !query.text.is_empty() || facets.is_some();
        let unfiltered = TrackQuery {
            text: String::new(),
            sort: Vec::new(),
            ..query.clone()
        };
        let need_total = filtered && self.total.get() == 0;
        let backend = self.backend.clone();
        let view = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |library| {
                    let keys = match &facets {
                        Some(chosen) => library.facet_track_keys(&query, chosen)?,
                        None => library.track_keys(&query)?,
                    };
                    let total = if need_total {
                        Some(library.track_keys(&unfiltered)?.len())
                    } else {
                        None
                    };
                    Ok((keys, total))
                })
                .await;
            let Some(view) = view.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            view.loaded.set(true);
            let (keys, total) = result.unwrap_or_default();
            if !filtered {
                view.total.set(keys.len());
            } else if let Some(total) = total {
                view.total.set(total);
            }
            let count = if filtered {
                fill(
                    &tr("{} of {}"),
                    &[&format::count(keys.len()), &format::count(view.total.get())],
                )
            } else {
                format::count(keys.len())
            };
            view.count.set_text(&count);
            if *view.model.keys() == keys[..] {
                // The same tracks (after an edit): the rows shown are fetched
                // again in place, so the table keeps its scroll position.
                view.refresh_rows();
            } else {
                view.model.set_keys(keys);
            }
            view.show_selection();
            if let Some(loaded) = view.on_loaded.borrow().as_ref() {
                loaded(view.model.n_items() as usize);
            }
        });
    }

    /// Shows only `columns`, in the table's order, each with its width and
    /// whether it takes spare room.
    pub(crate) fn show_columns(&self, columns: &[(TrackColumn, i32, bool)]) {
        for (column, c) in &self.columns {
            match columns.iter().find(|(shown, _, _)| shown == column) {
                Some((_, width, expand)) => {
                    c.set_visible(true);
                    c.set_fixed_width(*width);
                    c.set_expand(*expand);
                }
                None => c.set_visible(false),
            }
        }
    }

    /// Leaves the headers unmarked while the user has not sorted: the order is
    /// the query's.
    pub(crate) fn unmark_default_order(&self) {
        self.unmarked.set(true);
        self.show_sort();
    }

    /// Hides the status bar under the table.
    pub(crate) fn hide_status(&self) {
        if let Some(status) = self.status_selection.parent() {
            status.set_visible(false);
        }
    }

    /// Shows the tracks matching `text` (words or a query) at once; `title`
    /// names them in the queue.
    pub(crate) fn show_text(self: &Rc<Self>, text: &str, title: &str) {
        self.title.replace(title.to_owned());
        self.quiet.set(true);
        self.filter.set_text(text);
        self.quiet.set(false);
        if let Some(browser) = self.shown_browser() {
            browser.set_text(text);
        }
        self.reload();
    }

    /// Shows no tracks.
    pub(crate) fn clear(&self) {
        self.generation.set(self.generation.get() + 1);
        self.model.set_keys(Vec::new());
    }

    /// Calls `loaded` with the number of tracks after each load.
    pub(crate) fn connect_loaded(&self, loaded: impl Fn(usize) + 'static) {
        self.on_loaded.replace(Some(Box::new(loaded)));
    }

    /// Plays from the first selected row, or from the first row.
    pub(crate) fn play_selected(&self) {
        let first = selected_positions(&self.selection)
            .first()
            .copied()
            .unwrap_or(0);
        if (first as usize) < self.model.keys().len() {
            self.play_from(first as usize);
        }
    }

    /// Moves the selection by `by` rows and keeps it in view.
    pub(crate) fn move_selection(&self, by: i32) {
        let count = self.model.n_items();
        if count == 0 {
            return;
        }
        let current = selected_positions(&self.selection).first().copied();
        let next = match current {
            Some(p) => (i64::from(p) + i64::from(by)).clamp(0, i64::from(count) - 1) as u32,
            None => 0,
        };
        self.table
            .scroll_to(next, None, gtk::ListScrollFlags::SELECT, None);
    }

    /// Reloads after the library changed, dropping the cached rows.
    pub(crate) fn library_changed(self: &Rc<Self>) {
        self.model.forget_rows();
        self.total.set(0);
        if let Some(browser) = self.shown_browser() {
            browser.set_text(self.filter.text().trim());
        }
        self.reload();
    }

    fn play_from(&self, position: usize) {
        let keys = self.model.keys();
        let end = (position + PLAY_WINDOW).min(keys.len());
        let Some(tracks) = keys.get(position..end) else {
            return;
        };
        self.backend
            .send(crate::backend::PlayerRequest::PlayTracks {
                tracks: tracks.to_vec(),
                start: 0,
                title: self.title.borrow().clone(),
                shuffle: false,
            });
    }

    /// Sets the rating of `item`: shown at once, while the library stores it
    /// (and announces it to the other lists); put back if that fails.
    fn rate(&self, item: &TrackItem, stars: Option<u8>) {
        let key = item.key();
        let Some(before) = item.row().map(|r| r.rating) else {
            return;
        };
        self.ratings_changed(&[key], stars);
        let backend = self.backend.clone();
        let root = self.root.downgrade();
        glib::spawn_future_local(async move {
            if let Err(error) = backend.query(move |l| l.set_rating(key, stars)).await
                && let Some(window) = root
                    .upgrade()
                    .and_then(|r| r.root())
                    .and_downcast::<crate::window::MainWindow>()
            {
                window.ratings_changed(&[key], before);
                window.toast(&error.0, true);
            }
        });
    }

    /// Shows a changed rating in the rows of `keys`, in place.
    pub(crate) fn ratings_changed(&self, keys: &[TrackKey], stars: Option<u8>) {
        for key in keys {
            if let Some(row) = self.model.cached(*key) {
                let mut row = (*row).clone();
                row.rating = stars;
                self.model.update_row(row);
            }
        }
        // The items shown keep their own rows; the cells of a row share one.
        self.cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
            .for_each(|cell| {
                if let Some(item) = cell.item()
                    && keys.contains(&item.key())
                    && let Some(row) = item.row()
                    && row.rating != stars
                {
                    let mut row = (*row).clone();
                    row.rating = stars;
                    item.fill(Rc::new(row));
                }
            });
    }

    /// The rating stars of the row at `position`, if it is shown.
    fn stars_at(&self, position: u32) -> Option<RatingStars> {
        self.cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
            .filter(|c| c.position() == Some(position))
            .find_map(|c| c.child().and_downcast::<RatingStars>())
    }

    /// Rates the row at `position` with its stars, as a click would (for tests).
    pub(crate) fn demo_rate(&self, position: u32, stars: Option<u8>) -> bool {
        self.stars_at(position).map(|s| s.choose(stars)).is_some()
    }

    /// The rating the row at `position` shows, if it is shown (for tests).
    pub(crate) fn shown_rating(&self, position: u32) -> Option<Option<u8>> {
        self.stars_at(position).map(|s| s.rating())
    }

    /// Fetches the rows shown again, keeping them on screen until they come.
    fn refresh_rows(&self) {
        let mut seen = Vec::new();
        for cell in self
            .cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
        {
            if let Some(item) = cell.item()
                && !seen.contains(&item)
            {
                self.model.refetch(&item);
                seen.push(item);
            }
        }
    }

    /// Selects `count` rows from `start`.
    pub(crate) fn select_rows(&self, start: u32, count: u32) {
        self.selection.select_range(start, count, true);
    }

    /// Selects every row.
    pub(crate) fn select_all(&self) {
        self.selection.select_all();
    }

    /// The table, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.table.clone().upcast()
    }

    /// The filter field, for keyboard focus.
    pub(crate) fn filter_entry(&self) -> &gtk::Entry {
        &self.filter
    }

    /// The number of keys and the batches fetched (for tests).
    pub(crate) fn stats(&self) -> (u32, u64) {
        (self.model.n_items(), self.model.fetches())
    }

    /// The scrolled window around the table (for tests).
    pub(crate) fn vadjustment(&self) -> Option<gtk::Adjustment> {
        self.table.vadjustment()
    }

    /// The keys in their order (for tests).
    pub(crate) fn keys(&self) -> Rc<[TrackKey]> {
        self.model.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(column: TrackColumn, descending: bool) -> TrackSort {
        TrackSort { column, descending }
    }

    #[test]
    fn header_clicks_cycle_and_stack_keys() {
        let a = TrackColumn::Artist;
        let b = TrackColumn::Album;
        let keys = next_sort(&[], a, false);
        assert_eq!(keys, [key(a, false)]);
        let keys = next_sort(&keys, a, true);
        assert_eq!(keys, [key(a, true)]);
        let keys = next_sort(&keys, b, false);
        assert_eq!(keys, [key(b, false), key(a, true)]);
        // Descending, then off.
        let keys = next_sort(&keys, b, true);
        let keys = next_sort(&keys, b, false);
        assert_eq!(keys, [key(a, true)]);
        let keys = next_sort(&keys, a, false);
        assert!(keys.is_empty());
    }
}
