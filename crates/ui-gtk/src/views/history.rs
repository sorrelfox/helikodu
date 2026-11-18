// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Play history: every listen by day, newest first. The toolbar holds the
//! totals, the period (today, 7 days, 30 days, all), a filter and "Save as
//! playlist"; a day row gives that day's plays and listening time; a skipped
//! listen is dimmed and says where it was skipped. The status bar has the
//! totals of the period and the rule for counting a play.
//!
//! The columns share out the width by weight, in the header and the rows
//! alike, with a layout manager of their own.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::backend::{Backend, HistoryRow, HistoryTotals, PlayedFrom, PlayerRequest, TrackKey};
use crate::dialogs::common::{bind_segmented, segmented};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::playback::{Change, Playback};
use crate::views::actions::{self, Target};
use crate::views::table::{PlayingRows, TableCell, cell_at, selected_positions};
use crate::widgets::empty_state::empty_state;
use crate::widgets::{CoverArt, PlayingIndicator, label};

/// One column: a fixed width, or a share of what is left.
#[derive(Debug, Clone, Copy)]
enum Column {
    Fixed(i32),
    Weight(f32),
}

/// Time, Title, Artist, Album, Played from, Listened.
const COLUMNS: [Column; 6] = [
    Column::Fixed(44),
    Column::Weight(1.6),
    Column::Weight(1.1),
    Column::Weight(1.2),
    Column::Weight(0.9),
    Column::Fixed(110),
];

/// The space between two columns.
const GAP: i32 = 16;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct ColumnsLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for ColumnsLayout {
        const NAME: &'static str = "HkHistoryColumns";
        type Type = super::ColumnsLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for ColumnsLayout {}

    impl LayoutManagerImpl for ColumnsLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            _for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let children = children(widget);
            if orientation == gtk::Orientation::Horizontal {
                // The weighted columns ellipsize: they need no width of their own.
                let fixed: i32 = COLUMNS
                    .iter()
                    .map(|c| match c {
                        Column::Fixed(w) => *w,
                        Column::Weight(_) => 0,
                    })
                    .sum();
                let gaps = GAP * (children.len().max(1) as i32 - 1);
                return (fixed + gaps, fixed + gaps, -1, -1);
            }
            let (mut min, mut nat) = (0, 0);
            for child in &children {
                let (m, n, _, _) = child.measure(orientation, -1);
                min = min.max(m);
                nat = nat.max(n);
            }
            (min, nat, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let children = children(widget);
            let gaps = GAP * (children.len().max(1) as i32 - 1);
            let (fixed, weights) = COLUMNS.iter().fold((0, 0.0), |(f, w), c| match c {
                Column::Fixed(px) => (f + px, w),
                Column::Weight(share) => (f, w + share),
            });
            let free = (width - fixed - gaps).max(0) as f32;
            let rtl = widget.direction() == gtk::TextDirection::Rtl;
            let mut x = 0;
            for (child, column) in children.iter().zip(COLUMNS.iter()) {
                let w = match column {
                    Column::Fixed(px) => *px,
                    Column::Weight(share) => (free * share / weights).floor() as i32,
                };
                let left = if rtl { width - x - w } else { x };
                child.size_allocate(&gtk::Allocation::new(left, 0, w, height), -1);
                x += w + GAP;
            }
        }
    }
}

glib::wrapper! {
    /// Lays out the columns of the history by [`COLUMNS`].
    pub(crate) struct ColumnsLayout(ObjectSubclass<imp::ColumnsLayout>)
        @extends gtk::LayoutManager;
}

fn children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.first_child();
    while let Some(w) = child {
        child = w.next_sibling();
        if w.should_layout() {
            out.push(w);
        }
    }
    out
}

/// A row of the columns.
fn columns_box() -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.set_layout_manager(Some(glib::Object::new::<ColumnsLayout>()));
    line
}

/// The periods of the segmented control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Period {
    Today,
    Week,
    Month,
    All,
}

impl Period {
    const ALL: [Self; 4] = [Self::Today, Self::Week, Self::Month, Self::All];

    fn label(self) -> String {
        match self {
            Self::Today => tr("Today"),
            Self::Week => tr("7 days"),
            Self::Month => tr("30 days"),
            Self::All => tr("All"),
        }
    }

    /// The listening time of the period, for the status bar.
    fn time_phrase(self, time: Duration) -> String {
        let pattern = match self {
            Self::Today => tr("{} today"),
            Self::Week => tr("{} in the last 7 days"),
            Self::Month => tr("{} in the last 30 days"),
            Self::All => tr("{} in all"),
        };
        fill(&pattern, &[&format::minutes(time)])
    }

    /// The start of the period: midnight of its first day.
    pub(crate) fn since(self) -> Option<SystemTime> {
        let days = match self {
            Self::Today => 0,
            Self::Week => 6,
            Self::Month => 29,
            Self::All => return None,
        };
        let now = glib::DateTime::now_local().ok()?;
        let midnight =
            glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 0, 0, 0.0)
                .ok()?
                .add_days(-days)
                .ok()?;
        let seconds = u64::try_from(midnight.to_unix()).ok()?;
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
    }
}

/// A line of the list: a day, or a listen.
#[derive(Debug, Clone)]
enum Line {
    Day { title: String, summary: String },
    Listen(Rc<HistoryRow>),
}

fn local(time: SystemTime) -> Option<glib::DateTime> {
    let seconds = time.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    glib::DateTime::from_unix_local(i64::try_from(seconds).ok()?).ok()
}

/// The day of `time` as a number that grows by one a day.
fn day_number(date: &glib::DateTime) -> i32 {
    date.year() * 400 + date.day_of_year()
}

/// "Today", "Yesterday", or the date.
fn day_title(date: &glib::DateTime, today: &glib::DateTime) -> String {
    match day_number(today) - day_number(date) {
        0 => tr("Today"),
        1 => tr("Yesterday"),
        _ => {
            // Translators: a day in the play history, as strftime: "Saturday 15 November".
            let pattern = if date.year() == today.year() {
                tr("%A %e %B")
            } else {
                tr("%A %e %B %Y")
            };
            date.format(&pattern)
                .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                .unwrap_or_default()
        }
    }
}

/// "9 plays · 41 min" for the listens of a day or a period.
fn plays_and_time(rows: &[&Rc<HistoryRow>]) -> (usize, Duration, usize) {
    let plays = rows.iter().filter(|r| r.counted).count();
    let time = rows.iter().map(|r| r.listened).sum();
    (plays, time, rows.len() - plays)
}

fn plays_text(plays: usize) -> String {
    fill(&ntr("{} play", "{} plays", plays), &[&format::count(plays)])
}

/// Where a listen came from, as the table shows it.
fn played_from(from: &PlayedFrom) -> String {
    match from {
        PlayedFrom::Unknown => String::new(),
        PlayedFrom::Album => tr("Album"),
        PlayedFrom::Artist => tr("Artist"),
        PlayedFrom::Genre => tr("Genre"),
        PlayedFrom::Playlist(name) if name.is_empty() => tr("Playlist"),
        PlayedFrom::Playlist(name) => name.clone(),
        PlayedFrom::Search(text) => fill(&tr("Search: “{}”"), &[text]),
        PlayedFrom::Folder(name) => fill(&tr("Folder: {}"), &[name]),
        PlayedFrom::Queue => tr("Queue"),
        PlayedFrom::Files => tr("Files"),
    }
}

/// The lines for `rows` (newest first) that match `filter`, by day.
fn lines(rows: &[Rc<HistoryRow>], filter: &str) -> Vec<Line> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    let shown: Vec<&Rc<HistoryRow>> = rows
        .iter()
        .filter(|row| {
            words.is_empty() || {
                let from = played_from(&row.from);
                let fields = [
                    row.track.title.as_str(),
                    row.track.artist.as_str(),
                    row.track.album_title.as_str(),
                    from.as_str(),
                ];
                words
                    .iter()
                    .all(|w| fields.iter().any(|f| f.to_lowercase().contains(w.as_str())))
            }
        })
        .collect();
    let Ok(today) = glib::DateTime::now_local() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut start = 0;
    while start < shown.len() {
        let day = local(shown[start].started_at);
        let number = day.as_ref().map(day_number);
        let end = shown[start..]
            .iter()
            .position(|r| local(r.started_at).as_ref().map(day_number) != number)
            .map_or(shown.len(), |n| start + n);
        let (plays, time, _) = plays_and_time(&shown[start..end]);
        out.push(Line::Day {
            title: day
                .as_ref()
                .map(|d| day_title(d, &today))
                .unwrap_or_default(),
            summary: format!("{} · {}", plays_text(plays), format::minutes(time)),
        });
        out.extend(shown[start..end].iter().map(|r| Line::Listen(Rc::clone(r))));
        start = end;
    }
    out
}

/// The Play history page.
pub(crate) struct HistoryView {
    pub(crate) root: gtk::Box,
    subtitle: gtk::Label,
    filter: gtk::Entry,
    save: gtk::Button,
    content: gtk::Stack,
    list: gtk::ListView,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    status_left: gtk::Label,
    status_right: gtk::Label,
    backend: Backend,
    covers: Rc<CoverCache>,
    period: Cell<Period>,
    rows: RefCell<Vec<Rc<HistoryRow>>>,
    totals: Cell<Option<HistoryTotals>>,
    generation: Cell<u64>,
    /// The cells, marked where the listen is of the playing track.
    playing: PlayingRows,
}

impl std::fmt::Debug for HistoryView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryView")
            .field("period", &self.period)
            .finish_non_exhaustive()
    }
}

impl HistoryView {
    /// The page over `backend`.
    pub(crate) fn new(
        backend: &Backend,
        playback: &Rc<Playback>,
        covers: &Rc<CoverCache>,
    ) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-history-view");
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        toolbar.add_css_class("hk-toolbar");
        let title = label(&tr("History"), &["hk-view-title"]);
        title.set_hexpand(false);
        title.set_accessible_role(gtk::AccessibleRole::Heading);
        toolbar.append(&title);
        let subtitle = label("", &["hk-t3", "hk-numeric"]);
        subtitle.set_valign(gtk::Align::Baseline);
        subtitle.set_hexpand(true);
        toolbar.append(&subtitle);
        let names: Vec<String> = Period::ALL.iter().map(|p| p.label()).collect();
        let (periods, period_buttons) = segmented(&names);
        periods.update_property(&[gtk::accessible::Property::Label(&tr("Period"))]);
        toolbar.append(&periods);
        let filter = gtk::Entry::new();
        filter.add_css_class("hk-input");
        filter.add_css_class("hk-history-filter");
        filter.set_placeholder_text(Some(&tr("Filter")));
        filter.set_primary_icon_name(Some("system-search-symbolic"));
        filter.set_size_request(200, -1);
        filter.set_valign(gtk::Align::Center);
        filter.update_property(&[gtk::accessible::Property::Label(&tr("Filter the history"))]);
        toolbar.append(&filter);
        let save = gtk::Button::with_label(&tr("Save as playlist"));
        save.add_css_class("hk-btn");
        save.add_css_class("small");
        save.set_valign(gtk::Align::Center);
        toolbar.append(&save);
        root.append(&toolbar);

        let header = columns_box();
        header.add_css_class("hk-table-header");
        header.add_css_class("hk-history-row");
        for (i, text) in [
            tr("Time"),
            tr("Title"),
            tr("Artist"),
            tr("Album"),
            tr("Played from"),
            tr("Listened"),
        ]
        .iter()
        .enumerate()
        {
            let l = label(text, &["hk-section-label"]);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            if i == 5 {
                l.set_xalign(1.0);
            }
            header.append(&l);
        }
        root.append(&header);

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let list = gtk::ListView::new(Some(selection.clone()), None::<gtk::ListItemFactory>);
        list.add_css_class("hk-history");
        list.set_vexpand(true);
        list.update_property(&[gtk::accessible::Property::Label(&tr("Play history"))]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        let content = gtk::Stack::new();
        content.set_vexpand(true);
        content.add_named(&scroller, Some("list"));
        content.add_named(
            &empty_state(
                "document-open-recent-symbolic",
                &tr("Nothing played"),
                &tr("Tracks you listen to are listed here by day, with where they came from."),
                &[],
            ),
            Some("empty"),
        );
        root.append(&content);

        let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        status.add_css_class("hk-statusbar");
        let status_left = label("", &["hk-numeric"]);
        status_left.set_hexpand(true);
        status.append(&status_left);
        let status_right = label("", &[]);
        status_right.set_xalign(1.0);
        status.append(&status_right);
        root.append(&status);

        let view = Rc::new(Self {
            root,
            subtitle,
            filter,
            save,
            content,
            list,
            store,
            selection,
            status_left,
            status_right,
            backend: backend.clone(),
            covers: covers.clone(),
            period: Cell::new(Period::Week),
            rows: RefCell::new(Vec::new()),
            totals: Cell::new(None),
            generation: Cell::new(0),
            playing: PlayingRows::default(),
        });
        view.list.set_factory(Some(&view.factory()));
        let weak = Rc::downgrade(&view);
        bind_segmented(&period_buttons, 1, move |i| {
            if let Some(view) = weak.upgrade() {
                view.period.set(Period::ALL[i]);
                view.reload();
            }
        });
        view.connect(playback);
        view
    }

    fn factory(self: &Rc<Self>) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let both = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let day = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            day.add_css_class("hk-history-day");
            let day_title = label("", &["hk-history-day-title"]);
            day_title.set_accessible_role(gtk::AccessibleRole::Heading);
            day.append(&day_title);
            day.append(&label(
                "",
                &["hk-t3", "hk-history-day-summary", "hk-numeric"],
            ));
            both.append(&day);
            let line = columns_box();
            line.add_css_class("hk-history-row");
            let time = label("", &["hk-mono", "hk-t3", "hk-history-time"]);
            line.append(&time);
            let title_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let cover = CoverArt::new(24, 4.0);
            cover.add_css_class("small");
            title_box.append(&cover);
            // The table has no number column: the playing row's indicator
            // leads the title.
            let indicator = PlayingIndicator::default();
            indicator.set_visible(false);
            title_box.append(&indicator);
            let title = label("", &["hk-medium"]);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.set_hexpand(true);
            title_box.append(&title);
            line.append(&title_box);
            for classes in [&["hk-t2"][..], &["hk-t2"], &["hk-t3"]] {
                let l = label("", classes);
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                line.append(&l);
            }
            let listened = label("", &["hk-numeric", "hk-t2"]);
            listened.set_xalign(1.0);
            line.append(&listened);
            both.append(&line);
            let cell = TableCell::new(&both);
            if let Some(view) = weak.upgrade() {
                view.playing.add(&cell);
            }
            item.set_child(Some(&cell));
        });
        let weak = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(cell), Some(object), Some(view)) = (
                item.child().and_downcast::<TableCell>(),
                item.item().and_downcast::<glib::BoxedAnyObject>(),
                weak.upgrade(),
            ) else {
                return;
            };
            cell.set_position(Some(item.position()));
            let line = object.borrow::<Line>();
            let is_listen = matches!(&*line, Line::Listen(_));
            item.set_selectable(is_listen);
            item.set_activatable(is_listen);
            view.show_line(&cell, &line);
        });
        factory.connect_unbind(|_, item| {
            if let Some(cell) = item
                .downcast_ref::<gtk::ListItem>()
                .and_then(|i| i.child().and_downcast::<TableCell>())
            {
                cell.set_position(None);
                cell.set_track(None);
            }
        });
        factory
    }

    /// Marks `cell` as a listen of the playing track, or not.
    fn mark(cell: &TableCell, playing: bool) {
        cell.set_playing(playing);
        let indicator = cell
            .child()
            .and_then(|both| both.last_child())
            .and_then(|line| line.first_child()?.next_sibling())
            .and_then(|title_box| title_box.first_child()?.next_sibling())
            .and_downcast::<PlayingIndicator>();
        if let Some(indicator) = indicator {
            indicator.set_visible(playing);
        }
    }

    fn show_line(&self, cell: &TableCell, line: &Line) {
        let Some(both) = cell.child() else { return };
        let (Some(day), Some(row_box)) = (both.first_child(), both.last_child()) else {
            return;
        };
        day.set_visible(matches!(line, Line::Day { .. }));
        row_box.set_visible(matches!(line, Line::Listen(_)));
        cell.remove_css_class("hk-skip");
        cell.remove_css_class("hk-missing");
        match line {
            Line::Day { title, summary } => {
                cell.set_track(None);
                Self::mark(cell, false);
                cell.add_css_class("hk-day-cell");
                if let (Some(t), Some(s)) = (
                    day.first_child().and_downcast::<gtk::Label>(),
                    day.last_child().and_downcast::<gtk::Label>(),
                ) {
                    t.set_text(title);
                    s.set_text(summary);
                }
            }
            Line::Listen(row) => {
                cell.remove_css_class("hk-day-cell");
                let key = (!row.track.missing).then_some(row.track.key);
                cell.set_track(key);
                Self::mark(cell, self.playing.contains(key));
                let parts = children(&row_box);
                let [time, title_box, artist, album, from, listened] = parts.as_slice() else {
                    return;
                };
                let set = |w: &gtk::Widget, text: &str| {
                    if let Some(l) = w.downcast_ref::<gtk::Label>() {
                        l.set_text(text);
                    }
                };
                set(
                    time,
                    &local(row.started_at)
                        .and_then(|d| d.format("%H:%M").ok())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                );
                if let Some(cover) = title_box.first_child().and_downcast::<CoverArt>() {
                    self.covers
                        .show(&cover, row.track.cover, row.track.cover_seed, 24);
                    cover.set_opacity(if row.counted { 1.0 } else { 0.5 });
                }
                if let Some(title) = title_box.last_child() {
                    if row.track.missing {
                        set(&title, &tr("A track no longer in the library"));
                    } else {
                        set(&title, &row.track.title);
                    }
                }
                set(artist, &row.track.artist);
                set(album, &row.track.album_title);
                set(from, &played_from(&row.from));
                if row.counted {
                    set(listened, &format::duration(row.listened));
                } else {
                    set(
                        listened,
                        &fill(&tr("Skipped at {}"), &[&format::duration(row.listened)]),
                    );
                    cell.add_css_class("hk-skip");
                }
                if row.track.missing {
                    cell.add_css_class("hk-missing");
                }
            }
        }
    }

    fn connect(self: &Rc<Self>, playback: &Rc<Playback>) {
        let weak = Rc::downgrade(self);
        self.filter.connect_changed(move |_| {
            if let Some(view) = weak.upgrade() {
                view.show_rows();
            }
        });
        let weak = Rc::downgrade(self);
        self.save.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.save_as_playlist();
            }
        });
        let weak = Rc::downgrade(self);
        self.list.connect_activate(move |_, position| {
            if let Some(view) = weak.upgrade() {
                view.play(position);
            }
        });
        // A listen ends when another track starts: read the history again.
        let weak = Rc::downgrade(self);
        playback.connect(move |player, change| {
            let Some(view) = weak.upgrade() else { return };
            match change {
                Change::Track if view.root.is_mapped() => view.reload(),
                Change::Playing => {
                    let playing = player.playing;
                    view.playing.update(playing, |cell| {
                        Self::mark(cell, playing.contains_opt(cell.track()))
                    });
                }
                _ => {}
            }
        });
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            let Some(position) = cell_at(&view.list, x, y).and_then(|c| c.position()) else {
                return;
            };
            if view.listen_at(position).is_none() {
                return;
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if !view.selection.is_selected(position) {
                view.selection.select_item(position, true);
            }
            if let Some(target) = view.target() {
                actions::popup(&view.list, target, Some((x, y)));
            }
        });
        self.list.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if actions::is_menu_key(key, state)
                && let Some(target) = view.target()
            {
                actions::popup(&view.list, target, None);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.list.add_controller(keys);
    }

    fn window(&self) -> Option<crate::window::MainWindow> {
        self.root.root().and_downcast::<crate::window::MainWindow>()
    }

    fn listen_at(&self, position: u32) -> Option<Rc<HistoryRow>> {
        let object = self
            .store
            .item(position)
            .and_downcast::<glib::BoxedAnyObject>()?;
        let line = object.borrow::<Line>();
        match &*line {
            Line::Listen(row) => Some(Rc::clone(row)),
            Line::Day { .. } => None,
        }
    }

    /// The selected listens of tracks still in the library, for the menu.
    pub(crate) fn target(&self) -> Option<Target> {
        let rows: Vec<Rc<crate::backend::TrackRow>> = selected_positions(&self.selection)
            .into_iter()
            .filter_map(|p| self.listen_at(p))
            .filter(|r| !r.track.missing)
            .map(|r| Rc::new(r.track.clone()))
            .collect();
        (!rows.is_empty()).then(|| Target::tracks(rows))
    }

    fn play(&self, position: u32) {
        let Some(row) = self.listen_at(position).filter(|r| !r.track.missing) else {
            return;
        };
        self.backend.send(PlayerRequest::PlayTracks {
            tracks: vec![row.track.key],
            start: 0,
            title: tr("History"),
            shuffle: false,
        });
    }

    /// Reads the history again when the page is shown.
    pub(crate) fn show(self: &Rc<Self>) {
        self.reload();
    }

    /// Reads the period's listens and the totals.
    pub(crate) fn reload(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let since = self.period.get().since();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let rows = backend.query(move |l| l.history(since)).await;
            let totals = backend.query(|l| l.history_totals()).await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            view.rows
                .replace(rows.unwrap_or_default().into_iter().map(Rc::new).collect());
            view.totals.set(totals.ok());
            view.show_rows();
        });
    }

    /// Fills the list from the rows read and the filter.
    fn show_rows(&self) {
        let rows = self.rows.borrow();
        let lines = lines(&rows, &self.filter.text());
        let objects: Vec<glib::BoxedAnyObject> =
            lines.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &objects);
        self.content
            .set_visible_child_name(if objects.is_empty() { "empty" } else { "list" });
        self.save.set_sensitive(rows.iter().any(|r| r.counted));
        let totals = self.totals.get().unwrap_or_default();
        self.subtitle.set_text(&match (totals.plays, totals.first) {
            (0, _) | (_, None) => tr("Nothing played yet"),
            (plays, Some(first)) => fill(
                &ntr("{} play since {}", "{} plays since {}", plays as usize),
                &[
                    &format::count(plays),
                    &local(first)
                        .and_then(|d| d.format("%e %b %Y").ok())
                        .map(|s| s.trim().to_owned())
                        .unwrap_or_default(),
                ],
            ),
        });
        let all: Vec<&Rc<HistoryRow>> = rows.iter().collect();
        let (plays, time, skips) = plays_and_time(&all);
        self.status_left.set_text(&format!(
            "{} · {} · {}",
            plays_text(plays),
            self.period.get().time_phrase(time),
            fill(
                &ntr("{} skipped", "{} skipped", skips),
                &[&format::count(skips)]
            ),
        ));
        self.status_right.set_text(&rule(&totals));
    }

    /// Makes a playlist of what is shown, without skips and repeats.
    fn save_as_playlist(self: &Rc<Self>) {
        let mut keys: Vec<TrackKey> = Vec::new();
        for position in 0..self.store.n_items() {
            if let Some(row) = self.listen_at(position)
                && row.counted
                && !row.track.missing
                && !keys.contains(&row.track.key)
            {
                keys.push(row.track.key);
            }
        }
        if keys.is_empty() {
            return;
        }
        let name = fill(&tr("History, {}"), &[&self.period.get().label()]);
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let count = keys.len();
            let made = backend
                .query(move |l| {
                    let key = l.create_playlist(&name)?;
                    l.add_to_playlist(key, &keys, None)?;
                    Ok((key, name))
                })
                .await;
            let Some(window) = weak.upgrade().and_then(|v| v.window()) else {
                return;
            };
            match made {
                Ok((key, name)) => {
                    window.playlists_changed(Some(key));
                    window.toast(
                        &fill(
                            &ntr("Saved {} track as “{}”", "Saved {} tracks as “{}”", count),
                            &[&format::count(count), &name],
                        ),
                        false,
                    );
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// The widget F6 focuses.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.list.clone().upcast()
    }

    /// The number of day rows and listens shown (for tests).
    pub(crate) fn counts(&self) -> (usize, usize) {
        let mut days = 0;
        let mut listens = 0;
        for position in 0..self.store.n_items() {
            if self.listen_at(position).is_some() {
                listens += 1;
            } else {
                days += 1;
            }
        }
        (days, listens)
    }
}

/// The rule for counting a play, for the status bar.
fn rule(totals: &HistoryTotals) -> String {
    let minutes = totals.play_minutes as usize;
    if totals.play_percent == 50 {
        fill(
            &ntr(
                "A play counts after half the track or {} minute",
                "A play counts after half the track or {} minutes",
                minutes,
            ),
            &[&minutes.to_string()],
        )
    } else {
        fill(
            &ntr(
                "A play counts after {} % of the track or {} minute",
                "A play counts after {} % of the track or {} minutes",
                minutes,
            ),
            &[&totals.play_percent.to_string(), &minutes.to_string()],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_name_where_a_listen_came_from() {
        let name = |id: u64| (id == 7).then(|| "Late evening".to_owned());
        assert_eq!(
            PlayedFrom::parse(Some("playlist:7"), name),
            PlayedFrom::Playlist("Late evening".into())
        );
        assert_eq!(
            PlayedFrom::parse(Some("search:slow"), name),
            PlayedFrom::Search("slow".into())
        );
        assert_eq!(
            PlayedFrom::parse(Some("folder:/home/me/Incoming"), name),
            PlayedFrom::Folder("Incoming".into())
        );
        assert_eq!(PlayedFrom::parse(Some("album:12"), name), PlayedFrom::Album);
        assert_eq!(PlayedFrom::parse(Some("queue"), name), PlayedFrom::Queue);
        assert_eq!(PlayedFrom::parse(None, name), PlayedFrom::Unknown);
        assert_eq!(
            PlayedFrom::parse(Some("playlist:3"), name),
            PlayedFrom::Playlist(String::new())
        );
    }
}
