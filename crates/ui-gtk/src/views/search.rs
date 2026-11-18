// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Search results, shown while the search entry has text: artists, albums and
//! tracks matching the words, each group under a section label with its count,
//! the matched text in the accent. Up and Down in the entry move through the
//! rows; Enter plays a track or opens an artist or album. Without results the
//! page says so and offers to clear the search.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::backend::{Backend, PlayerRequest, SearchResults, TrackRow};
use crate::format;
use crate::i18n::{fill, tr};
use crate::models::CoverCache;
use crate::views::actions::{self, Target};
use crate::views::drag::DraggedTracks;
use crate::widgets::avatar::Avatar;
use crate::widgets::{CoverArt, TitleLine, label};
use crate::window::Page;

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

/// The search page.
pub(crate) struct SearchView {
    pub(crate) root: gtk::Stack,
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
    pub(crate) fn new(backend: &Backend, covers: &Rc<CoverCache>) -> Rc<Self> {
        let root = gtk::Stack::new();
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.add_css_class("hk-search");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        head.add_css_class("hk-search-head");
        let title = label("", &["hk-view-title"]);
        title.set_accessible_role(gtk::AccessibleRole::Heading);
        head.append(&title);
        let total = label("", &["hk-t3", "hk-numeric"]);
        total.set_valign(gtk::Align::Baseline);
        head.append(&total);
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

        let view = Rc::new(Self {
            root,
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
        });
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

    /// Searches for `text`.
    pub(crate) fn search(self: &Rc<Self>, text: &str) {
        let text = text.trim().to_owned();
        self.text.replace(text.clone());
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
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
            self.root.set_visible_child_name("none");
            self.results.replace(results);
            self.entries.borrow_mut().clear();
            self.root.announce(
                &fill(&tr("No results for “{}”"), &[text]),
                gtk::AccessibleAnnouncementPriority::Medium,
            );
            return;
        }
        self.root.set_visible_child_name("results");
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

    /// Moves the selection by `by` rows, skipping the section labels.
    pub(crate) fn move_selection(&self, by: i32) {
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
}
