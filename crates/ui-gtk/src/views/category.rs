// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library, Composers and Library, Years: the layout of Genres, a list at the
//! left and the albums of the chosen entry in a grid at the right.
//!
//! - Composers lists the people in the composer role by sort name; an album with
//!   several composers is under each of them.
//! - Years lists decades and, under each, its years, newest first; a decade can be
//!   chosen too and then shows every album of the decade. The albums without a year
//!   are under "Unknown", the last row.
//!
//! Choosing an entry runs a new album query; the grid never filters in memory.
//! "Play all" plays the entry's tracks without audiobooks.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::{
    AlbumKey, AlbumScope, AlbumSort, ArtistKey, ArtistRow, Backend, PlayerRequest, YearRange,
    YearRow,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::playback::Playback;
use crate::views::AlbumsView;
use crate::views::genres::{list_pane, sort_button};
use crate::widgets::empty_state::empty_state;
use crate::widgets::label;

/// Which list the view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Category {
    /// The people in the composer role.
    Composers,
    /// The release years and decades.
    Years,
}

impl Category {
    fn name(self) -> &'static str {
        match self {
            Self::Composers => "composers",
            Self::Years => "years",
        }
    }
}

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// The albums it shows.
    scope: AlbumScope,
    /// The name.
    name: String,
    /// The number of albums.
    albums: usize,
    /// The number of tracks.
    tracks: usize,
    /// A decade row, drawn as a section label.
    decade: bool,
}

/// The rows of the Years list: each decade, then its years.
fn year_entries(years: &[YearRow]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut unknown = None;
    for row in years {
        let Some(year) = row.year else {
            unknown = Some(Entry {
                scope: AlbumScope::Years(YearRange::Unknown),
                name: tr("Unknown"),
                albums: row.album_count,
                tracks: row.track_count,
                decade: false,
            });
            continue;
        };
        let decade = YearRange::decade_of(year);
        let heading = entries
            .iter()
            .rposition(|e| e.decade)
            .filter(|&i| entries[i].scope == AlbumScope::Years(decade));
        let index = match heading {
            Some(index) => index,
            None => {
                let first = year.div_euclid(10) * 10;
                entries.push(Entry {
                    scope: AlbumScope::Years(decade),
                    name: fill(&tr("{}s"), &[&first.to_string()]),
                    albums: 0,
                    tracks: 0,
                    decade: true,
                });
                entries.len() - 1
            }
        };
        entries[index].albums += row.album_count;
        entries[index].tracks += row.track_count;
        entries.push(Entry {
            scope: AlbumScope::Years(YearRange::of(Some(year))),
            name: year.to_string(),
            albums: row.album_count,
            tracks: row.track_count,
            decade: false,
        });
    }
    entries.extend(unknown);
    entries
}

fn composer_entries(rows: &[ArtistRow]) -> Vec<Entry> {
    rows.iter()
        .map(|c| Entry {
            scope: AlbumScope::Composer(c.key),
            name: c.name.clone(),
            albums: c.album_count,
            tracks: c.track_count,
            decade: false,
        })
        .collect()
}

/// The Composers or the Years view.
pub(crate) struct CategoryView {
    pub(crate) root: gtk::Box,
    category: Category,
    count: gtk::Label,
    list: gtk::ListBox,
    detail: gtk::Stack,
    name: gtk::Label,
    summary: gtk::Label,
    sort_button: gtk::MenuButton,
    albums: AlbumsView,
    backend: Backend,
    entries: RefCell<Vec<Entry>>,
    current: Cell<Option<AlbumScope>>,
    loaded: Cell<bool>,
    quiet: Cell<bool>,
    sort: Cell<AlbumSort>,
    generation: Cell<u64>,
}

impl std::fmt::Debug for CategoryView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CategoryView")
            .field("category", &self.category)
            .finish_non_exhaustive()
    }
}

impl CategoryView {
    /// The view of `category` over `backend`; `open_album` shows an album page.
    pub(crate) fn new(
        category: Category,
        backend: &Backend,
        playback: &Rc<Playback>,
        covers: &Rc<CoverCache>,
        open_album: impl Fn(AlbumKey) + 'static,
    ) -> Rc<Self> {
        let group_name = category.name();
        let (title, section, icon, none_title, none_text, sort) = match category {
            Category::Composers => (
                tr("Composers"),
                tr("Composer"),
                "helikodu-composer-symbolic",
                tr("No composer chosen"),
                tr("Choose a composer to see their albums."),
                AlbumSort::Year,
            ),
            Category::Years => (
                tr("Years"),
                tr("Year"),
                "helikodu-calendar-symbolic",
                tr("No year chosen"),
                tr("Choose a year to see its albums."),
                AlbumSort::AlbumArtist,
            ),
        };
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let order = gio::Menu::new();
        match category {
            Category::Composers => {
                order.append(
                    Some(&tr("Name")),
                    Some(&format!("{group_name}.order::name")),
                );
            }
            Category::Years => {
                order.append(
                    Some(&tr("Newest first")),
                    Some(&format!("{group_name}.order::name")),
                );
            }
        }
        order.append(
            Some(&tr("Number of albums")),
            Some(&format!("{group_name}.order::count")),
        );
        let order_button = gtk::MenuButton::new();
        order_button.set_icon_name("view-sort-descending-symbolic");
        order_button.add_css_class("hk-menu");
        order_button.set_menu_model(Some(&order));
        crate::widgets::menu_names::name_menus(&order_button);
        let order_text = match category {
            Category::Composers => tr("Sort the composers"),
            Category::Years => tr("Sort the years"),
        };
        order_button.set_tooltip_text(Some(&order_text));
        order_button.update_property(&[gtk::accessible::Property::Label(&order_text)]);
        let (pane, count, list) = list_pane(&title, order_button.upcast_ref());
        root.append(&pane);

        let detail = gtk::Stack::new();
        detail.set_hexpand(true);
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("hk-detail-header");
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        titles.append(&label(&section, &["hk-section-label"]));
        let name = label("", &["hk-hero-title", "small"]);
        name.set_accessible_role(gtk::AccessibleRole::Heading);
        titles.append(&name);
        let summary = label("", &["hk-t3"]);
        titles.append(&summary);
        header.append(&titles);
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
        let sorts = gio::Menu::new();
        for (target, text) in [
            ("artist", tr("Album artist")),
            ("title", tr("Title")),
            ("year", tr("Year")),
            ("added", tr("Date added")),
        ] {
            sorts.append(Some(&text), Some(&format!("{group_name}.sort::{target}")));
        }
        let sort_button = sort_button(&sort_label(sort), &sorts);
        for w in [
            play.upcast_ref::<gtk::Widget>(),
            shuffle.upcast_ref(),
            sort_button.upcast_ref(),
        ] {
            w.set_valign(gtk::Align::End);
            header.append(w);
        }
        page.append(&header);
        let albums = AlbumsView::default();
        albums.bind(backend, playback, covers, open_album);
        albums.set_vexpand(true);
        page.append(&albums);
        detail.add_named(&page, Some("entry"));
        detail.add_named(
            &empty_state(icon, &none_title, &none_text, &[]),
            Some("none"),
        );
        detail.set_visible_child_name("none");
        root.append(&detail);

        let view = Rc::new(Self {
            root,
            category,
            count,
            list,
            detail,
            name,
            summary,
            sort_button,
            albums,
            backend: backend.clone(),
            entries: RefCell::new(Vec::new()),
            current: Cell::new(None),
            loaded: Cell::new(false),
            quiet: Cell::new(false),
            sort: Cell::new(sort),
            generation: Cell::new(0),
        });
        view.connect(&play, &shuffle);
        view
    }

    fn connect(self: &Rc<Self>, play: &gtk::Button, shuffle: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_selected(move |_, row| {
            let (Some(view), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            if view.quiet.get() {
                return;
            }
            let entry = usize::try_from(row.index())
                .ok()
                .and_then(|i| view.shown_entry(i));
            if let Some(entry) = entry {
                view.open(&entry);
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
            view.fill(name == "count");
        });
        group.add_action(&order);
        let sort = gio::SimpleAction::new_stateful(
            "sort",
            Some(glib::VariantTy::STRING),
            &sort_target(self.sort.get()).to_variant(),
        );
        let weak = Rc::downgrade(self);
        sort.connect_activate(move |action, target| {
            let (Some(view), Some(name)) = (weak.upgrade(), target.and_then(|t| t.str())) else {
                return;
            };
            action.set_state(&name.to_variant());
            let sort = match name {
                "title" => AlbumSort::Title,
                "year" => AlbumSort::Year,
                "added" => AlbumSort::DateAdded,
                _ => AlbumSort::AlbumArtist,
            };
            view.sort_button.set_label(&sort_label(sort));
            view.sort.set(sort);
            if let Some(scope) = view.current.get() {
                view.albums.set_embedded(scope, sort);
            }
        });
        group.add_action(&sort);
        self.root
            .insert_action_group(self.category.name(), Some(&group));
    }

    /// The entry of the list row at `index`.
    fn shown_entry(&self, index: usize) -> Option<Entry> {
        let row = self.list.row_at_index(i32::try_from(index).ok()?)?;
        let at: usize = row.widget_name().parse().ok()?;
        self.entries.borrow().get(at).cloned()
    }

    /// Shows the list, reading it the first time.
    pub(crate) fn show(self: &Rc<Self>) {
        if !self.loaded.get() {
            self.load();
        }
    }

    fn load(self: &Rc<Self>) {
        self.loaded.set(true);
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        let category = self.category;
        glib::spawn_future_local(async move {
            let entries = match category {
                Category::Composers => backend
                    .query(|l| l.composers())
                    .await
                    .map(|rows| composer_entries(&rows)),
                Category::Years => backend
                    .query(|l| l.years())
                    .await
                    .map(|rows| year_entries(&rows)),
            }
            .unwrap_or_default();
            if let Some(view) = weak.upgrade() {
                view.entries.replace(entries);
                view.fill(false);
            }
        });
    }

    /// Fills the list, by name (or year) or by the number of albums.
    fn fill(self: &Rc<Self>, by_count: bool) {
        self.quiet.set(true);
        crate::widgets::remove_rows(&self.list);
        let entries = self.entries.borrow().clone();
        let mut order: Vec<usize> = (0..entries.len()).collect();
        if by_count {
            // Without the decades, which would only repeat their years.
            order.retain(|&i| !entries[i].decade);
            order.sort_by_key(|&i| std::cmp::Reverse(entries[i].albums));
        }
        let listed = entries.iter().filter(|e| !e.decade).count();
        self.count.set_text(&format::count(listed));
        for i in order {
            let entry = &entries[i];
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&i.to_string());
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let classes: &[&str] = if entry.decade {
                &["hk-section-label"]
            } else {
                &[]
            };
            let name = label(&entry.name, classes);
            name.set_hexpand(true);
            line.append(&name);
            let count = label(&format::count(entry.albums), &["hk-count"]);
            line.append(&count);
            row.set_child(Some(&line));
            if entry.decade {
                row.add_css_class("hk-decade");
            }
            row.update_property(&[gtk::accessible::Property::Label(&fill(
                &ntr("{}, {} album", "{}, {} albums", entry.albums),
                &[&entry.name, &entry.albums.to_string()],
            ))]);
            self.list.append(&row);
        }
        self.quiet.set(false);
        let wanted = self.current.get().or_else(|| {
            entries
                .iter()
                .find(|e| !e.decade)
                .or(entries.first())
                .map(|e| e.scope)
        });
        let entry = entries.iter().find(|e| Some(e.scope) == wanted).cloned();
        match entry {
            Some(entry) => self.open(&entry),
            None => {
                self.current.set(None);
                self.detail.set_visible_child_name("none");
            }
        }
    }

    fn open(self: &Rc<Self>, entry: &Entry) {
        self.quiet.set(true);
        let at = self
            .entries
            .borrow()
            .iter()
            .position(|e| e.scope == entry.scope);
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && at.is_some_and(|at| row.widget_name() == at.to_string())
            {
                self.list.select_row(Some(row));
            }
            child = widget.next_sibling();
        }
        self.quiet.set(false);
        self.name.set_text(&entry.name);
        let albums = fill(
            &ntr("{} album", "{} albums", entry.albums),
            &[&format::count(entry.albums)],
        );
        let tracks = fill(
            &ntr("{} track", "{} tracks", entry.tracks),
            &[&format::count(entry.tracks)],
        );
        let summary = fill(&tr("{} · {}"), &[&albums, &tracks]);
        self.summary.set_text(&summary);
        self.detail.set_visible_child_name("entry");
        self.current.set(Some(entry.scope));
        self.albums.set_embedded(entry.scope, self.sort.get());
        if let AlbumScope::Composer(composer) = entry.scope {
            self.add_total_time(composer, summary);
        }
    }

    /// Adds the total time of `composer`'s tracks to the summary line.
    fn add_total_time(self: &Rc<Self>, composer: ArtistKey, summary: String) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let total = backend
                .query(move |l| l.composer_duration(composer))
                .await
                .unwrap_or(Duration::ZERO);
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation || total.is_zero() {
                return;
            }
            view.summary
                .set_text(&fill(&tr("{} · {}"), &[&summary, &format::minutes(total)]));
        });
    }

    fn play(&self, shuffle: bool) {
        let Some(scope) = self.current.get() else {
            return;
        };
        match scope {
            AlbumScope::Composer(composer) => self
                .backend
                .send(PlayerRequest::PlayComposer { composer, shuffle }),
            AlbumScope::Years(years) => self
                .backend
                .send(PlayerRequest::PlayYears { years, shuffle }),
            _ => {}
        }
    }

    /// Reloads after the library changed.
    pub(crate) fn library_changed(self: &Rc<Self>) {
        if self.loaded.get() {
            self.load();
            self.albums.reload();
        }
    }

    /// The list, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.list.clone().upcast()
    }

    /// The rows listed, decades included (for tests).
    pub(crate) fn count(&self) -> usize {
        self.entries.borrow().len()
    }

    /// The name shown in the detail header (for tests).
    pub(crate) fn shown_name(&self) -> String {
        self.name.text().into()
    }

    /// Chooses the list row at `index` (for tests).
    pub(crate) fn choose(&self, index: usize) {
        if let Some(row) = i32::try_from(index)
            .ok()
            .and_then(|i| self.list.row_at_index(i))
        {
            self.list.select_row(Some(&row));
        }
    }
}

fn sort_label(sort: AlbumSort) -> String {
    match sort {
        AlbumSort::Title => tr("Title"),
        AlbumSort::Year => tr("Year"),
        AlbumSort::DateAdded => tr("Date added"),
        AlbumSort::AlbumArtist => tr("Album artist"),
    }
}

fn sort_target(sort: AlbumSort) -> &'static str {
    match sort {
        AlbumSort::Title => "title",
        AlbumSort::Year => "year",
        AlbumSort::DateAdded => "added",
        AlbumSort::AlbumArtist => "artist",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_are_grouped_by_decade_with_unknown_last() {
        let row = |year, albums| YearRow {
            year,
            album_count: albums,
            track_count: albums * 10,
        };
        let entries = year_entries(&[
            row(Some(2021), 3),
            row(Some(2020), 2),
            row(Some(2019), 4),
            row(None, 1),
        ]);
        let shown: Vec<(&str, usize, bool)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.albums, e.decade))
            .collect();
        assert_eq!(
            shown,
            vec![
                ("2020s", 5, true),
                ("2021", 3, false),
                ("2020", 2, false),
                ("2010s", 4, true),
                ("2019", 4, false),
                ("Unknown", 1, false),
            ]
        );
        assert_eq!(
            entries[0].scope,
            AlbumScope::Years(YearRange::Span {
                first: 2020,
                last: 2029
            })
        );
    }
}
