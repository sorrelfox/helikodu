// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tracks that sound alike: the tracks whose sound comes closest to a track,
//! an album or an artist, as the track analysis noted it. 50 at a time, never
//! one below a score of 0.5, with "How alike" as a bar and a word.
//!
//! Before the analysis has reached the seed (or while too few tracks are
//! analysed, or with the analysis turned off) the page says so with the
//! analysis progress of Settings › Library, and fills in by itself once the
//! analysis gets there: it asks again when the analysis reports progress.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use super::list_table::{Column, ColumnKind, ListTable, Value, follow_playing};
use crate::backend::generators::too_few_analysed;
use crate::backend::services::{AnalysisProgress, AnalysisState};
use crate::backend::{
    Backend, NotYet, PlayerRequest, RadioSeed, SimilarFound, SimilarRow, SimilarSeed, TrackRow,
    Update,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::playback::Playback;
use crate::views::actions::Target;
use crate::views::artists::button;
use crate::views::table::PlayingRows;
use crate::widgets::avatar::Avatar;
use crate::widgets::{CoverArt, label};

/// How many tracks the list grows by.
pub(crate) const STEP: usize = 50;
/// Fewer tracks than this make the list "few".
const FEW: usize = 5;

/// The word of a score: "Very close" from 0.85, "Close" from 0.70,
/// "Somewhat alike" below.
pub(crate) fn alike_word(score: f32) -> String {
    if score >= 0.85 {
        tr("Very close")
    } else if score >= 0.70 {
        tr("Close")
    } else {
        tr("Somewhat alike")
    }
}

/// The share of the bar: from the lowest score listed (0.5) to 1.
fn alike_share(score: f32) -> f64 {
    f64::from(((score - 0.5) / 0.5).clamp(0.0, 1.0))
}

/// The score as a number of 100, for the accessible name.
fn of_hundred(score: f32) -> u32 {
    (score.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// What the not-yet page says: its headline and paragraph.
pub(crate) fn not_yet_words(reason: NotYet, off: bool, several: bool) -> (String, String) {
    match (reason, off) {
        (NotYet::Unreadable, _) => (
            tr("This track could not be analysed"),
            tr("Its file could not be read. Helikodu tries again when the file changes."),
        ),
        (NotYet::NotAnalysed, true) => (
            tr("Track analysis is turned off"),
            tr(
                "Helikodu notes how tracks sound as part of the track analysis. Turn it on under Settings › Library, and the list appears here once this track is analysed.",
            ),
        ),
        (NotYet::TooFew(analysed), _) => (
            tr("Too few tracks are analysed so far"),
            fill(
                &ntr(
                    "Helikodu can compare this track with the {} track it has analysed, which is too few for a useful list. The list appears here once more are done.",
                    "Helikodu can compare this track with the {} tracks it has analysed, which is too few for a useful list. The list appears here once more are done.",
                    analysed,
                ),
                &[&format::count(analysed)],
            ),
        ),
        (NotYet::NotAnalysed, false) if several => (
            tr("These tracks have not been analysed yet"),
            tr(
                "Helikodu notes how each track sounds while it analyses the library in the background. The list appears here by itself once the analysis has reached these tracks.",
            ),
        ),
        (NotYet::NotAnalysed, false) => (
            tr("This track has not been analysed yet"),
            tr(
                "Helikodu notes how each track sounds while it analyses the library in the background. The list appears here by itself once the analysis has reached this track.",
            ),
        ),
    }
}

/// The page of tracks that sound alike.
pub(crate) struct SimilarView {
    pub(crate) root: gtk::Box,
    cover: CoverArt,
    avatar: Avatar,
    title: gtk::Label,
    subtitle: gtk::Label,
    buttons: Vec<gtk::Button>,
    body: gtk::Stack,
    table: Rc<ListTable>,
    few: gtk::Label,
    status_left: gtk::Label,
    more: gtk::Button,
    headline: gtk::Label,
    paragraph: gtk::Label,
    progress: gtk::Box,
    progress_count: gtk::Label,
    progress_meter: gtk::LevelBar,
    progress_state: gtk::Label,
    settings_button: gtk::Button,
    backend: Backend,
    covers: Rc<CoverCache>,
    seed: Cell<Option<SimilarSeed>>,
    /// What the seed is, for the subtitle: "Mari Kask · Tidewater Rooms".
    seed_words: RefCell<String>,
    rows: RefCell<Vec<SimilarRow>>,
    limit: Cell<usize>,
    has_more: Cell<bool>,
    not_yet: Cell<Option<NotYet>>,
    /// The analysis counts last shown, to ask again only when they change.
    analysed: Cell<Option<usize>>,
    generation: Cell<u64>,
    _playing: Rc<PlayingRows>,
}

impl std::fmt::Debug for SimilarView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimilarView")
            .field("seed", &self.seed)
            .finish_non_exhaustive()
    }
}

impl SimilarView {
    /// The page over `backend`.
    pub(crate) fn new(
        backend: &Backend,
        covers: &Rc<CoverCache>,
        playback: &Rc<Playback>,
    ) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-similar");
        let hero = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        hero.add_css_class("hk-playlist-hero");
        hero.add_css_class("hk-similar-hero");
        let picture = gtk::Stack::new();
        let cover = CoverArt::new(88, 6.0);
        picture.add_named(&cover, Some("cover"));
        let avatar = Avatar::new(88);
        picture.add_named(&avatar, Some("avatar"));
        picture.set_valign(gtk::Align::Center);
        hero.append(&picture);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        titles.set_valign(gtk::Align::Center);
        titles.append(&label(&tr("Sounds like"), &["hk-section-label"]));
        let title = label("", &["hk-hero-title", "small"]);
        title.set_accessible_role(gtk::AccessibleRole::Heading);
        title.set_max_width_chars(1);
        titles.append(&title);
        let subtitle = label("", &["hk-t3"]);
        subtitle.set_max_width_chars(1);
        titles.append(&subtitle);
        hero.append(&titles);
        let play = button(&tr("Play"), Some("media-playback-start-symbolic"), true);
        let enqueue = button(&tr("Add to queue"), Some("list-add-symbolic"), false);
        let radio = button(&tr("Start radio"), Some("helikodu-radio-symbolic"), false);
        let save = button(&tr("Save as playlist"), None, false);
        let buttons_box = crate::widgets::WrapBox::new(8);
        buttons_box.set_valign(gtk::Align::Center);
        for b in [&play, &enqueue, &radio, &save] {
            buttons_box.append(b);
        }
        hero.append(&buttons_box);
        root.append(&hero);
        picture.set_widget_name("similar-picture");

        let explain = label(
            &tr(
                "Compared by sound alone: how bright each track is, how noisy or tonal, how busy its rhythm and how much its loudness varies. Tags are not used, so the list can cross genres. Take it as a guide.",
            ),
            &["hk-t3", "hk-small", "hk-similar-explain"],
        );
        explain.set_wrap(true);
        explain.set_ellipsize(gtk::pango::EllipsizeMode::None);
        explain.set_single_line_mode(false);

        let playing = Rc::new(PlayingRows::default());
        let table = ListTable::new(&tr("Tracks that sound alike"), false, covers, &playing);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list.append(&explain);
        list.append(&table.header);
        list.append(&table.scroller);
        let few = label(
            &tr("Few tracks in the library sound like this one."),
            &["hk-t2"],
        );
        few.add_css_class("hk-similar-few");
        few.set_visible(false);
        list.append(&few);
        let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        status.add_css_class("hk-statusbar");
        let status_left = label("", &[]);
        status_left.set_hexpand(true);
        status.append(&status_left);
        let more = gtk::Button::with_label(&fill(&tr("Show {} more"), &[&STEP.to_string()]));
        more.add_css_class("hk-link-button");
        status.append(&more);
        list.append(&status);

        // Not yet: centred under a line.
        let waiting = gtk::Box::new(gtk::Orientation::Vertical, 0);
        waiting.add_css_class("hk-similar-not-yet");
        let centre = gtk::Box::new(gtk::Orientation::Vertical, 10);
        centre.set_valign(gtk::Align::Center);
        centre.set_halign(gtk::Align::Center);
        centre.set_vexpand(true);
        // The waveform icon of the design is bundled by another part of the
        // window; until then the desktop's loudspeaker icon stands in.
        let icon = gtk::Image::from_icon_name("audio-volume-high-symbolic");
        icon.set_pixel_size(40);
        icon.add_css_class("hk-t3");
        centre.append(&icon);
        let headline = label("", &["hk-similar-headline"]);
        headline.set_halign(gtk::Align::Center);
        headline.set_widget_name("similar-headline");
        centre.append(&headline);
        let paragraph = gtk::Label::new(None);
        paragraph.add_css_class("hk-t2");
        paragraph.set_wrap(true);
        paragraph.set_justify(gtk::Justification::Center);
        paragraph.set_max_width_chars(60);
        paragraph.set_size_request(-1, -1);
        paragraph.set_widget_name("similar-paragraph");
        centre.append(&paragraph);
        let progress = gtk::Box::new(gtk::Orientation::Vertical, 2);
        progress.set_halign(gtk::Align::Center);
        progress.set_margin_top(8);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let progress_count = label("", &[]);
        head.append(&progress_count);
        let progress_meter = gtk::LevelBar::for_interval(0.0, 1.0);
        progress_meter.add_css_class("hk-meter");
        progress_meter.set_size_request(160, -1);
        progress_meter.set_valign(gtk::Align::Center);
        progress_meter.update_property(&[gtk::accessible::Property::Label(&tr("Tracks analysed"))]);
        head.append(&progress_meter);
        progress.append(&head);
        let progress_state = label("", &["hk-t3", "hk-small"]);
        progress_state.set_halign(gtk::Align::Center);
        progress.append(&progress_state);
        centre.append(&progress);
        let settings_button = gtk::Button::with_label(&tr("Open the analysis settings"));
        settings_button.add_css_class("hk-btn");
        settings_button.set_halign(gtk::Align::Center);
        settings_button.set_margin_top(8);
        centre.append(&settings_button);
        waiting.append(&centre);

        let body = gtk::Stack::new();
        body.set_vexpand(true);
        body.add_named(&list, Some("list"));
        body.add_named(&waiting, Some("not-yet"));
        root.append(&body);

        let view = Rc::new(Self {
            root,
            cover,
            avatar,
            title,
            subtitle,
            buttons: vec![play.clone(), enqueue.clone(), radio.clone(), save.clone()],
            body,
            table,
            few,
            status_left,
            more,
            headline,
            paragraph,
            progress,
            progress_count,
            progress_meter,
            progress_state,
            settings_button,
            backend: backend.clone(),
            covers: covers.clone(),
            seed: Cell::new(None),
            seed_words: RefCell::new(String::new()),
            rows: RefCell::new(Vec::new()),
            limit: Cell::new(STEP),
            has_more: Cell::new(false),
            not_yet: Cell::new(None),
            analysed: Cell::new(None),
            generation: Cell::new(0),
            _playing: playing.clone(),
        });
        follow_playing(&playing, playback);
        view.set_columns();
        view.connect(&play, &enqueue, &radio, &save, playback);
        view
    }

    fn window(&self) -> Option<crate::window::MainWindow> {
        self.root.root().and_downcast::<crate::window::MainWindow>()
    }

    fn set_columns(self: &Rc<Self>) {
        let mut alike = Column::new("alike", tr("How alike"), 176, ColumnKind::Likeness);
        alike.arrow = Some(" ↓");
        // The narrow window keeps the title, the word and the length.
        let columns = vec![
            Column::new("number", "#".into(), 22, ColumnKind::Number).right(),
            Column::new("title", tr("Title"), 0, ColumnKind::TitleWithCover).kept_narrow(),
            Column::new("artist", tr("Artist"), 0, ColumnKind::Text),
            Column::new("album", tr("Album"), 0, ColumnKind::Text),
            alike.kept_narrow(),
            Column::new("length", tr("Length"), 48, ColumnKind::Text)
                .right()
                .kept_narrow(),
        ];
        let weak = Rc::downgrade(self);
        self.table.set_columns(
            columns,
            move |position, row, id| match id {
                "title" => Value::Text(row.title.clone()),
                "artist" => Value::Text(row.artist.clone()),
                "album" => Value::Text(row.album_title.clone()),
                "length" => Value::Text(row.duration.map(format::duration).unwrap_or_default()),
                "alike" => {
                    let score = weak
                        .upgrade()
                        .and_then(|v| {
                            v.rows
                                .borrow()
                                .get(position as usize)
                                .filter(|r| r.row.key == row.key)
                                .map(|r| r.score)
                        })
                        .unwrap_or(0.5);
                    let word = alike_word(score);
                    // Translators: the accessible name of "How alike":
                    // its word, then the score of 100.
                    let name = fill(
                        &tr("{}, {} of 100"),
                        &[&word, &of_hundred(score).to_string()],
                    );
                    Value::Likeness(alike_share(score), word, name)
                }
                _ => Value::Text(String::new()),
            },
            |_| {},
        );
    }

    fn connect(
        self: &Rc<Self>,
        play: &gtk::Button,
        enqueue: &gtk::Button,
        radio: &gtk::Button,
        save: &gtk::Button,
        playback: &Rc<Playback>,
    ) {
        let weak = Rc::downgrade(self);
        play.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.play_from(0);
            }
        });
        let weak = Rc::downgrade(self);
        enqueue.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.enqueue();
            }
        });
        let weak = Rc::downgrade(self);
        radio.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade()
                && let Some(seed) = view.seed.get()
            {
                view.backend
                    .send(PlayerRequest::StartRadio(radio_seed(seed)));
            }
        });
        let weak = Rc::downgrade(self);
        save.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.save_as_playlist();
            }
        });
        let weak = Rc::downgrade(self);
        self.more.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.limit.set(view.limit.get() + STEP);
                view.load();
            }
        });
        let weak = Rc::downgrade(self);
        self.table.connect(move |position| {
            if let Some(view) = weak.upgrade() {
                view.play_from(position);
            }
        });
        let weak = Rc::downgrade(self);
        self.settings_button.connect_clicked(move |_| {
            if let Some(window) = weak.upgrade().and_then(|v| v.window()) {
                crate::dialogs::settings::open_at(
                    &window,
                    crate::dialogs::settings::Page::Library,
                    crate::dialogs::settings::track_analysis::ROW,
                );
            }
        });
        // The not-yet page follows the analysis, and fills in by itself.
        let weak = Rc::downgrade(self);
        playback.connect_event(move |update| {
            if let (Update::Analysis(progress), Some(view)) = (update, weak.upgrade()) {
                view.analysis_moved(progress);
            }
        });
    }

    /// Shows the tracks that sound like `seed`.
    pub(crate) fn show(self: &Rc<Self>, seed: SimilarSeed) {
        if self.seed.replace(Some(seed)) != Some(seed) {
            self.limit.set(STEP);
            self.rows.borrow_mut().clear();
            self.table.store.remove_all();
            self.title.set_text("");
            self.subtitle.set_text("");
            self.seed_words.replace(String::new());
            self.load_seed(seed);
        }
        self.load();
    }

    /// The seed's title, picture and what it is.
    fn load_seed(self: &Rc<Self>, seed: SimilarSeed) {
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let found = backend
                .query(move |l| {
                    Ok(match seed {
                        SimilarSeed::Track(key) => {
                            l.track_rows(&[key])?.into_iter().next().map(|r| {
                                let words = [r.artist.clone(), r.album_title.clone()]
                                    .into_iter()
                                    .filter(|w| !w.is_empty())
                                    .collect::<Vec<_>>()
                                    .join(" · ");
                                (r.title.clone(), words, r.cover, r.cover_seed, false)
                            })
                        }
                        SimilarSeed::Album(key) => l.album(key)?.map(|a| {
                            (
                                a.row.title.clone(),
                                fill(&tr("Album by {}"), &[&a.row.artist]),
                                a.row.cover,
                                a.row.cover_seed,
                                false,
                            )
                        }),
                        SimilarSeed::Artist(key) => l
                            .artist(key)?
                            .map(|a| (a.row.name.clone(), tr("Artist"), a.row.photo, 0, true)),
                    })
                })
                .await;
            let Some(view) = weak.upgrade() else { return };
            if view.seed.get() != Some(seed) {
                return;
            }
            if let Ok(Some((title, words, cover, cover_seed, artist))) = found {
                view.title.set_text(&title);
                view.seed_words.replace(words);
                if let Some(stack) = view.cover.parent().and_downcast::<gtk::Stack>() {
                    stack.set_visible_child_name(if artist { "avatar" } else { "cover" });
                }
                if artist {
                    view.avatar.set_name(&title);
                    view.avatar.set_photo(&view.covers, cover);
                } else {
                    view.covers.show(&view.cover, cover, cover_seed, 88);
                }
                view.show_subtitle();
            }
        });
    }

    /// Asks for the list again.
    fn load(self: &Rc<Self>) {
        let Some(seed) = self.seed.get() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let limit = self.limit.get();
        let backend = self.backend.clone();
        let services = backend.services();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let found = backend.query(move |l| l.similar_tracks(seed, limit)).await;
            let progress = gtk::gio::spawn_blocking(move || services.analysis_progress())
                .await
                .ok()
                .flatten();
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            match found {
                Ok(SimilarFound::Tracks { rows, more }) => view.show_rows(rows, more),
                Ok(SimilarFound::NotYet(reason)) => view.show_not_yet(reason, progress),
                Err(error) => {
                    if let Some(window) = view.window() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    fn show_rows(&self, rows: Vec<SimilarRow>, more: bool) {
        self.not_yet.set(None);
        self.has_more.set(more);
        let list: Vec<TrackRow> = rows.iter().map(|r| r.row.clone()).collect();
        let length: Duration = list.iter().filter_map(|r| r.duration).sum();
        let count = list.len();
        self.rows.replace(rows);
        self.table.show(&list);
        self.few.set_visible(count < FEW);
        self.more.set_visible(more);
        let left = if more {
            fill(
                &ntr("The {} closest track", "The {} closest tracks", count),
                &[&format::count(count)],
            )
        } else {
            fill(
                &ntr(
                    "All {} track that sounds alike",
                    "All {} tracks that sound alike",
                    count,
                ),
                &[&format::count(count)],
            )
        };
        self.status_left
            .set_text(&format!("{left} · {}", format::minutes(length)));
        for b in &self.buttons {
            b.set_sensitive(count > 0);
        }
        self.body.set_visible_child_name("list");
        self.show_subtitle();
    }

    fn show_subtitle(&self) {
        let words = self.seed_words.borrow().clone();
        let count = self.rows.borrow().len();
        let mut parts: Vec<String> = Vec::new();
        if !words.is_empty() {
            parts.push(words);
        }
        if self.not_yet.get().is_none() && count > 0 {
            let several = !matches!(self.seed.get(), Some(SimilarSeed::Track(_)));
            parts.push(if several {
                fill(
                    &ntr(
                        "the {} track that sounds most like its tracks",
                        "the {} tracks that sound most like its tracks",
                        count,
                    ),
                    &[&format::count(count)],
                )
            } else {
                fill(
                    &ntr(
                        "the {} track that sounds most like it",
                        "the {} tracks that sound most like it",
                        count,
                    ),
                    &[&format::count(count)],
                )
            });
        }
        self.subtitle.set_text(&parts.join(" · "));
    }

    fn show_not_yet(&self, reason: NotYet, progress: Option<AnalysisProgress>) {
        self.not_yet.set(Some(reason));
        self.rows.borrow_mut().clear();
        self.table.store.remove_all();
        let off = progress.is_some_and(|p| p.state == AnalysisState::Off);
        let several = !matches!(self.seed.get(), Some(SimilarSeed::Track(_)));
        let (headline, paragraph) = not_yet_words(reason, off, several);
        self.headline.set_text(&headline);
        self.paragraph.set_text(&paragraph);
        let show_progress = !off && reason != NotYet::Unreadable;
        self.progress
            .set_visible(show_progress && progress.is_some());
        if let Some(progress) = progress {
            self.show_progress(&progress);
        }
        self.settings_button
            .set_label(&if off && reason == NotYet::NotAnalysed {
                tr("Turn it on in Settings…")
            } else {
                tr("Open the analysis settings")
            });
        self.settings_button
            .set_visible(reason != NotYet::Unreadable);
        for b in &self.buttons {
            b.set_sensitive(false);
        }
        self.body.set_visible_child_name("not-yet");
        self.show_subtitle();
    }

    fn show_progress(&self, progress: &AnalysisProgress) {
        self.analysed.set(Some(progress.analyzed));
        self.progress_count.set_text(&fill(
            &ntr(
                "{} of {} track analysed",
                "{} of {} tracks analysed",
                progress.total,
            ),
            &[
                &format::count(progress.analyzed),
                &format::count(progress.total),
            ],
        ));
        let share = if progress.total == 0 {
            1.0
        } else {
            progress.analyzed as f64 / progress.total as f64
        };
        self.progress_meter.set_value(share.clamp(0.0, 1.0));
        self.progress_state
            .set_text(&crate::dialogs::settings::track_analysis::state_text(
                progress,
            ));
    }

    /// The analysis moved on: the progress follows, and the list is asked
    /// for again once the seed may be analysed or enough tracks are.
    fn analysis_moved(self: &Rc<Self>, progress: &AnalysisProgress) {
        let Some(reason) = self.not_yet.get() else {
            return;
        };
        if !self.root.is_mapped() {
            return;
        }
        let before = self.analysed.get();
        self.show_progress(progress);
        let changed = before != Some(progress.analyzed);
        let worth = match reason {
            NotYet::TooFew(_) => !too_few_analysed(progress.analyzed, progress.total),
            NotYet::NotAnalysed | NotYet::Unreadable => true,
        };
        if changed && worth {
            self.load();
        }
    }

    fn keys(&self) -> Vec<crate::backend::TrackKey> {
        self.rows.borrow().iter().map(|r| r.row.key).collect()
    }

    fn title(&self) -> String {
        fill(&tr("Sounds like {}"), &[&self.title.text()])
    }

    fn play_from(&self, start: usize) {
        let tracks = self.keys();
        if tracks.is_empty() {
            return;
        }
        let start = start.min(tracks.len() - 1);
        self.backend.send(PlayerRequest::PlayTracks {
            tracks,
            start,
            title: self.title(),
            shuffle: false,
        });
    }

    fn enqueue(&self) {
        let rows: Vec<Rc<TrackRow>> = self
            .rows
            .borrow()
            .iter()
            .map(|r| Rc::new(r.row.clone()))
            .collect();
        if rows.is_empty() {
            return;
        }
        if let Some(window) = self.window() {
            crate::views::actions::play(
                &window,
                self.backend.clone(),
                Target::tracks(rows),
                Some(false),
            );
        }
    }

    /// Makes a playlist of the tracks listed, named after the seed, and
    /// opens it.
    fn save_as_playlist(&self) {
        let Some(window) = self.window() else { return };
        let tracks = self.keys();
        if tracks.is_empty() {
            return;
        }
        let taken: Vec<String> = window.playlist_rows().into_iter().map(|p| p.name).collect();
        let name = crate::window::playlists::unused_name(&self.title(), &taken);
        let backend = self.backend.clone();
        let weak = window.downgrade();
        glib::spawn_future_local(async move {
            let created = backend
                .query(move |l| {
                    let key = l.create_playlist(&name)?;
                    l.add_to_playlist(key, &tracks, None)?;
                    Ok(key)
                })
                .await;
            let Some(window) = weak.upgrade() else { return };
            match created {
                Ok(key) => {
                    window.playlists_changed(None);
                    window.open_playlist(key);
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// The selected tracks as a context menu target.
    pub(crate) fn target(&self) -> Option<Target> {
        self.table.target()
    }

    /// The list, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.table.list.clone().upcast()
    }

    /// What the page shows (for tests).
    pub(crate) fn state(&self) -> SimilarState {
        SimilarState {
            title: self.title.text().to_string(),
            subtitle: self.subtitle.text().to_string(),
            tracks: self.rows.borrow().len(),
            words: self
                .rows
                .borrow()
                .iter()
                .map(|r| alike_word(r.score))
                .collect(),
            status: self.status_left.text().to_string(),
            more: self.more.is_visible(),
            not_yet: (self.body.visible_child_name().as_deref() == Some("not-yet"))
                .then(|| self.headline.text().to_string()),
            buttons_sensitive: self
                .buttons
                .iter()
                .all(gtk::prelude::WidgetExt::is_sensitive),
            headers: self.table.header_texts(),
            row_shape: self.table.first_row_shape(),
            settings_button: self
                .settings_button
                .label()
                .map(|l| l.to_string())
                .unwrap_or_default(),
            progress: self.progress.is_visible(),
        }
    }

    /// Follows the narrow window.
    pub(crate) fn set_narrow(&self, narrow: bool) {
        self.table.set_narrow(narrow);
    }

    /// Shows 50 more (for tests).
    pub(crate) fn demo_more(self: &Rc<Self>) {
        self.more.emit_clicked();
    }

    /// Clicks Save as playlist (for tests).
    pub(crate) fn demo_save(&self) {
        self.save_as_playlist();
    }
}

/// What the page of tracks that sound alike shows, for tests.
#[derive(Debug, Clone, Default)]
pub struct SimilarState {
    /// The seed's title.
    pub title: String,
    /// What the seed is and the size of the list.
    pub subtitle: String,
    /// The number of tracks listed.
    pub tracks: usize,
    /// The word of How alike in each row.
    pub words: Vec<String>,
    /// The status bar.
    pub status: String,
    /// Whether "Show 50 more" shows.
    pub more: bool,
    /// The headline of the not-yet page, when it shows.
    pub not_yet: Option<String>,
    /// Whether Play, Add to queue, Start radio and Save as playlist can be used.
    pub buttons_sensitive: bool,
    /// The table's headers.
    pub headers: Vec<String>,
    /// The artist under the title of the first row, and whether it shows the
    /// likeness bar.
    pub row_shape: (Option<String>, bool),
    /// The label of the not-yet page's button.
    pub settings_button: String,
    /// Whether the not-yet page shows the analysis progress.
    pub progress: bool,
}

/// The player's seed for a radio from `seed`.
fn radio_seed(seed: SimilarSeed) -> RadioSeed {
    match seed {
        SimilarSeed::Track(key) => RadioSeed::Track(key.0),
        SimilarSeed::Album(key) => RadioSeed::Album(i64::try_from(key.0).unwrap_or(0)),
        SimilarSeed::Artist(key) => RadioSeed::Artist(i64::try_from(key.0).unwrap_or(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_alike_reads_as_drawn() {
        assert_eq!(alike_word(0.94), "Very close");
        assert_eq!(alike_word(0.85), "Very close");
        assert_eq!(alike_word(0.84), "Close");
        assert_eq!(alike_word(0.70), "Close");
        assert_eq!(alike_word(0.69), "Somewhat alike");
        assert_eq!(alike_share(0.5), 0.0);
        assert_eq!(alike_share(1.0), 1.0);
        assert_eq!(of_hundred(0.94), 94);
    }

    #[test]
    fn each_reason_has_its_words() {
        assert_eq!(
            not_yet_words(NotYet::NotAnalysed, false, false).0,
            "This track has not been analysed yet"
        );
        assert_eq!(
            not_yet_words(NotYet::NotAnalysed, false, true).0,
            "These tracks have not been analysed yet"
        );
        assert_eq!(
            not_yet_words(NotYet::NotAnalysed, true, false).0,
            "Track analysis is turned off"
        );
        let (head, text) = not_yet_words(NotYet::TooFew(212), false, false);
        assert_eq!(head, "Too few tracks are analysed so far");
        assert!(text.contains("212 tracks"));
        assert_eq!(
            not_yet_words(NotYet::Unreadable, false, false).0,
            "This track could not be analysed"
        );
    }
}
