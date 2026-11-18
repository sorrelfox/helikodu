// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The album page: cover, title, artist, badges and the play buttons; the
//! tracks disc by disc; the details column with the media kind.
//!
//! An audiobook uses the same page: "Play" continues where listening stopped,
//! "Start again" goes back to the start, a meter shows how much has been heard,
//! and the track table lists the chapters with where each starts.

mod book;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{CompositeTemplate, gdk, gio, glib};

use crate::backend::{
    AlbumDetail, AlbumKey, Backend, MediaKind, PlayerRequest, TrackFacts, TrackKey, TrackRow,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::playback::{Change, Playback, PlayingKeys};
use crate::views::actions::{self, Target};
use crate::views::drag::DraggedTracks;
use crate::widgets::side_column::SideColumn;
use crate::widgets::{CoverArt, PlayingIndicator, RatingStars, TitleLine, label};
use crate::window::MainWindow;

/// The details column's default width and its limits.
const DETAILS_WIDTH: (i32, (i32, i32)) = (260, (220, 520));
/// How wide dragging the details column leaves the track table at least, so
/// its columns stay readable.
const TRACKS_MIN_WIDTH: i32 = 440;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/github/sorrelfox/Helikodu/ui/album.ui")]
    pub(crate) struct AlbumPage {
        #[template_child]
        pub(super) cover: TemplateChild<CoverArt>,
        #[template_child]
        pub(super) section: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) play_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) restart: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) progress: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) heard: TemplateChild<gtk::LevelBar>,
        #[template_child]
        pub(super) heard_text: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) title_column: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) plays_column: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) rating_column: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) length_column: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) kind: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub(super) kind_text: TemplateChild<gtk::Label>,
        /// Set while the page sets the Kind dropdown itself.
        pub(super) kind_quiet: Cell<bool>,
        /// Where an audiobook stands, while one is shown.
        pub(super) book: RefCell<Option<book::Progress>>,
        #[template_child]
        pub(super) title: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) artist: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) meta: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) badges: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) play: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) shuffle: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) tracks: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub(super) details: TemplateChild<gtk::Grid>,
        #[template_child]
        pub(super) track_section: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) track_heading: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) track_details: TemplateChild<gtk::Grid>,
        /// Counts the selections, so that facts loaded for an earlier one are
        /// dropped.
        pub(super) track_generation: Cell<u64>,
        #[template_child]
        pub(super) body: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) details_box: TemplateChild<gtk::Box>,
        /// The details column, whose width the user drags.
        pub(super) details_column: RefCell<Option<SideColumn>>,
        pub(super) backend: RefCell<Option<Backend>>,
        pub(super) album: Cell<Option<AlbumKey>>,
        pub(super) detail: RefCell<Option<AlbumDetail>>,
        pub(super) playing: Cell<PlayingKeys>,
        /// A track to select once the album is shown.
        pub(super) select: Cell<Option<TrackKey>>,
        pub(super) generation: Cell<u64>,
        pub(super) covers: RefCell<Option<Rc<CoverCache>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumPage {
        const NAME: &'static str = "HkAlbumPage";
        type Type = super::AlbumPage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            CoverArt::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.cover.set_shape(184, 8.0);
            self.cover.add_css_class("hero");
            let obj = self.obj();
            // The details column keeps the width chosen for it; its text wraps.
            let details_box = self.details_box.get();
            self.body.remove(&details_box);
            let column = SideColumn::new(&details_box, DETAILS_WIDTH.0, DETAILS_WIDTH.1);
            column.set_beside_min(TRACKS_MIN_WIDTH);
            self.body.append(&column);
            self.details_column.replace(Some(column));
            for (button, shuffle) in [(self.play.get(), false), (self.shuffle.get(), true)] {
                let weak = obj.downgrade();
                button.connect_clicked(move |_| {
                    if let Some(page) = weak.upgrade() {
                        // An audiobook continues where listening stopped.
                        let start = page
                            .imp()
                            .book
                            .borrow()
                            .as_ref()
                            .map_or(0, |b| b.continue_at);
                        page.play_from(start, shuffle);
                    }
                });
            }
            let names = book::kind_names();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            self.kind.set_model(Some(&gtk::StringList::new(&names)));
            let weak = obj.downgrade();
            self.kind.connect_selected_notify(move |dropdown| {
                if let Some(page) = weak.upgrade()
                    && !page.imp().kind_quiet.get()
                    && let Some(kind) = usize::try_from(dropdown.selected())
                        .ok()
                        .and_then(|i| book::KINDS.get(i))
                {
                    page.set_kind(*kind);
                }
            });
            let weak = obj.downgrade();
            self.tracks.connect_selected_rows_changed(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.show_selected();
                }
            });
            let weak = obj.downgrade();
            self.tracks.connect_row_activated(move |_, row| {
                if let Some(page) = weak.upgrade()
                    && let Ok(index) = row.widget_name().parse::<usize>()
                {
                    page.play_from(index, false);
                }
            });
        }
    }

    impl WidgetImpl for AlbumPage {}
    impl BoxImpl for AlbumPage {}
}

glib::wrapper! {
    /// The album page.
    pub(crate) struct AlbumPage(ObjectSubclass<imp::AlbumPage>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for AlbumPage {
    fn default() -> Self {
        glib::Object::new()
    }
}

fn track_row(index: usize, track: &TrackRow, playing: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    // The row's index into the album's tracks, for activation.
    row.set_widget_name(&index.to_string());
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    if playing {
        row.add_css_class("playing");
        row.add_css_class("hk-playing");
        let indicator = PlayingIndicator::default();
        indicator.set_size_request(24, -1);
        indicator.set_halign(gtk::Align::End);
        line.append(&indicator);
    } else {
        let number = label(
            &track.number.map(|n| n.to_string()).unwrap_or_default(),
            &["hk-t3", "hk-numeric"],
        );
        number.set_xalign(1.0);
        number.set_size_request(24, -1);
        line.append(&number);
    }
    let title = TitleLine::new(&track.title, if playing { &["hk-playing"] } else { &[] });
    title.set_copies(track.key, track.copies);
    line.append(&title);
    let plays = label(&track.plays.to_string(), &["hk-t3", "hk-numeric"]);
    plays.set_xalign(1.0);
    plays.set_size_request(56, -1);
    line.append(&plays);
    let rating = RatingStars::default();
    rating.set_rating(track.rating);
    rating.set_size_request(74, -1);
    rating.set_halign(gtk::Align::Start);
    let rating_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rating_box.set_size_request(74, -1);
    rating_box.append(&rating);
    line.append(&rating_box);
    let length = label(
        &track.duration.map(format::duration).unwrap_or_default(),
        &["hk-t2", "hk-numeric"],
    );
    length.set_xalign(1.0);
    length.set_size_request(48, -1);
    line.append(&length);
    row.set_child(Some(&line));
    let name = match track.number {
        Some(n) => fill(&tr("Track {}: {}"), &[&n.to_string(), &track.title]),
        None => track.title.clone(),
    };
    let name = if playing {
        fill(&tr("{}, playing"), &[&name])
    } else {
        name
    };
    row.update_property(&[gtk::accessible::Property::Label(&name)]);
    row
}

/// The rating stars inside `widget`.
fn find_stars(widget: &gtk::Widget) -> Option<RatingStars> {
    let mut child = widget.first_child();
    while let Some(w) = child {
        if let Some(stars) = w.downcast_ref::<RatingStars>() {
            return Some(stars.clone());
        }
        if let Some(stars) = find_stars(&w) {
            return Some(stars);
        }
        child = w.next_sibling();
    }
    None
}

impl AlbumPage {
    /// Connects the page to `backend` and follows `playback`.
    pub(crate) fn bind(&self, backend: &Backend, playback: &Rc<Playback>, covers: &Rc<CoverCache>) {
        self.imp().backend.replace(Some(backend.clone()));
        self.imp().covers.replace(Some(covers.clone()));
        self.install_actions();
        self.restore_details_width(backend);
        let weak = self.downgrade();
        playback.connect(move |view, change| {
            if let Some(page) = weak.upgrade()
                && change == Change::Playing
                && page.imp().playing.replace(view.playing) != view.playing
            {
                page.show_tracks();
            }
        });
    }

    /// Gives the details column the width chosen in the last run, and keeps
    /// the width the user drags to for the next. One width serves every album
    /// and audiobook.
    fn restore_details_width(&self, backend: &Backend) {
        let Some(column) = self.imp().details_column.borrow().clone() else {
            return;
        };
        let store = backend.ui_state();
        let width = store.load().clamped().panes.details_width;
        column.set_chosen_width(i32::try_from(width).unwrap_or(DETAILS_WIDTH.0));
        column.connect_changed(move |width| {
            let mut state = store.load();
            state.panes.details_width = u32::try_from(width).unwrap_or(260);
            if let Err(error) = store.save(&state) {
                log::warn!("cannot save the window state: {error}");
            }
        });
    }

    /// The page's actions: Add to queue, More, Edit tags, Open folder; the
    /// track rows' context menu and dragging.
    fn install_actions(&self) {
        let group = gio::SimpleActionGroup::new();
        for name in ["enqueue", "more", "edit-tags", "open-folder", "restart"] {
            let action = gio::SimpleAction::new(name, None);
            let weak = self.downgrade();
            action.connect_activate(move |action, _| {
                let Some(page) = weak.upgrade() else { return };
                let (Some(window), Some(album), Some(backend)) = (
                    page.root().and_downcast::<MainWindow>(),
                    page.imp().album.get(),
                    page.imp().backend.borrow().clone(),
                ) else {
                    return;
                };
                let tracks: Vec<TrackKey> = page
                    .imp()
                    .detail
                    .borrow()
                    .as_ref()
                    .map(|d| d.tracks().map(|t| t.key).collect())
                    .unwrap_or_default();
                match action.name().as_str() {
                    "enqueue" => actions::play(&window, backend, Target::Album(album), Some(false)),
                    "more" => {
                        actions::popup(&page.imp().play.get(), Target::Album(album), None);
                    }
                    "edit-tags" => window.open_tag_editor(tracks),
                    "restart" => page.start_again(&backend, album, tracks),
                    _ => {
                        // The selected track's folder, or else the album's.
                        let path = page.selected_track().and_then(|t| t.path).or_else(|| {
                            page.imp()
                                .detail
                                .borrow()
                                .as_ref()
                                .and_then(|d| d.tracks().find_map(|t| t.path.clone()))
                        });
                        actions::open_folder(&window, path);
                    }
                }
            });
            group.add_action(&action);
        }
        self.insert_action_group("album-page", Some(&group));
        let tracks = self.imp().tracks.get();
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = self.downgrade();
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(page) = weak.upgrade() else { return };
            if let Some(target) = page.row_target(y) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                actions::popup(&page.imp().tracks.get(), target, Some((x, y)));
            }
        });
        tracks.add_controller(click);
        let weak = self.downgrade();
        DraggedTracks::source(&tracks, move |_, y| {
            let page = weak.upgrade()?;
            let Some(Target::Tracks { rows, .. }) = page.row_target(y) else {
                return None;
            };
            let row = rows.first()?;
            Some((
                DraggedTracks {
                    keys: vec![row.key],
                    ..DraggedTracks::default()
                },
                row.title.clone(),
            ))
        });
    }

    /// The track under `y` of the track list, as a menu target.
    fn row_target(&self, y: f64) -> Option<Target> {
        let row = self.imp().tracks.row_at_y(y as i32)?;
        let index = row.widget_name().parse::<usize>().ok()?;
        let detail = self.imp().detail.borrow();
        let track = detail.as_ref()?.tracks().nth(index)?.clone();
        Some(Target::tracks(vec![Rc::new(track)]))
    }

    /// The album shown.
    pub(crate) fn album(&self) -> Option<AlbumKey> {
        self.imp().album.get()
    }

    /// Loads and shows `key`.
    pub(crate) fn show_album(&self, key: AlbumKey) {
        let imp = self.imp();
        let Some(backend) = imp.backend.borrow().clone() else {
            return;
        };
        imp.album.set(Some(key));
        let generation = imp.generation.get() + 1;
        imp.generation.set(generation);
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let detail = backend.query(move |library| library.album(key)).await;
            let Some(page) = weak.upgrade() else { return };
            if page.imp().generation.get() == generation {
                page.show_detail(detail.ok().flatten());
            }
        });
    }

    fn show_detail(&self, detail: Option<AlbumDetail>) {
        let imp = self.imp();
        let Some(detail) = detail else {
            imp.title.set_text(&tr("Album not found"));
            imp.detail.replace(None);
            self.show_tracks();
            return;
        };
        let row = &detail.row;
        match imp.covers.borrow().as_ref() {
            Some(covers) => covers.show(&imp.cover, row.cover, row.cover_seed, 184),
            None => imp.cover.show(row.cover_seed, None),
        }
        imp.title.set_text(&row.title);
        imp.title.set_tooltip_text(Some(&row.title));
        imp.artist.set_text(&row.artist);
        let is_book = detail.kind == MediaKind::Audiobook;
        let progress = is_book.then(|| book::Progress::of(&detail));
        self.show_kind(&detail, progress.as_ref());
        imp.book.replace(progress);
        let count = detail.tracks().count();
        let tracks = if is_book {
            fill(
                &ntr("{} chapter", "{} chapters", count),
                &[&crate::format::count(count)],
            )
        } else {
            fill(
                &ntr("{} track", "{} tracks", count),
                &[&crate::format::count(count)],
            )
        };
        let mut parts = Vec::new();
        if is_book && let Some(reader) = &detail.reader {
            parts.push(fill(&tr("read by {}"), &[reader]));
        }
        if let Some(year) = row.year {
            parts.push(year.to_string());
        }
        parts.push(tracks);
        parts.push(format::minutes(detail.duration()));
        imp.meta.set_text(&format!("· {}", parts.join(" · ")));
        while let Some(child) = imp.badges.first_child() {
            imp.badges.remove(&child);
        }
        let container = if is_book {
            book::container_badge(&detail)
        } else {
            None
        };
        for badge in container
            .into_iter()
            .chain(format::badges(&row.format, detail.album_gain_db))
        {
            let badge = gtk::Label::new(Some(&badge));
            badge.add_css_class("hk-badge");
            imp.badges.append(&badge);
        }
        let mut entries: Vec<(String, String, bool)> = Vec::new();
        if is_book {
            entries.push((tr("Author"), row.artist.clone(), false));
            if let Some(reader) = &detail.reader {
                entries.push((tr("Read by"), reader.clone(), false));
            }
        } else if let Some(genre) = &detail.genre {
            entries.push((tr("Genre"), genre.clone(), false));
        }
        if let Some(year) = row.year {
            entries.push((tr("Released"), year.to_string(), false));
        }
        if let Some(added) = &detail.added {
            entries.push((tr("Added"), added.clone(), false));
        }
        if let Some(size) = detail.size {
            entries.push((tr("Size"), format::size(size), false));
        }
        // An audiobook in one file names the file, anything else its folder.
        let first = detail.tracks().find_map(|t| t.path.clone());
        let file = first.filter(|file| {
            is_book
                && detail
                    .tracks()
                    .all(|t| t.path.as_deref().is_none_or(|p| p == file.as_path()))
        });
        match (&file, &detail.folder) {
            (Some(file), _) => {
                entries.push((tr("File"), file.display().to_string(), true));
            }
            (_, Some(folder)) => entries.push((tr("Folder"), folder.clone(), true)),
            _ => {}
        }
        fill_grid(&imp.details, entries);
        imp.detail.replace(Some(detail));
        self.show_tracks();
        self.apply_select();
    }

    /// Selects `track`, and scrolls to it, once its album is shown.
    pub(crate) fn select_track(&self, track: TrackKey) {
        self.imp().select.set(Some(track));
        self.apply_select();
    }

    fn apply_select(&self) {
        let imp = self.imp();
        let Some(track) = imp.select.get() else {
            return;
        };
        let index = {
            let detail = imp.detail.borrow();
            let Some(detail) = detail
                .as_ref()
                .filter(|d| Some(d.row.key) == imp.album.get())
            else {
                return;
            };
            detail.tracks().position(|t| t.key == track)
        };
        imp.select.set(None);
        let Some(index) = index.map(|i| i.to_string()) else {
            return;
        };
        let mut child = imp.tracks.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && row.widget_name() == index
            {
                imp.tracks.select_row(Some(row));
                row.grab_focus();
                return;
            }
        }
    }

    /// The title of the selected track.
    pub(crate) fn selected_title(&self) -> Option<String> {
        self.selected_track().map(|t| t.title)
    }

    /// The selected track.
    fn selected_track(&self) -> Option<TrackRow> {
        let imp = self.imp();
        let index = imp
            .tracks
            .selected_row()?
            .widget_name()
            .parse::<usize>()
            .ok()?;
        let detail = imp.detail.borrow();
        detail.as_ref()?.tracks().nth(index).cloned()
    }

    /// Shows the selected track below the album's details: what its row
    /// knows at once, the rest when the library has answered.
    fn show_selected(&self) {
        let imp = self.imp();
        let generation = imp.track_generation.get() + 1;
        imp.track_generation.set(generation);
        let Some(track) = self.selected_track() else {
            imp.track_section.set_visible(false);
            return;
        };
        self.show_track(&track, None);
        let Some(backend) = imp.backend.borrow().clone() else {
            return;
        };
        let key = track.key;
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let facts = backend.query(move |library| library.track_facts(key)).await;
            let Some(page) = weak.upgrade() else { return };
            if page.imp().track_generation.get() == generation
                && let Ok(Some(facts)) = facts
            {
                page.show_track(&track, Some(&facts));
            }
        });
    }

    fn show_track(&self, track: &TrackRow, facts: Option<&TrackFacts>) {
        let imp = self.imp();
        imp.track_heading.set_text(&track.title);
        let several_discs = imp
            .detail
            .borrow()
            .as_ref()
            .is_some_and(|d| d.discs.len() > 1);
        let album_artist = imp
            .detail
            .borrow()
            .as_ref()
            .map(|d| d.row.artist.clone())
            .unwrap_or_default();
        fill_grid(
            &imp.track_details,
            track_entries(track, facts, &album_artist, several_discs),
        );
        imp.track_section.set_visible(true);
    }

    /// The text of the selected track's section, line by line: "Name: value".
    pub(crate) fn track_section_texts(&self) -> Vec<String> {
        let imp = self.imp();
        if !imp.track_section.is_visible() {
            return Vec::new();
        }
        let mut texts = vec![imp.track_heading.text().to_string()];
        let mut child = imp.track_details.first_child();
        while let Some(name) = child {
            let value = name.next_sibling();
            child = value.as_ref().and_then(gtk::Widget::next_sibling);
            if let (Some(name), Some(value)) = (
                name.downcast_ref::<gtk::Label>(),
                value.and_downcast::<gtk::Label>(),
            ) {
                texts.push(format!("{}: {}", name.text(), value.text()));
            }
        }
        texts
    }

    fn show_tracks(&self) {
        let imp = self.imp();
        crate::widgets::remove_rows(&imp.tracks);
        let detail = imp.detail.borrow();
        let Some(detail) = detail.as_ref() else {
            return;
        };
        let playing = imp.playing.get();
        if let Some(progress) = imp.book.borrow().as_ref() {
            for (index, track) in detail.tracks().enumerate() {
                imp.tracks.append(&book::chapter_row(
                    index,
                    track,
                    progress.starts.get(index).copied().unwrap_or_default(),
                    progress.heard.get(index).copied().unwrap_or(false),
                    playing.contains(track.key),
                ));
            }
            return;
        }
        let several = detail.discs.len() > 1;
        let mut index = 0;
        for disc in &detail.discs {
            if several {
                let text = match (&disc.subtitle, disc.number) {
                    (Some(subtitle), _) => subtitle.clone(),
                    (None, Some(n)) => fill(&tr("Disc {}"), &[&n.to_string()]),
                    (None, None) => tr("Disc"),
                };
                let row = gtk::ListBoxRow::new();
                row.add_css_class("disc");
                row.set_activatable(false);
                row.set_selectable(false);
                row.set_focusable(false);
                row.set_child(Some(&label(&text, &["hk-section-label"])));
                imp.tracks.append(&row);
            }
            for track in &disc.tracks {
                imp.tracks
                    .append(&track_row(index, track, playing.contains(track.key)));
                index += 1;
            }
        }
    }

    /// Shows a changed rating in the rows of `keys`, in place.
    pub(crate) fn ratings_changed(&self, keys: &[TrackKey], stars: Option<u8>) {
        let imp = self.imp();
        let changed: Vec<usize> = match imp.detail.borrow_mut().as_mut() {
            Some(detail) => detail
                .discs
                .iter_mut()
                .flat_map(|d| d.tracks.iter_mut())
                .enumerate()
                .filter(|(_, t)| keys.contains(&t.key))
                .map(|(i, t)| {
                    t.rating = stars;
                    i
                })
                .collect(),
            None => return,
        };
        let mut index = 0;
        while let Some(row) = imp.tracks.row_at_index(index) {
            index += 1;
            if row
                .widget_name()
                .parse::<usize>()
                .is_ok_and(|i| changed.contains(&i))
                && let Some(rating) = find_stars(row.upcast_ref())
            {
                rating.set_rating(stars);
            }
        }
    }

    fn play_from(&self, start: usize, shuffle: bool) {
        let imp = self.imp();
        if let (Some(backend), Some(album)) = (imp.backend.borrow().as_ref(), imp.album.get()) {
            backend.send(PlayerRequest::PlayAlbum {
                album,
                start,
                shuffle,
            });
        }
    }

    /// Shows the parts of the page that depend on the media kind: the section
    /// label, the play buttons, the meter, the table's columns and the Kind
    /// dropdown.
    fn show_kind(&self, detail: &AlbumDetail, progress: Option<&book::Progress>) {
        let imp = self.imp();
        let is_book = progress.is_some();
        imp.section.set_text(&if is_book {
            tr("Audiobook")
        } else {
            tr("Album")
        });
        imp.shuffle.set_visible(!is_book);
        imp.restart.set_visible(is_book);
        imp.progress.set_visible(is_book);
        if let Some(progress) = progress {
            imp.heard.set_value(progress.fraction());
            imp.heard_text.set_text(&progress.text());
        }
        imp.title_column
            .set_text(&if is_book { tr("Chapter") } else { tr("Title") });
        imp.plays_column.set_text(&if is_book {
            tr("Starts at")
        } else {
            tr("Plays")
        });
        imp.plays_column
            .set_size_request(if is_book { 70 } else { 56 }, -1);
        imp.rating_column.set_visible(!is_book);
        imp.length_column
            .set_size_request(if is_book { 56 } else { 48 }, -1);
        imp.kind_quiet.set(true);
        let position = book::KINDS
            .iter()
            .position(|k| *k == detail.kind)
            .unwrap_or(1);
        imp.kind.set_selected(u32::try_from(position).unwrap_or(1));
        imp.kind_quiet.set(false);
        imp.kind_text.set_text(&book::kind_text(detail.kind));
    }

    /// Gives the album shown the media kind `kind`, then shows it again.
    fn set_kind(&self, kind: MediaKind) {
        let imp = self.imp();
        let (Some(backend), Some(album)) = (imp.backend.borrow().clone(), imp.album.get()) else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let done = backend
                .query(move |library| library.set_album_kind(album, kind))
                .await;
            let Some(page) = weak.upgrade() else { return };
            match done {
                Ok(()) => page.show_album(album),
                Err(error) => {
                    if let Some(window) = page.root().and_downcast::<MainWindow>() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    /// Forgets where listening to the audiobook stopped and plays it from the
    /// start.
    fn start_again(&self, backend: &Backend, album: AlbumKey, tracks: Vec<TrackKey>) {
        let weak = self.downgrade();
        let backend = backend.clone();
        glib::spawn_future_local(async move {
            let _ = backend
                .query(move |library| library.forget_resume(&tracks))
                .await;
            backend.send(PlayerRequest::PlayAlbum {
                album,
                start: 0,
                shuffle: false,
            });
            if let Some(page) = weak.upgrade() {
                page.show_album(album);
            }
        });
    }

    /// The play button, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.imp().play.get().upcast()
    }

    /// The ratings the track rows show, in order (for tests).
    pub(crate) fn shown_ratings(&self) -> Vec<Option<u8>> {
        let tracks = self.imp().tracks.get();
        let mut ratings = Vec::new();
        let mut index = 0;
        while let Some(row) = tracks.row_at_index(index) {
            index += 1;
            if let Some(stars) = find_stars(row.upcast_ref()) {
                ratings.push(stars.rating());
            }
        }
        ratings
    }

    /// Opens the context menu of the first track row, as a right click
    /// would (for tests and screenshots).
    pub(crate) fn demo_menu(&self) -> Option<gtk::PopoverMenu> {
        let tracks = self.imp().tracks.get();
        // The first track, past a disc's heading.
        let row = (0..)
            .map_while(|i| tracks.row_at_index(i))
            .find(|r| r.widget_name().parse::<usize>().is_ok())?;
        let bounds = row.compute_bounds(&tracks)?;
        let y = f64::from(bounds.y() + bounds.height() / 2.0);
        let target = self.row_target(y)?;
        actions::popup(&tracks, target, Some((240.0, y)))
    }
}

/// Fills `grid` with a name and a value on each line; a value marked as a path
/// is set in the monospace face and can be selected.
fn fill_grid(grid: &gtk::Grid, entries: Vec<(String, String, bool)>) {
    while let Some(child) = grid.first_child() {
        grid.remove(&child);
    }
    for (i, (name, value, mono)) in entries.into_iter().enumerate() {
        let name = label(&name, &["hk-t3"]);
        name.set_size_request(72, -1);
        name.set_valign(gtk::Align::Start);
        let value_label = gtk::Label::new(Some(&value));
        value_label.set_xalign(0.0);
        value_label.set_wrap(true);
        value_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        value_label.set_hexpand(true);
        value_label.set_selectable(mono);
        value_label.add_css_class(if mono { "hk-mono-meta" } else { "hk-medium" });
        let row = i32::try_from(i).unwrap_or(0);
        grid.attach(&name, 0, row, 1, 1);
        grid.attach(&value_label, 1, row, 1, 1);
    }
}

/// The lines of the selected track's section: its own facts, each only when
/// known; the artists only when they are not the album's.
fn track_entries(
    track: &TrackRow,
    facts: Option<&TrackFacts>,
    album_artist: &str,
    several_discs: bool,
) -> Vec<(String, String, bool)> {
    let mut entries: Vec<(String, String, bool)> = Vec::new();
    let mut add = |name: String, value: String| entries.push((name, value, false));
    if !track.artist.is_empty() && track.artist != album_artist {
        add(tr("Artist"), track.artist.clone());
    }
    if let Some(composers) = facts.map(|f| &f.composers).filter(|c| !c.is_empty()) {
        add(tr("Composer"), composers.clone());
    }
    match (track.disc.filter(|_| several_discs), track.number) {
        (Some(disc), Some(number)) => add(
            tr("Track"),
            fill(
                &tr("Disc {}, track {}"),
                &[&disc.to_string(), &number.to_string()],
            ),
        ),
        (_, Some(number)) => add(tr("Track"), number.to_string()),
        _ => {}
    }
    if let Some(duration) = track.duration {
        add(tr("Length"), format::duration(duration));
    }
    let mut format = format::badges(&track.format, None);
    if let Some(kbps) = track.format.bitrate_kbps {
        format.push(fill(&tr("{} kbit/s"), &[&kbps.to_string()]));
    }
    if !format.is_empty() {
        add(tr("Format"), format.join(" \u{b7} "));
    }
    if let Some(gain) = facts.and_then(|f| f.track_gain_db) {
        let sign = if gain < 0.0 { "\u{2212}" } else { "+" };
        add(
            tr("Gain"),
            fill(&tr("{} dB"), &[&format!("{sign}{:.1}", gain.abs())]),
        );
    }
    add(tr("Plays"), format::count(track.plays));
    if let Some(last) = facts.and_then(|f| f.last_played.clone()) {
        add(tr("Last played"), last);
    }
    if let Some(stars) = track.rating.filter(|s| *s > 0) {
        add(
            tr("Rating"),
            fill(
                &ntr("{} star", "{} stars", usize::from(stars)),
                &[&stars.to_string()],
            ),
        );
    }
    if let Some(added) = facts.and_then(|f| f.added.clone()) {
        add(tr("Added"), added);
    }
    if facts.is_some_and(|f| f.has_lyrics) {
        add(tr("Lyrics"), tr("Yes"));
    }
    if track.copies > 0 {
        add(
            tr("Copies"),
            crate::widgets::copies_badge::describe(track.copies),
        );
    }
    if let Some(size) = facts.and_then(|f| f.file_size) {
        add(tr("Size"), format::size(size));
    }
    if let Some(path) = &track.path {
        entries.push((tr("File"), path.display().to_string(), true));
    }
    entries
}
