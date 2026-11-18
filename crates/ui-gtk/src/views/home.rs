// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Home: the albums added and played lately, each in one row of tiles (as
//! many as fit, never a second row), and two lists side by side: the most
//! played tracks and albums to rediscover.
//!
//! Everything comes from the library's existing queries on the worker
//! thread: the album list by date added, the play history (the albums played
//! lately, and when each album was played last), and the tracks by play
//! count. "Rediscover" lists albums not played for [`REDISCOVER_AFTER`],
//! the longest unplayed first, then albums never played, the earliest added
//! first.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib};

use crate::backend::{
    AlbumFilter, AlbumKey, AlbumQuery, AlbumRow, AlbumScope, AlbumSort, Backend, BackendError,
    LibrarySource, PlayerRequest, TrackColumn, TrackKey, TrackQuery, TrackRow, TrackSort,
};
use crate::format;
use crate::i18n::{fill, tr};
use crate::models::{AlbumItem, CoverCache};
use crate::playback::{Change, Playback, PlayingKeys};
use crate::views::actions::{self, Target};
use crate::views::album_tile::AlbumTile;
use crate::widgets::{CoverArt, PlayingIndicator, label};

/// How many tiles a row holds at most.
const TILES: usize = 12;
/// How many rows each list has.
const LIST_ROWS: usize = 5;
/// How long an album must have gone unplayed to be rediscovered.
pub(crate) const REDISCOVER_AFTER: Duration = Duration::from_secs(90 * 24 * 60 * 60);
/// The narrowest a tile gets, and the gap between tiles.
const TILE_MIN: i32 = 148;
const TILE_GAP: i32 = 20;

/// What Home shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct HomeData {
    /// The albums added last, newest first.
    pub(crate) added: Vec<AlbumRow>,
    /// The albums played last, the latest first.
    pub(crate) played: Vec<AlbumRow>,
    /// The most played tracks.
    pub(crate) most_played: Vec<TrackRow>,
    /// Albums to play again, with when they were played last.
    pub(crate) rediscover: Vec<(AlbumRow, Option<SystemTime>)>,
}

/// Reads what Home shows from `library`, `now` being the time of the reading.
pub(crate) fn load(library: &dyn LibrarySource, now: SystemTime) -> Result<HomeData, BackendError> {
    let by_added = AlbumQuery {
        sort: AlbumSort::DateAdded,
        filter: AlbumFilter::All,
        scope: AlbumScope::All,
    };
    let added = library.album_rows(&by_added, 0, TILES)?;
    // The history, newest first: the albums in the order they were last played.
    let history = library.history(None)?;
    let mut last_played: HashMap<AlbumKey, SystemTime> = HashMap::new();
    let mut order = Vec::new();
    for listen in &history {
        let album = listen.track.album;
        if album.0 == 0 || last_played.contains_key(&album) {
            continue;
        }
        last_played.insert(album, listen.started_at);
        order.push(album);
    }
    let album_row = |key: AlbumKey| -> Result<Option<AlbumRow>, BackendError> {
        Ok(library.album(key)?.map(|detail| detail.row))
    };
    let mut played = Vec::new();
    for key in order.iter().copied() {
        if played.len() == TILES {
            break;
        }
        if let Some(row) = album_row(key)? {
            played.push(row);
        }
    }
    let by_plays = TrackQuery {
        sort: vec![TrackSort {
            column: TrackColumn::Plays,
            descending: true,
        }],
        ..TrackQuery::default()
    };
    let top: Vec<_> = library
        .track_keys(&by_plays)?
        .into_iter()
        .take(LIST_ROWS)
        .collect();
    let most_played: Vec<TrackRow> = library
        .track_rows(&top)?
        .into_iter()
        .filter(|t| t.plays > 0)
        .collect();
    let mut rediscover = Vec::new();
    for key in order.iter().rev().copied() {
        if rediscover.len() == LIST_ROWS {
            break;
        }
        let Some(when) = last_played.get(&key).copied() else {
            continue;
        };
        if now.duration_since(when).unwrap_or_default() < REDISCOVER_AFTER {
            continue;
        }
        if let Some(row) = album_row(key)? {
            rediscover.push((row, Some(when)));
        }
    }
    if rediscover.len() < LIST_ROWS {
        let keys = library.album_keys(&by_added)?;
        let mut offset = keys.len();
        'fill: while offset > 0 && rediscover.len() < LIST_ROWS {
            let start = offset.saturating_sub(TILES);
            let rows = library.album_rows(&by_added, start, offset - start)?;
            for row in rows.into_iter().rev() {
                if rediscover.len() == LIST_ROWS {
                    break 'fill;
                }
                if !last_played.contains_key(&row.key) {
                    rediscover.push((row, None));
                }
            }
            offset = start;
        }
    }
    Ok(HomeData {
        added,
        played,
        most_played,
        rediscover,
    })
}

/// The month an album was played last, or "Never".
pub(crate) fn last_played_text(when: Option<SystemTime>) -> String {
    let Some(when) = when else {
        return tr("Never");
    };
    let seconds = when
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    glib::DateTime::from_unix_local(i64::try_from(seconds).unwrap_or(0))
        .ok()
        // Translators: the month an album was played last, as strftime:
        // "Nov 2026"; keep the % codes, change their order as needed.
        .and_then(|date| date.format(&tr("%b %Y")).ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

mod row_imp {
    use super::*;

    /// One row of tiles: as many as fit at [`TILE_MIN`] or wider, sharing the
    /// width; the others are not shown.
    #[derive(Debug, Default)]
    pub(crate) struct TileRow {}

    #[glib::object_subclass]
    impl ObjectSubclass for TileRow {
        const NAME: &'static str = "HkTileRow";
        type Type = super::TileRow;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for TileRow {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for TileRow {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let obj = self.obj();
            match orientation {
                gtk::Orientation::Horizontal => (TILE_MIN, TILE_MIN, -1, -1),
                _ => {
                    let width = if for_size < 0 { TILE_MIN } else { for_size };
                    let (_, tile) = layout(width, children(&obj).len());
                    let mut height = 0;
                    for child in children(&obj) {
                        let (min, _, _, _) = child.measure(gtk::Orientation::Vertical, tile);
                        height = height.max(min);
                    }
                    (height, height, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let obj = self.obj();
            let all = children(&obj);
            let (count, tile) = layout(width, all.len());
            for (i, child) in all.iter().enumerate() {
                let shown = i < count;
                child.set_child_visible(shown);
                if shown {
                    let x = i32::try_from(i).unwrap_or(0) * (tile + TILE_GAP);
                    child.size_allocate(&gtk::Allocation::new(x, 0, tile, height), -1);
                }
            }
        }
    }
}

glib::wrapper! {
    /// A row of tiles that never wraps.
    pub(crate) struct TileRow(ObjectSubclass<row_imp::TileRow>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for TileRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl TileRow {
    fn clear(&self) {
        while let Some(child) = self.first_child() {
            child.unparent();
        }
    }

    fn append(&self, child: &impl IsA<gtk::Widget>) {
        child.set_parent(self);
    }

    /// How many tiles are shown.
    pub(crate) fn shown(&self) -> usize {
        children(self)
            .iter()
            .filter(|c| c.is_child_visible())
            .count()
    }
}

fn children(row: &TileRow) -> Vec<gtk::Widget> {
    let mut all = Vec::new();
    let mut child = row.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        all.push(c);
    }
    all
}

/// How many tiles fit in `width`, at most `available`, and how wide each is.
fn layout(width: i32, available: usize) -> (usize, i32) {
    let fit = usize::try_from((width + TILE_GAP) / (TILE_MIN + TILE_GAP))
        .unwrap_or(1)
        .max(1);
    let count = fit.min(available.max(1));
    // The tiles keep the width of a full row, so a short row does not stretch.
    let n = i32::try_from(fit).unwrap_or(1);
    let tile = ((width - TILE_GAP * (n - 1)) / n).max(1);
    (count, tile)
}

/// Where a click on an album goes.
type OpenAlbum = Rc<dyn Fn(AlbumKey)>;

/// The Home page.
pub(crate) struct HomeView {
    /// The page.
    pub(crate) root: gtk::ScrolledWindow,
    backend: Backend,
    covers: Rc<CoverCache>,
    added: TileRow,
    played: TileRow,
    most: gtk::ListBox,
    rediscover: gtk::ListBox,
    data: RefCell<HomeData>,
    open_album: RefCell<Option<OpenAlbum>>,
    show_all: RefCell<Option<Rc<dyn Fn()>>>,
    loading: Cell<bool>,
    stale: Cell<bool>,
    playing: Cell<Option<AlbumKey>>,
    /// The tracks whose Most played rows show as playing.
    playing_tracks: Cell<PlayingKeys>,
}

impl std::fmt::Debug for HomeView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HomeView").finish_non_exhaustive()
    }
}

/// A 30 px header row: the title and, at the right, `t3` text or a link.
fn header(title: &str, right: &gtk::Widget, list: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("hk-home-head");
    if list {
        row.add_css_class("list");
    }
    let heading = label(title, &["hk-strong"]);
    heading.set_hexpand(true);
    row.append(&heading);
    right.set_valign(gtk::Align::Center);
    row.append(right);
    row
}

fn link(text: &str) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("hk-home-link");
    button
}

impl HomeView {
    /// The page over `backend`.
    pub(crate) fn new(
        backend: &Backend,
        playback: &Rc<Playback>,
        covers: &Rc<CoverCache>,
    ) -> Rc<Self> {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("hk-home");
        let title = label(&tr("Home"), &["hk-view-title"]);
        title.add_css_class("hk-home-title");
        body.append(&title);

        let show_all = link(&tr("Show all"));
        body.append(&header(&tr("Recently added"), show_all.upcast_ref(), false));
        let added = TileRow::default();
        added.add_css_class("hk-home-tiles");
        body.append(&added);
        let history = link(&tr("History"));
        history.set_action_name(Some("win.show-history"));
        body.append(&header(&tr("Recently played"), history.upcast_ref(), false));
        let played = TileRow::default();
        played.add_css_class("hk-home-tiles");
        body.append(&played);

        let lists = gtk::Box::new(gtk::Orientation::Horizontal, 40);
        lists.set_homogeneous(true);
        let most = gtk::ListBox::new();
        most.add_css_class("hk-home-list");
        most.set_activate_on_single_click(false);
        most.update_property(&[gtk::accessible::Property::Label(&tr("Most played"))]);
        let rediscover = gtk::ListBox::new();
        rediscover.add_css_class("hk-home-list");
        rediscover.set_activate_on_single_click(true);
        rediscover.update_property(&[gtk::accessible::Property::Label(&tr("Rediscover"))]);
        for (title, caption, list) in [
            (tr("Most played"), tr("Plays"), &most),
            (tr("Rediscover"), tr("Last played"), &rediscover),
        ] {
            let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let caption = label(&caption, &["hk-t3", "hk-secondary"]);
            column.append(&header(&title, caption.upcast_ref(), true));
            column.append(list);
            lists.append(&column);
        }
        body.append(&lists);

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_vexpand(true);
        root.add_css_class("hk-content");
        root.set_child(Some(&body));

        let view = Rc::new(Self {
            root,
            backend: backend.clone(),
            covers: covers.clone(),
            added,
            played,
            most,
            rediscover,
            data: RefCell::new(HomeData::default()),
            open_album: RefCell::new(None),
            show_all: RefCell::new(None),
            loading: Cell::new(false),
            stale: Cell::new(true),
            playing: Cell::new(None),
            playing_tracks: Cell::new(PlayingKeys::default()),
        });
        let weak = Rc::downgrade(&view);
        show_all.connect_clicked(move |_| {
            let show = weak.upgrade().and_then(|v| v.show_all.borrow().clone());
            if let Some(show) = show {
                show();
            }
        });
        let weak = Rc::downgrade(&view);
        view.most.connect_row_activated(move |_, row| {
            if let Some(view) = weak.upgrade() {
                view.play_most_played(row.index());
            }
        });
        let weak = Rc::downgrade(&view);
        view.rediscover.connect_row_activated(move |_, row| {
            let Some(view) = weak.upgrade() else { return };
            let key = usize::try_from(row.index())
                .ok()
                .and_then(|i| view.data.borrow().rediscover.get(i).map(|(a, _)| a.key));
            if let Some(key) = key {
                view.open(key);
            }
        });
        let weak = Rc::downgrade(&view);
        playback.connect(move |player, change| {
            let Some(view) = weak.upgrade() else { return };
            match change {
                Change::Track => {
                    view.playing.set(player.album());
                    view.mark_tiles();
                }
                Change::Playing
                    if view.playing_tracks.replace(player.playing) != player.playing =>
                {
                    view.mark_most_played();
                }
                _ => {}
            }
        });
        view
    }

    /// Where a click on a tile or a rediscovered album goes.
    pub(crate) fn connect_open(&self, open: impl Fn(AlbumKey) + 'static) {
        self.open_album.replace(Some(Rc::new(open)));
    }

    /// Where "Show all" goes.
    pub(crate) fn connect_show_all(&self, show: impl Fn() + 'static) {
        self.show_all.replace(Some(Rc::new(show)));
    }

    fn open(&self, key: AlbumKey) {
        let open = self.open_album.borrow().clone();
        if let Some(open) = open {
            open(key);
        }
    }

    /// Shows the page, reading the library again when it changed.
    pub(crate) fn show(self: &Rc<Self>) {
        if self.stale.get() {
            self.reload();
        }
    }

    /// Marks the page as out of date; it reads the library when shown next,
    /// or now while it is shown.
    pub(crate) fn library_changed(self: &Rc<Self>) {
        self.stale.set(true);
        if self.root.is_mapped() {
            self.reload();
        }
    }

    fn reload(self: &Rc<Self>) {
        if self.loading.replace(true) {
            return;
        }
        self.stale.set(false);
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let data = backend
                .query(|library| load(library, SystemTime::now()))
                .await;
            let Some(view) = weak.upgrade() else { return };
            view.loading.set(false);
            match data {
                Ok(data) => view.fill(data),
                Err(error) => log::warn!("Home cannot be read: {}", error.0),
            }
        });
    }

    fn fill(self: &Rc<Self>, data: HomeData) {
        for (row, albums) in [(&self.added, &data.added), (&self.played, &data.played)] {
            row.clear();
            for album in albums {
                row.append(&self.tile(album));
            }
        }
        crate::widgets::remove_rows(&self.most);
        let top = data.most_played.first().map_or(1, |t| t.plays.max(1));
        for (rank, track) in data.most_played.iter().enumerate() {
            self.most.append(&most_played_row(rank + 1, track, top));
        }
        self.mark_most_played();
        crate::widgets::remove_rows(&self.rediscover);
        for (album, when) in &data.rediscover {
            self.rediscover.append(&self.rediscover_row(album, *when));
        }
        self.data.replace(data);
    }

    /// Rings the tiles of the playing album.
    fn mark_tiles(&self) {
        let playing = self.playing.get();
        for row in [&self.added, &self.played] {
            for child in children(row) {
                if let Some(tile) = child.downcast_ref::<AlbumTile>() {
                    tile.cover()
                        .set_playing(tile.key().is_some() && tile.key() == playing);
                }
            }
        }
    }

    /// Marks the Most played row of the playing track: the playing-row look,
    /// with the indicator in place of the rank.
    fn mark_most_played(&self) {
        let playing = self.playing_tracks.get();
        let mut child = self.most.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() else {
                continue;
            };
            let on = row
                .widget_name()
                .parse::<u64>()
                .is_ok_and(|key| playing.contains(TrackKey(key)));
            if on {
                row.add_css_class("hk-playing");
            } else {
                row.remove_css_class("hk-playing");
            }
            if let Some(lead) = row
                .child()
                .and_then(|line| line.first_child())
                .and_downcast::<gtk::Stack>()
            {
                lead.set_visible_child_name(if on { "playing" } else { "rank" });
            }
        }
    }

    fn tile(self: &Rc<Self>, album: &AlbumRow) -> gtk::Widget {
        // A button to screen readers: Enter plays the album, a click opens it.
        let tile = glib::Object::builder::<AlbumTile>()
            .property("accessible-role", gtk::AccessibleRole::Button)
            .build();
        tile.cover().set_shape(0, 6.0);
        tile.hide_meta();
        tile.set_focusable(true);
        let item = AlbumItem::new(album.key, 0, Some(Rc::new(album.clone())));
        tile.bind(&item, &self.covers);
        tile.cover()
            .set_playing(self.playing.get() == Some(album.key));
        tile.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{} by {}"),
            &[&album.title, &album.artist],
        ))]);
        let key = album.key;
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
                    actions::popup(&tile, Target::Album(key), Some((x, y)));
                }
                gdk::BUTTON_PRIMARY if presses == 2 => view.play_album(key),
                gdk::BUTTON_PRIMARY => view.open(key),
                _ => {}
            }
        });
        tile.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        let weak_tile = tile.downgrade();
        keys.connect_key_pressed(move |_, pressed, _, state| {
            let (Some(view), Some(tile)) = (weak.upgrade(), weak_tile.upgrade()) else {
                return glib::Propagation::Proceed;
            };
            if actions::is_menu_key(pressed, state) {
                actions::popup(&tile, Target::Album(key), None);
            } else if matches!(pressed, gdk::Key::Return | gdk::Key::KP_Enter) {
                view.play_album(key);
            } else {
                return glib::Propagation::Proceed;
            }
            glib::Propagation::Stop
        });
        tile.add_controller(keys);
        tile.upcast()
    }

    fn play_album(&self, key: AlbumKey) {
        self.backend.send(PlayerRequest::PlayAlbum {
            album: key,
            start: 0,
            shuffle: false,
        });
    }

    fn play_most_played(&self, index: i32) {
        let data = self.data.borrow();
        let Ok(start) = usize::try_from(index) else {
            return;
        };
        if start >= data.most_played.len() {
            return;
        }
        self.backend.send(PlayerRequest::PlayTracks {
            tracks: data.most_played.iter().map(|t| t.key).collect(),
            start,
            title: tr("Most played"),
            shuffle: false,
        });
    }

    fn rediscover_row(&self, album: &AlbumRow, when: Option<SystemTime>) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let cover = CoverArt::new(22, 3.0);
        cover.add_css_class("small");
        self.covers.show(&cover, album.cover, album.cover_seed, 22);
        line.append(&cover);
        let text = gtk::Label::new(None);
        text.set_xalign(0.0);
        text.set_hexpand(true);
        text.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.set_markup(&two_part_markup(&album.title, &album.artist));
        line.append(&text);
        line.append(&label(&last_played_text(when), &["hk-t3", "hk-secondary"]));
        row.set_child(Some(&line));
        row.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{} · {}"),
            &[&album.title, &album.artist],
        ))]);
        row
    }

    /// The first tile row, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.added
            .first_child()
            .unwrap_or_else(|| self.root.clone().upcast())
    }

    /// What the page shows, for tests: the tiles shown in each row, and the
    /// rows of the two lists in words.
    pub(crate) fn texts(&self) -> (usize, usize, Vec<String>, Vec<String>) {
        let data = self.data.borrow();
        let most = data
            .most_played
            .iter()
            .map(|t| format!("{} · {} {}", t.title, t.artist, t.plays))
            .collect();
        let rediscover = data
            .rediscover
            .iter()
            .map(|(a, when)| format!("{} · {} {}", a.title, a.artist, last_played_text(*when)))
            .collect();
        (self.added.shown(), self.played.shown(), most, rediscover)
    }
}

/// "Title · Artist" with the artist in `t3`.
fn two_part_markup(first: &str, second: &str) -> String {
    if second.is_empty() {
        return glib::markup_escape_text(first).to_string();
    }
    format!(
        "{} <span alpha=\"62%\">· {}</span>",
        glib::markup_escape_text(first),
        glib::markup_escape_text(second)
    )
}

fn most_played_row(rank: usize, track: &TrackRow, top: u32) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_widget_name(&track.key.0.to_string());
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    // The rank, or the playing indicator in its place.
    let lead = gtk::Stack::new();
    lead.set_hhomogeneous(true);
    lead.set_vhomogeneous(true);
    let number = label(&rank.to_string(), &["hk-t3", "hk-numeric", "hk-home-rank"]);
    number.set_xalign(1.0);
    lead.add_named(&number, Some("rank"));
    let indicator = PlayingIndicator::default();
    indicator.set_halign(gtk::Align::End);
    indicator.set_valign(gtk::Align::Center);
    lead.add_named(&indicator, Some("playing"));
    lead.set_visible_child_name("rank");
    line.append(&lead);
    let text = gtk::Label::new(None);
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.set_markup(&two_part_markup(&track.title, &track.artist));
    line.append(&text);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bar.add_css_class("hk-home-bar");
    bar.set_size_request(96, 3);
    bar.set_valign(gtk::Align::Center);
    let fill_width = (f64::from(track.plays) / f64::from(top) * 96.0).round();
    // In range by construction: 0 to 96.
    #[allow(clippy::cast_possible_truncation)]
    let fill_width = fill_width.clamp(0.0, 96.0) as i32;
    let filled = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    filled.add_css_class("hk-home-bar-fill");
    filled.set_size_request(fill_width, 3);
    filled.set_halign(gtk::Align::Start);
    bar.append(&filled);
    line.append(&bar);
    let plays = label(
        &format::count(track.plays),
        &["hk-numeric", "hk-home-plays"],
    );
    plays.set_xalign(1.0);
    line.append(&plays);
    row.set_child(Some(&line));
    row.update_property(&[gtk::accessible::Property::Label(&fill(
        &tr("{} · {}"),
        &[&track.title, &track.artist],
    ))]);
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_fill_one_row() {
        // 1188 px: seven tiles of 152 with six gaps of 20.
        assert_eq!(layout(1188, 12), (7, 152));
        // Fewer albums than fit: they keep the width of a full row.
        assert_eq!(layout(1188, 3), (3, 152));
        // Narrower than one tile: one, as wide as there is.
        assert_eq!(layout(100, 12), (1, 100));
    }

    #[test]
    fn never_played_reads_never() {
        assert_eq!(last_played_text(None), tr("Never"));
        assert!(
            !last_played_text(Some(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
            ))
            .is_empty()
        );
    }
}
