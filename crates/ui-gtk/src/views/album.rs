// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The album page: cover, title, artist, badges and the play buttons; the
//! tracks disc by disc; the details column.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{CompositeTemplate, gdk, gio, glib};

use crate::backend::{AlbumDetail, AlbumKey, Backend, PlayerRequest, TrackKey, TrackRow};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::playback::{Change, Playback};
use crate::views::actions::{self, Target};
use crate::views::drag::DraggedTracks;
use crate::widgets::{CoverArt, PlayingIndicator, RatingStars, label};
use crate::window::MainWindow;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/github/sorrelfox/Helikodu/ui/album.ui")]
    pub(crate) struct AlbumPage {
        #[template_child]
        pub(super) cover: TemplateChild<CoverArt>,
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
        pub(super) backend: RefCell<Option<Backend>>,
        pub(super) album: Cell<Option<AlbumKey>>,
        pub(super) detail: RefCell<Option<AlbumDetail>>,
        pub(super) playing: Cell<Option<TrackKey>>,
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
            for (button, shuffle) in [(self.play.get(), false), (self.shuffle.get(), true)] {
                let weak = obj.downgrade();
                button.connect_clicked(move |_| {
                    if let Some(page) = weak.upgrade() {
                        page.play_from(0, shuffle);
                    }
                });
            }
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
    let title = label(&track.title, if playing { &["hk-playing"] } else { &[] });
    title.set_hexpand(true);
    title.set_max_width_chars(1);
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

impl AlbumPage {
    /// Connects the page to `backend` and follows `playback`.
    pub(crate) fn bind(&self, backend: &Backend, playback: &Rc<Playback>, covers: &Rc<CoverCache>) {
        self.imp().backend.replace(Some(backend.clone()));
        self.imp().covers.replace(Some(covers.clone()));
        self.install_actions();
        let weak = self.downgrade();
        playback.connect(move |view, change| {
            if let Some(page) = weak.upgrade()
                && change == Change::Track
            {
                let key = view.now.as_ref().map(|n| n.track.key);
                if page.imp().playing.replace(key) != key {
                    page.show_tracks();
                }
            }
        });
    }

    /// The page's actions: Add to queue, More, Edit tags, Open folder; the
    /// track rows' context menu and dragging.
    fn install_actions(&self) {
        let group = gio::SimpleActionGroup::new();
        for name in ["enqueue", "more", "edit-tags", "open-folder"] {
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
                    _ => {
                        let path = page
                            .imp()
                            .detail
                            .borrow()
                            .as_ref()
                            .and_then(|d| d.tracks().find_map(|t| t.path.clone()));
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
        let count = detail.tracks().count();
        let tracks = fill(
            &ntr("{} track", "{} tracks", count),
            &[&crate::format::count(count)],
        );
        let mut parts = Vec::new();
        if let Some(year) = row.year {
            parts.push(year.to_string());
        }
        parts.push(tracks);
        parts.push(format::minutes(detail.duration()));
        imp.meta.set_text(&format!("· {}", parts.join(" · ")));
        while let Some(child) = imp.badges.first_child() {
            imp.badges.remove(&child);
        }
        for badge in format::badges(&row.format, detail.album_gain_db) {
            let badge = gtk::Label::new(Some(&badge));
            badge.add_css_class("hk-badge");
            imp.badges.append(&badge);
        }
        while let Some(child) = imp.details.first_child() {
            imp.details.remove(&child);
        }
        let mut entries: Vec<(String, String, bool)> = Vec::new();
        if let Some(genre) = &detail.genre {
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
        if let Some(folder) = &detail.folder {
            entries.push((tr("Folder"), folder.clone(), true));
        }
        for (i, (name, value, mono)) in entries.into_iter().enumerate() {
            let name = label(&name, &["hk-t3"]);
            name.set_size_request(72, -1);
            name.set_valign(gtk::Align::Start);
            let value_label = gtk::Label::new(Some(&value));
            value_label.set_xalign(0.0);
            value_label.set_wrap(true);
            value_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            value_label.set_width_chars(20);
            value_label.set_max_width_chars(20);
            value_label.set_selectable(mono);
            value_label.add_css_class(if mono { "hk-mono-meta" } else { "hk-medium" });
            let row = i32::try_from(i).unwrap_or(0);
            imp.details.attach(&name, 0, row, 1, 1);
            imp.details.attach(&value_label, 1, row, 1, 1);
        }
        imp.detail.replace(Some(detail));
        self.show_tracks();
    }

    fn show_tracks(&self) {
        let imp = self.imp();
        crate::widgets::remove_rows(&imp.tracks);
        let detail = imp.detail.borrow();
        let Some(detail) = detail.as_ref() else {
            return;
        };
        let playing = imp.playing.get();
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
                row.set_focusable(false);
                row.set_child(Some(&label(&text, &["hk-section-label"])));
                imp.tracks.append(&row);
            }
            for track in &disc.tracks {
                imp.tracks
                    .append(&track_row(index, track, Some(track.key) == playing));
                index += 1;
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

    /// The play button, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.imp().play.get().upcast()
    }

    /// Opens the context menu of the first track row, as a right click
    /// would (for tests and screenshots).
    pub(crate) fn demo_menu(&self) -> Option<gtk::PopoverMenu> {
        let tracks = self.imp().tracks.get();
        let row = tracks.row_at_index(0)?;
        let bounds = row.compute_bounds(&tracks)?;
        let y = f64::from(bounds.y() + bounds.height() / 2.0);
        let target = self.row_target(y)?;
        actions::popup(&tracks, target, Some((240.0, y)))
    }
}
