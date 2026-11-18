// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Search results, shown while the search entry has text: artists, albums and
//! tracks matching the words, each group under a section label with its count,
//! the matched text in the accent. Up and Down in the entry move through the
//! rows; Enter plays a track or opens an artist or album. Without results the
//! page says so and offers to clear the search.
//!
//! Text with a field operator is a query of the query language: it lists the
//! tracks it selects in a table instead. A query that cannot be read keeps the
//! last one that could, with a notice that says what fails.
//!
//! A side column shows examples of the query syntax and the saved searches.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use helikodu_service::library::SyntaxError;

use crate::backend::{
    Backend, PlayerRequest, SavedSearchKey, SavedSearchRow, SearchResults, TrackColumn, TrackRow,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::playback::Playback;
use crate::query_text;
use crate::views::actions::{self, Target};
use crate::views::drag::DraggedTracks;
use crate::views::tracks::TracksView;
use crate::widgets::avatar::Avatar;
use crate::widgets::{CoverArt, TitleLine, label};
use crate::window::Page;

/// The examples of the side column: a query and what it shows.
fn examples() -> [(&'static str, String); 4] {
    [
        ("artist:kask year:2018..2021", tr("A field and a range")),
        (
            "rating>=4 -genre:ambient",
            tr("Compare, and exclude with a minus"),
        ),
        (
            "added<30d sort:added desc",
            tr("Relative dates and sorting"),
        ),
        (
            "title~\"^Intro\" OR duration<30s",
            tr("Regular expressions and OR"),
        ),
    ]
}

/// The columns of the query results: Title, Artist, Album, Genre, Added and
/// Length.
const QUERY_COLUMNS: &[(TrackColumn, i32, bool)] = &[
    (TrackColumn::Title, 130, true),
    (TrackColumn::Artist, 96, true),
    (TrackColumn::Album, 100, true),
    (TrackColumn::Genre, 80, false),
    (TrackColumn::Added, 90, false),
    (TrackColumn::Length, 80, false),
];

/// How many tracks are listed before "Show all".
const TRACKS_SHOWN: usize = 7;

/// What a row of the results stands for.
#[derive(Debug, Clone)]
enum Entry {
    Artist(crate::backend::ArtistKey),
    Album(crate::backend::AlbumKey),
    Track(usize),
    ShowAll,
}

type OpenPage = Box<dyn Fn(Page)>;
type ShowAll = Box<dyn Fn(String)>;
type Clear = Box<dyn Fn()>;
type UseText = Box<dyn Fn(String)>;

/// The search page.
pub(crate) struct SearchView {
    pub(crate) root: gtk::Box,
    stack: gtk::Stack,
    title: gtk::Label,
    total: gtk::Label,
    list: gtk::ListBox,
    empty_title: gtk::Label,
    backend: Backend,
    covers: Rc<CoverCache>,
    results: RefCell<SearchResults>,
    entries: RefCell<Vec<Entry>>,
    text: RefCell<String>,
    generation: Cell<u64>,
    open: RefCell<Option<OpenPage>>,
    show_all: RefCell<Option<ShowAll>>,
    clear: RefCell<Option<Clear>>,
    /// The query results.
    tracks: Rc<TracksView>,
    query_count: gtk::Label,
    query_save: gtk::Button,
    smart_save: gtk::Button,
    plain_save: gtk::Button,
    notice: gtk::Box,
    notice_message: gtk::Label,
    notice_example: gtk::Label,
    /// The last query that could be read, which the table shows.
    last_good: RefCell<Option<String>>,
    /// The saved searches.
    saved_list: gtk::ListBox,
    saved_empty: gtk::Label,
    saved: RefCell<Vec<SavedSearchRow>>,
    use_text: RefCell<Option<UseText>>,
}

impl std::fmt::Debug for SearchView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchView")
            .field("text", &self.text)
            .finish_non_exhaustive()
    }
}

/// `text` with the parts that match a word of `words` in the accent and bold.
fn highlighted(text: &str, words: &[String]) -> pango::AttrList {
    let attributes = pango::AttrList::new();
    let lower = text.to_lowercase();
    // Byte offsets carry over only where lower-casing keeps the lengths.
    if lower.len() != text.len() {
        return attributes;
    }
    let accent = crate::style::current().accent.text;
    for word in words.iter().filter(|w| !w.is_empty()) {
        let mut from = 0;
        while let Some(found) = lower[from..].find(word.as_str()) {
            let start = (from + found) as u32;
            let end = start + word.len() as u32;
            let mut color = pango::AttrColor::new_foreground(
                u16::from(accent.r) * 257,
                u16::from(accent.g) * 257,
                u16::from(accent.b) * 257,
            );
            color.set_start_index(start);
            color.set_end_index(end);
            attributes.insert(color);
            let mut weight = pango::AttrInt::new_weight(pango::Weight::Semibold);
            weight.set_start_index(start);
            weight.set_end_index(end);
            attributes.insert(weight);
            from += found + word.len().max(1);
        }
    }
    attributes
}

impl SearchView {
    /// The page over `backend`.
    pub(crate) fn new(
        backend: &Backend,
        covers: &Rc<CoverCache>,
        playback: &Rc<Playback>,
    ) -> Rc<Self> {
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        let root = stack.clone();
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.add_css_class("hk-search");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        head.add_css_class("hk-search-head");
        let title = label("", &["hk-view-title"]);
        title.set_accessible_role(gtk::AccessibleRole::Heading);
        head.append(&title);
        let total = label("", &["hk-t3", "hk-numeric"]);
        total.set_valign(gtk::Align::Baseline);
        total.set_hexpand(true);
        head.append(&total);
        let plain_save = save_button(&tr("Save search"), Some("user-bookmarks-symbolic"));
        head.append(&plain_save);
        page.append(&head);
        let list = gtk::ListBox::new();
        list.add_css_class("hk-results");
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.update_property(&[gtk::accessible::Property::Label(&tr("Search results"))]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        page.append(&scroller);
        root.add_named(&page, Some("results"));

        let none = gtk::Box::new(gtk::Orientation::Vertical, 0);
        none.add_css_class("hk-no-results");
        let empty_title = label("", &["hk-view-title"]);
        empty_title.set_accessible_role(gtk::AccessibleRole::Heading);
        none.append(&empty_title);
        let advice = label(
            &tr(
                "Nothing in your library matches. Check the spelling, or try fewer or shorter words.",
            ),
            &["hk-t2"],
        );
        advice.set_margin_top(14);
        advice.set_wrap(true);
        none.append(&advice);
        let fields = label(
            &tr("Search looks at titles, artists, albums, genres, composers and file paths."),
            &["hk-t3", "hk-secondary"],
        );
        fields.set_margin_top(8);
        fields.set_wrap(true);
        none.append(&fields);
        let clear = gtk::Button::with_label(&tr("Clear search"));
        clear.add_css_class("hk-btn");
        clear.add_css_class("small");
        clear.set_halign(gtk::Align::Start);
        clear.set_margin_top(18);
        none.append(&clear);
        root.add_named(&none, Some("none"));

        // The results of a query.
        let query_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        query_page.add_css_class("hk-query-results");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        head.add_css_class("hk-search-head");
        let query_title = label(&tr("Results"), &["hk-view-title"]);
        query_title.set_accessible_role(gtk::AccessibleRole::Heading);
        head.append(&query_title);
        let query_count = label("", &["hk-t3", "hk-numeric"]);
        query_count.set_valign(gtk::Align::Baseline);
        query_count.set_hexpand(true);
        head.append(&query_count);
        let query_save = save_button(&tr("Save search"), Some("user-bookmarks-symbolic"));
        head.append(&query_save);
        let smart_save = save_button(&tr("Save as smart playlist"), None);
        // Offered by the smart playlists, when they are there.
        smart_save.set_action_name(Some("win.new-smart-playlist"));
        head.append(&smart_save);
        query_page.append(&head);
        let notice = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        notice.add_css_class("hk-notice");
        notice.add_css_class("hk-query-notice");
        let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
        icon.set_valign(gtk::Align::Start);
        notice.append(&icon);
        let lines = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let first = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let notice_message = label("", &["hk-medium"]);
        notice_message.set_ellipsize(pango::EllipsizeMode::None);
        notice_message.set_wrap(true);
        notice_message.set_single_line_mode(false);
        first.append(&notice_message);
        let notice_example = label("", &["hk-mono", "hk-query-example"]);
        first.append(&notice_example);
        lines.append(&first);
        let still = label(
            &tr("The list still shows the last query that could be read."),
            &["hk-t3", "hk-hint"],
        );
        lines.append(&still);
        notice.append(&lines);
        notice.set_visible(false);
        query_page.append(&notice);
        let tracks = TracksView::new(backend, playback, true);
        tracks.show_columns(QUERY_COLUMNS);
        tracks.hide_status();
        tracks.unmark_default_order();
        tracks.root.set_vexpand(true);
        query_page.append(&tracks.root);
        root.add_named(&query_page, Some("query"));

        // The side column: the query syntax and the saved searches.
        let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side.add_css_class("hk-search-side");
        side.set_size_request(300, -1);
        let syntax = label(&tr("Query syntax"), &["hk-section-label"]);
        syntax.set_accessible_role(gtk::AccessibleRole::Heading);
        side.append(&syntax);
        let chips = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side.append(&chips);
        let saved_title = label(
            &tr("Saved searches"),
            &["hk-section-label", "hk-saved-title"],
        );
        saved_title.set_accessible_role(gtk::AccessibleRole::Heading);
        side.append(&saved_title);
        let saved_list = gtk::ListBox::new();
        saved_list.add_css_class("hk-saved");
        saved_list.set_selection_mode(gtk::SelectionMode::Single);
        saved_list.set_activate_on_single_click(true);
        saved_list.update_property(&[gtk::accessible::Property::Label(&tr("Saved searches"))]);
        side.append(&saved_list);
        let saved_empty = label(
            &tr("Save a search to find it here again."),
            &["hk-t3", "hk-hint"],
        );
        saved_empty.set_wrap(true);
        saved_empty.set_single_line_mode(false);
        saved_empty.set_visible(false);
        side.append(&saved_empty);
        let side_scroller = gtk::ScrolledWindow::new();
        side_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        side_scroller.set_child(Some(&side));
        side_scroller.add_css_class("hk-search-side-pane");

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&stack);
        root.append(&side_scroller);

        let view = Rc::new(Self {
            root,
            stack,
            title,
            total,
            list,
            empty_title,
            backend: backend.clone(),
            covers: covers.clone(),
            results: RefCell::new(SearchResults::default()),
            entries: RefCell::new(Vec::new()),
            text: RefCell::new(String::new()),
            generation: Cell::new(0),
            open: RefCell::new(None),
            show_all: RefCell::new(None),
            clear: RefCell::new(None),
            tracks,
            query_count,
            query_save,
            smart_save,
            plain_save,
            notice,
            notice_message,
            notice_example,
            last_good: RefCell::new(None),
            saved_list,
            saved_empty,
            saved: RefCell::new(Vec::new()),
            use_text: RefCell::new(None),
        });
        for (query, caption) in examples() {
            let chip = gtk::Button::with_label(query);
            chip.add_css_class("hk-query-chip");
            chip.set_halign(gtk::Align::Start);
            chip.set_tooltip_text(Some(&tr("Search with this query")));
            let weak = Rc::downgrade(&view);
            chip.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.use_text(query);
                }
            });
            chips.append(&chip);
            let caption = label(&caption, &["hk-query-caption"]);
            chips.append(&caption);
        }
        view.connect_query();
        let weak = Rc::downgrade(&view);
        clear.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade()
                && let Some(clear) = view.clear.borrow().as_ref()
            {
                clear();
            }
        });
        view.connect();
        // The matched text takes the accent of the scheme shown.
        let weak = Rc::downgrade(&view);
        crate::style::connect_changed(move |_| {
            if let Some(view) = weak.upgrade() {
                let text = view.text.borrow().clone();
                if !text.is_empty() {
                    view.search(&text);
                }
            }
        });
        view
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(view) = weak.upgrade() {
                view.activate(row);
            }
        });
        let weak = Rc::downgrade(self);
        self.list.connect_row_selected(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.show_hints();
            }
        });
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            let Some(row) = view.list.row_at_y(y as i32) else {
                return;
            };
            view.list.select_row(Some(&row));
            if let Some(target) = view.target() {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                actions::popup(&view.list, target, Some((x, y)));
            }
        });
        self.list.add_controller(click);
        // A track plays on a double click or Enter; an artist, an album or
        // "Show all" opens on a single click, as a tile does.
        self.list.set_activate_on_single_click(false);
        let open = gtk::GestureClick::new();
        open.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(self);
        open.connect_released(move |_, presses, _, y| {
            let Some(view) = weak.upgrade() else { return };
            if presses == 1
                && let Some(row) = view.list.row_at_y(y as i32)
                && !matches!(view.entry(&row), None | Some(Entry::Track(_)))
            {
                view.activate(&row);
            }
        });
        self.list.add_controller(open);
        let weak = Rc::downgrade(self);
        DraggedTracks::source(&self.list, move |_, y| {
            let view = weak.upgrade()?;
            let row = view.list.row_at_y(y as i32)?;
            let entry = view.entry(&row)?;
            let results = view.results.borrow();
            match entry {
                Entry::Track(i) => {
                    let track = results.tracks.get(i)?;
                    Some((
                        DraggedTracks {
                            keys: vec![track.key],
                            ..DraggedTracks::default()
                        },
                        track.title.clone(),
                    ))
                }
                Entry::Album(key) => Some((
                    DraggedTracks {
                        album: Some(key),
                        ..DraggedTracks::default()
                    },
                    results
                        .albums
                        .iter()
                        .find(|a| a.key == key)
                        .map(|a| a.title.clone())
                        .unwrap_or_default(),
                )),
                _ => None,
            }
        });
    }

    /// Calls `open` with the page of a chosen artist or album.
    pub(crate) fn connect_open(&self, open: impl Fn(Page) + 'static) {
        self.open.replace(Some(Box::new(open)));
    }

    /// Calls `show_all` with the text when "Show all tracks" is chosen.
    pub(crate) fn connect_show_all(&self, show_all: impl Fn(String) + 'static) {
        self.show_all.replace(Some(Box::new(show_all)));
    }

    /// Calls `clear` when "Clear search" is chosen.
    pub(crate) fn connect_clear(&self, clear: impl Fn() + 'static) {
        self.clear.replace(Some(Box::new(clear)));
    }

    /// Searches for `text`: words, or a query of the query language.
    pub(crate) fn search(self: &Rc<Self>, text: &str) {
        let text = text.trim().to_owned();
        self.text.replace(text.clone());
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let reading = query_text::read(&text);
        let is_query = reading.is_query;
        let good = reading.error.is_none();
        self.show_notice(reading.error.as_ref().filter(|_| is_query));
        if is_query {
            self.stack.set_visible_child_name("query");
            if good {
                self.last_good.replace(Some(text.clone()));
                self.smart_save
                    .set_action_target_value(Some(&text.to_variant()));
                self.tracks
                    .show_text(&text, &fill(&tr("Search for “{}”"), &[&text]));
            } else if self.last_good.borrow().is_none() {
                self.tracks.clear();
                self.show_query_count(0);
            }
            let saveable = self.last_good.borrow().is_some();
            self.query_save.set_sensitive(saveable);
            return;
        }
        self.last_good.replace(None);
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let query = text.clone();
            let results = backend.query(move |l| l.search(&query)).await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            view.show(&text, results.unwrap_or_default());
        });
    }

    fn show(&self, text: &str, results: SearchResults) {
        crate::widgets::remove_rows(&self.list);
        let count = results.artists.len() + results.albums.len() + results.track_total;
        if count == 0 {
            self.empty_title
                .set_text(&fill(&tr("No results for “{}”"), &[text]));
            self.stack.set_visible_child_name("none");
            self.results.replace(results);
            self.entries.borrow_mut().clear();
            self.root.announce(
                &fill(&tr("No results for “{}”"), &[text]),
                gtk::AccessibleAnnouncementPriority::Medium,
            );
            return;
        }
        self.stack.set_visible_child_name("results");
        self.title.set_text(&fill(&tr("Results for “{}”"), &[text]));
        self.total.set_text(&crate::format::count(count));
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        let mut entries = Vec::new();
        if !results.artists.is_empty() {
            self.section(&tr("Artists"), results.artists.len());
            for artist in &results.artists {
                let lead = Avatar::new(28);
                lead.set_name(&artist.name);
                let albums = fill(
                    &crate::i18n::ntr("{} album", "{} albums", artist.album_count),
                    &[&crate::format::count(artist.album_count)],
                );
                let tracks = fill(
                    &crate::i18n::ntr("{} track", "{} tracks", artist.track_count),
                    &[&crate::format::count(artist.track_count)],
                );
                self.row(
                    entries.len(),
                    lead.upcast_ref(),
                    &artist.name,
                    &fill(&tr("{} · {}"), &[&albums, &tracks]),
                    "",
                    &words,
                );
                entries.push(Entry::Artist(artist.key));
            }
        }
        if !results.albums.is_empty() {
            self.section(&tr("Albums"), results.albums.len());
            for album in &results.albums {
                let cover = CoverArt::new(28, 4.0);
                self.covers.show(&cover, album.cover, album.cover_seed, 28);
                let description = match album.year {
                    Some(year) => fill(&tr("{} · {}"), &[&album.artist, &year.to_string()]),
                    None => album.artist.clone(),
                };
                self.row(
                    entries.len(),
                    cover.upcast_ref(),
                    &album.title,
                    &description,
                    &format::short(&album.format),
                    &words,
                );
                entries.push(Entry::Album(album.key));
            }
        }
        if results.track_total > 0 {
            self.section(&tr("Tracks"), results.track_total);
            for (i, track) in results.tracks.iter().take(TRACKS_SHOWN).enumerate() {
                let cover = CoverArt::new(28, 4.0);
                self.covers.show(&cover, track.cover, track.cover_seed, 28);
                let description = if track.album_title.is_empty() {
                    track.artist.clone()
                } else {
                    fill(&tr("{} · {}"), &[&track.artist, &track.album_title])
                };
                // A track without a title shows its file name, as the queue does.
                let title = if track.title.is_empty() {
                    track
                        .path
                        .as_deref()
                        .and_then(std::path::Path::file_name)
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                } else {
                    track.title.clone()
                };
                self.row(
                    entries.len(),
                    cover.upcast_ref(),
                    &title,
                    &description,
                    &track.duration.map(format::duration).unwrap_or_default(),
                    &words,
                );
                if let Some(line) = self.list.last_child().and_then(|r| r.first_child())
                    && let Some(name) = line
                        .first_child()
                        .and_then(|lead| lead.next_sibling())
                        .and_downcast::<TitleLine>()
                {
                    name.set_copies(track.key, track.copies);
                }
                entries.push(Entry::Track(i));
            }
            if results.track_total > TRACKS_SHOWN {
                let row = gtk::ListBoxRow::new();
                row.set_widget_name(&entries.len().to_string());
                row.add_css_class("hk-show-all");
                let text = fill(
                    &tr("Show all {} tracks"),
                    &[&results.track_total.to_string()],
                );
                row.set_child(Some(&label(&text, &["hk-t2"])));
                row.update_property(&[gtk::accessible::Property::Label(&text)]);
                self.list.append(&row);
                entries.push(Entry::ShowAll);
            }
        }
        self.entries.replace(entries);
        self.results.replace(results);
        if let Some(first) = self.first_row() {
            self.list.select_row(Some(&first));
        }
    }

    fn section(&self, text: &str, count: usize) {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);
        row.set_selectable(false);
        row.set_focusable(false);
        row.add_css_class("hk-result-section");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        for (text, classes) in [
            (text.to_owned(), &["hk-section-label"][..]),
            (count.to_string(), &["hk-section-label", "hk-numeric"][..]),
        ] {
            let l = gtk::Label::new(Some(&text));
            for class in classes {
                l.add_css_class(class);
            }
            line.append(&l);
        }
        row.set_child(Some(&line));
        row.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{}, {}"),
            &[text, &count.to_string()],
        ))]);
        self.list.append(&row);
    }

    fn row(
        &self,
        index: usize,
        lead: &gtk::Widget,
        title: &str,
        description: &str,
        right: &str,
        words: &[String],
    ) {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&index.to_string());
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.append(lead);
        let name = TitleLine::new(title, &["hk-medium"]);
        name.label()
            .set_attributes(Some(&highlighted(title, words)));
        line.append(&name);
        let detail = label(description, &["hk-t2"]);
        detail.set_hexpand(true);
        detail.set_max_width_chars(1);
        line.append(&detail);
        let end = gtk::Stack::new();
        end.set_size_request(96, -1);
        let value = label(right, &["hk-t3", "hk-mono-meta"]);
        value.set_xalign(1.0);
        end.add_named(&value, Some("value"));
        end.add_named(&label("", &["hk-t3", "hk-hint"]), Some("hint"));
        if let Some(hint) = end.child_by_name("hint").and_downcast::<gtk::Label>() {
            hint.set_xalign(1.0);
        }
        line.append(&end);
        row.set_child(Some(&line));
        row.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{}, {}"),
            &[title, description],
        ))]);
        self.list.append(&row);
    }

    fn entry(&self, row: &gtk::ListBoxRow) -> Option<Entry> {
        let index = row.widget_name().parse::<usize>().ok()?;
        self.entries.borrow().get(index).cloned()
    }

    /// The selected row shows what Enter does.
    fn show_hints(&self) {
        let selected = self.list.selected_row();
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() else {
                continue;
            };
            let Some(end) = row
                .child()
                .and_then(|line| line.last_child())
                .and_downcast::<gtk::Stack>()
            else {
                continue;
            };
            let on = Some(row) == selected.as_ref();
            if on && let Some(hint) = end.child_by_name("hint").and_downcast::<gtk::Label>() {
                hint.set_text(&match self.entry(row) {
                    Some(Entry::Track(_)) => tr("Enter to play"),
                    _ => tr("Enter to open"),
                });
            }
            end.set_visible_child_name(if on { "hint" } else { "value" });
        }
    }

    fn first_row(&self) -> Option<gtk::ListBoxRow> {
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && row.is_selectable()
            {
                return Some(row.clone());
            }
            child = widget.next_sibling();
        }
        None
    }

    /// Whether the query results are shown.
    fn showing_query(&self) -> bool {
        self.stack.visible_child_name().as_deref() == Some("query")
    }

    /// Moves the selection by `by` rows, skipping the section labels.
    pub(crate) fn move_selection(&self, by: i32) {
        if self.showing_query() {
            self.tracks.move_selection(by);
            return;
        }
        let mut rows = Vec::new();
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && row.is_selectable()
            {
                rows.push(row.clone());
            }
            child = widget.next_sibling();
        }
        if rows.is_empty() {
            return;
        }
        let current = self
            .list
            .selected_row()
            .and_then(|s| rows.iter().position(|r| *r == s));
        let next = match current {
            Some(i) => (i as i32 + by).clamp(0, rows.len() as i32 - 1) as usize,
            None => 0,
        };
        self.list.select_row(Some(&rows[next]));
        // Keep it in view.
        if let Some(adjustment) = self.list.adjustment() {
            let row = &rows[next];
            let y = row
                .compute_point(&self.list, &gtk::graphene::Point::new(0.0, 0.0))
                .map_or(0.0, |p| f64::from(p.y()));
            let h = f64::from(row.height());
            if y < adjustment.value() {
                adjustment.set_value(y);
            } else if y + h > adjustment.value() + adjustment.page_size() {
                adjustment.set_value(y + h - adjustment.page_size());
            }
        }
    }

    /// Plays or opens the selected row (Enter in the entry).
    pub(crate) fn activate_selected(&self) {
        if self.showing_query() {
            self.tracks.play_selected();
            return;
        }
        if let Some(row) = self.list.selected_row() {
            self.activate(&row);
        }
    }

    fn activate(&self, row: &gtk::ListBoxRow) {
        let Some(entry) = self.entry(row) else { return };
        match entry {
            Entry::Artist(key) => {
                if let Some(open) = self.open.borrow().as_ref() {
                    open(Page::Artist(key));
                }
            }
            Entry::Album(key) => {
                if let Some(open) = self.open.borrow().as_ref() {
                    open(Page::Album(key));
                }
            }
            Entry::Track(index) => {
                let results = self.results.borrow();
                let tracks: Vec<_> = results.tracks.iter().map(|t| t.key).collect();
                self.backend.send(PlayerRequest::PlayTracks {
                    tracks,
                    start: index,
                    title: fill(&tr("Search for “{}”"), &[&self.text.borrow()]),
                    shuffle: false,
                });
            }
            Entry::ShowAll => {
                if let Some(show_all) = self.show_all.borrow().as_ref() {
                    show_all(self.text.borrow().clone());
                }
            }
        }
    }

    /// The selected row as a context menu target.
    pub(crate) fn target(&self) -> Option<Target> {
        if self.showing_query() {
            return self.tracks.target();
        }
        let row = self.list.selected_row()?;
        Some(match self.entry(&row)? {
            Entry::Artist(key) => Target::Artist(key),
            Entry::Album(key) => Target::Album(key),
            Entry::Track(index) => {
                let track: TrackRow = self.results.borrow().tracks.get(index)?.clone();
                Target::tracks(vec![Rc::new(track)])
            }
            Entry::ShowAll => return None,
        })
    }

    /// The number of artists, albums and tracks found (for tests).
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        let results = self.results.borrow();
        (
            results.artists.len(),
            results.albums.len(),
            results.track_total,
        )
    }

    /// The number of tracks the query results list, if they are shown, and
    /// whether the notice is shown (for tests).
    pub(crate) fn query_state(&self) -> Option<(usize, bool)> {
        self.showing_query()
            .then(|| (self.tracks.keys().len(), self.notice.is_visible()))
    }

    /// The names of the saved searches (for tests).
    pub(crate) fn saved_names(&self) -> Vec<String> {
        self.saved.borrow().iter().map(|s| s.name.clone()).collect()
    }

    /// Calls `use_text` with a query to put in the search entry.
    pub(crate) fn connect_use_text(&self, use_text: impl Fn(String) + 'static) {
        self.use_text.replace(Some(Box::new(use_text)));
    }

    fn use_text(&self, text: &str) {
        if let Some(use_text) = self.use_text.borrow().as_ref() {
            use_text(text.to_owned());
        }
    }

    fn show_query_count(&self, count: usize) {
        self.query_count.set_text(&fill(
            &ntr("{} track", "{} tracks", count),
            &[&format::count(count)],
        ));
    }

    /// Shows what fails in the text, or hides the notice.
    fn show_notice(&self, error: Option<&SyntaxError>) {
        let Some(error) = error else {
            self.notice.set_visible(false);
            return;
        };
        self.notice.set_visible(true);
        self.notice_message.set_text(&query_text::message(error));
        match error.example() {
            Some(example) => {
                self.notice_example
                    .set_text(&fill(&tr("for example {}"), &[example]));
                self.notice_example.set_visible(true);
            }
            None => self.notice_example.set_visible(false),
        }
    }

    fn connect_query(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.tracks.connect_loaded(move |count| {
            if let Some(view) = weak.upgrade() {
                view.show_query_count(count);
            }
        });
        for button in [&self.query_save, &self.plain_save] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |button| {
                if let Some(view) = weak.upgrade() {
                    view.ask_name(button.upcast_ref(), None);
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.saved_list.connect_row_activated(move |_, row| {
            let Some(view) = weak.upgrade() else { return };
            let text = view
                .saved
                .borrow()
                .get(row.index().max(0) as usize)
                .map(|s| s.text.clone());
            if let Some(text) = text {
                view.use_text(&text);
            }
        });
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            let Some(row) = view.saved_list.row_at_y(y as i32) else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            view.saved_list.select_row(Some(&row));
            view.saved_menu(&row, Some((x, y)));
        });
        self.saved_list.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let Some(row) = view.saved_list.selected_row() else {
                return glib::Propagation::Proceed;
            };
            if actions::is_menu_key(key, state) {
                view.saved_menu(&row, None);
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::Delete {
                if let Some(saved) = view.saved_at(&row) {
                    view.delete_saved(saved);
                }
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::F2 {
                if let Some(saved) = view.saved_at(&row) {
                    view.ask_name(row.upcast_ref(), Some(saved));
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.saved_list.add_controller(keys);
    }

    fn saved_at(&self, row: &gtk::ListBoxRow) -> Option<SavedSearchRow> {
        self.saved
            .borrow()
            .get(row.index().max(0) as usize)
            .cloned()
    }

    /// The menu of a saved search: open, rename, delete.
    fn saved_menu(self: &Rc<Self>, row: &gtk::ListBoxRow, at: Option<(f64, f64)>) {
        let Some(saved) = self.saved_at(row) else {
            return;
        };
        let group = gio::SimpleActionGroup::new();
        for name in ["open", "rename", "delete"] {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            let saved = saved.clone();
            let row = row.downgrade();
            action.connect_activate(move |action, _| {
                let Some(view) = weak.upgrade() else { return };
                match action.name().as_str() {
                    "open" => view.use_text(&saved.text),
                    "rename" => {
                        if let Some(row) = row.upgrade() {
                            view.ask_name(row.upcast_ref(), Some(saved.clone()));
                        }
                    }
                    _ => view.delete_saved(saved.clone()),
                }
            });
            group.add_action(&action);
        }
        self.saved_list.insert_action_group("saved", Some(&group));
        let menu = gio::Menu::new();
        menu.append(Some(&tr("Open")), Some("saved.open"));
        let edit = gio::Menu::new();
        edit.append(Some(&tr("Rename…")), Some("saved.rename"));
        edit.append(Some(&tr("Delete…")), Some("saved.delete"));
        menu.append_section(None, &edit);
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);
        popover.set_parent(&self.saved_list);
        let (x, y) = at.unwrap_or_else(|| {
            let bounds = row.compute_bounds(&self.saved_list);
            bounds.map_or((0.0, 0.0), |b| {
                (f64::from(b.x() + 24.0), f64::from(b.y() + b.height()))
            })
        });
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
    }

    /// Asks for a name under `anchor`: for a new saved search of the text
    /// shown, or a new name of `saved`.
    fn ask_name(self: &Rc<Self>, anchor: &gtk::Widget, saved: Option<SavedSearchRow>) {
        let popover = gtk::Popover::new();
        popover.add_css_class("hk-rename");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.set_width_chars(24);
        entry.set_placeholder_text(Some(&tr("Name")));
        entry.update_property(&[gtk::accessible::Property::Label(&tr("Name of the search"))]);
        match &saved {
            Some(saved) => entry.set_text(&saved.name),
            None => entry.set_text(&self.text.borrow()),
        }
        line.append(&entry);
        let commit_label = if saved.is_some() {
            tr("Rename")
        } else {
            tr("Save")
        };
        let commit_button = gtk::Button::with_label(&commit_label);
        commit_button.add_css_class("hk-btn");
        commit_button.add_css_class("primary");
        commit_button.add_css_class("compact");
        line.append(&commit_button);
        popover.set_child(Some(&line));
        popover.set_parent(anchor);
        popover.set_position(gtk::PositionType::Bottom);
        let weak = Rc::downgrade(self);
        let commit = Rc::new(move |entry: &gtk::Entry, popover: &gtk::Popover| {
            let name = entry.text().trim().to_owned();
            popover.popdown();
            let Some(view) = weak.upgrade() else { return };
            if name.is_empty() {
                return;
            }
            match &saved {
                Some(saved) if saved.name != name => {
                    let key = saved.key;
                    view.change_saved(move |l| l.rename_saved_search(key, &name));
                }
                Some(_) => {}
                None => {
                    // What the page shows: the last query that could be read,
                    // or the words.
                    let text = view
                        .last_good
                        .borrow()
                        .clone()
                        .unwrap_or_else(|| view.text.borrow().clone());
                    let query = query_text::read(&text).query;
                    view.change_saved(move |l| l.save_search(&name, &query).map(|_| ()));
                }
            }
        });
        let (c, p) = (commit.clone(), popover.downgrade());
        entry.connect_activate(move |entry| {
            if let Some(p) = p.upgrade() {
                c(entry, &p);
            }
        });
        let (c, p, e) = (commit, popover.downgrade(), entry.downgrade());
        commit_button.connect_clicked(move |_| {
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

    /// Asks before deleting `saved`.
    fn delete_saved(self: &Rc<Self>, saved: SavedSearchRow) {
        let dialog = gtk::AlertDialog::builder()
            .message(fill(&tr("Delete the saved search “{}”?"), &[&saved.name]))
            .detail(tr("The tracks it finds stay in the library."))
            .buttons([tr("Cancel"), tr("Delete")])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let window = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        let key: SavedSearchKey = saved.key;
        dialog.choose(window.as_ref(), gio::Cancellable::NONE, move |answer| {
            if let (Some(view), Ok(1)) = (weak.upgrade(), answer) {
                view.change_saved(move |l| l.delete_saved_search(key));
            }
        });
    }

    /// Runs `change` on the saved searches, then lists them again; a failure
    /// becomes a toast.
    fn change_saved(
        self: &Rc<Self>,
        change: impl FnOnce(
            &dyn crate::backend::LibrarySource,
        ) -> Result<(), crate::backend::BackendError>
        + Send
        + 'static,
    ) {
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend.query(move |l| change(l)).await;
            let Some(view) = weak.upgrade() else { return };
            if let Err(error) = result
                && let Some(window) = view.root.root().and_downcast::<crate::window::MainWindow>()
            {
                window.toast(&error.0, true);
            }
            view.refresh_saved();
        });
    }

    /// Lists the saved searches again, with their counts.
    pub(crate) fn refresh_saved(self: &Rc<Self>) {
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let saved = backend.query(|l| l.saved_searches()).await;
            let Some(view) = weak.upgrade() else { return };
            view.show_saved(saved.unwrap_or_default());
        });
    }

    fn show_saved(&self, saved: Vec<SavedSearchRow>) {
        crate::widgets::remove_rows(&self.saved_list);
        for search in &saved {
            let row = gtk::ListBoxRow::new();
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let icon = gtk::Image::from_icon_name("user-bookmarks-symbolic");
            icon.add_css_class("hk-t2");
            line.append(&icon);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
            text.set_hexpand(true);
            text.set_valign(gtk::Align::Center);
            text.append(&label(&search.name, &[]));
            text.append(&label(&search.text, &["hk-t3", "hk-mono-meta"]));
            line.append(&text);
            let count = label(&format::count(search.count), &["hk-t3", "hk-numeric"]);
            count.set_xalign(1.0);
            line.append(&count);
            row.set_child(Some(&line));
            row.set_tooltip_text(Some(&search.text));
            row.update_property(&[gtk::accessible::Property::Label(&fill(
                &tr("{}, {}"),
                &[
                    &search.name,
                    &fill(
                        &ntr("{} track", "{} tracks", search.count),
                        &[&format::count(search.count)],
                    ),
                ],
            ))]);
            self.saved_list.append(&row);
        }
        self.saved_empty.set_visible(saved.is_empty());
        self.saved_list.set_visible(!saved.is_empty());
        self.saved.replace(saved);
    }
}

/// A small button of a results head.
fn save_button(text: &str, icon: Option<&str>) -> gtk::Button {
    let button = gtk::Button::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    if let Some(icon) = icon {
        content.append(&gtk::Image::from_icon_name(icon));
    }
    content.append(&gtk::Label::new(Some(text)));
    button.set_child(Some(&content));
    button.add_css_class("hk-btn");
    button.add_css_class("small");
    button.set_valign(gtk::Align::Center);
    button
}
