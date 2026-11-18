// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Now Playing: the full-window view of the playing track. It replaces the
//! sidebar, the content and the panel; the deck stays below it.
//!
//! - A top strip: "Library" (leaves the view), the segments Lyrics, Queue and
//!   Analyzer, and the window controls.
//! - On a dark ground tinted with the cover's colour: the cover, the title,
//!   the artist, "Album · Year · Track n", the rating and the play count; at
//!   the right the synced lyrics with their times (a click seeks), the queue,
//!   or the analyzer: the spectrum and the level of each channel of what the
//!   device is given.
//! - The signal strip: the lamp and its state, the SignalPathChain, and the
//!   buffer and underrun counters.
//!
//! The view draws only when something changes: a new line glides into the
//! middle while playing, nothing moves while paused. The analyzer's
//! measurements are asked for only while its page is on screen, and they come
//! only while sound is played.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

use crate::backend::{Backend, PlaybackState, PlayerRequest, SignalState, TrackRow, Update};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::models::lyrics::{LyricsChange, LyricsModel};
use crate::playback::{Change, Playback, PlayerView};
use crate::style::color::Rgba;
use crate::widgets::level_meter::LevelMeter;
use crate::widgets::lyric_lines::{LinesStyle, LyricLines};
use crate::widgets::now_links::NowLinks;
use crate::widgets::signal_path_chain::SignalPathChain;
use crate::widgets::signal_path_list::state_label;
use crate::widgets::spectrum_view::SpectrumView;
use crate::widgets::{CoverArt, RatingStars, SignalLamp, label};
use crate::window::panel::lyrics::{offset_text, source_text};

/// The cover's side when there is room (the design's at 1440 × 900; the
/// column may be 400 wide), and the least it shrinks to.
const COVER_MAX: i32 = 340;
const COVER_MIN: i32 = 140;

/// Called with the size the view gets.
type Resized = Box<dyn Fn(i32, i32)>;

/// The ground with the cover's tint, and the size the cover may take.
mod imp_backdrop {
    use super::*;

    #[derive(Default)]
    pub(crate) struct Backdrop {
        pub(super) tint: Cell<Option<Rgba>>,
        /// Called with the allocated size, so the cover can follow it.
        pub(super) resized: std::cell::RefCell<Option<Resized>>,
        pub(super) size: Cell<(i32, i32)>,
    }

    impl std::fmt::Debug for Backdrop {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Backdrop")
                .field("tint", &self.tint.get())
                .finish()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Backdrop {
        const NAME: &'static str = "HkNowPlayingBackdrop";
        type Type = super::Backdrop;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    impl ObjectImpl for Backdrop {}

    impl WidgetImpl for Backdrop {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            if self.size.replace((width, height)) != (width, height)
                && let Some(resized) = self.resized.borrow().as_ref()
            {
                resized(width, height);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let bounds = graphene::Rect::new(0.0, 0.0, w, h);
            let ground = crate::style::tokens::glass::BG;
            snapshot.append_color(&ground.to_gdk(), &bounds);
            if let Some(tint) = self.tint.get() {
                // A soft tint of the cover's colour, an ellipse of 1000 × 640
                // centred at 20 % / 40 %, at half strength fading to nothing.
                let mut strong = tint.to_gdk();
                strong.set_alpha(0.5);
                let mut clear = tint.to_gdk();
                clear.set_alpha(0.0);
                snapshot.append_radial_gradient(
                    &bounds,
                    &graphene::Point::new(w * 0.2, h * 0.4),
                    500.0,
                    320.0,
                    0.0,
                    1.0,
                    &[
                        gsk::ColorStop::new(0.0, strong),
                        gsk::ColorStop::new(1.0, clear),
                    ],
                );
            }
        }
    }
}

glib::wrapper! {
    /// The ground of Now Playing.
    pub(crate) struct Backdrop(ObjectSubclass<imp_backdrop::Backdrop>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Backdrop {
    fn new() -> Self {
        glib::Object::new()
    }

    fn set_tint(&self, tint: Option<Rgba>) {
        if self.imp().tint.replace(tint) != tint {
            self.queue_draw();
        }
    }
}

/// The view's widgets.
#[derive(Debug)]
pub(crate) struct NowPlayingView {
    root: gtk::Overlay,
    backdrop: Backdrop,
    library: gtk::Button,
    controls: gtk::WindowControls,
    cover: CoverArt,
    title: gtk::Label,
    artist: gtk::Label,
    album: gtk::Label,
    /// The artists and the album as links, in place of the two labels.
    artist_links: NowLinks,
    album_links: NowLinks,
    rating: RatingStars,
    plays: gtk::Label,
    segment_lyrics: gtk::ToggleButton,
    segment_queue: gtk::ToggleButton,
    segment_analyzer: gtk::ToggleButton,
    pages: gtk::Stack,
    spectrum: SpectrumView,
    meter: LevelMeter,
    lines: Rc<LyricLines>,
    note: gtk::Label,
    queue: gtk::ListBox,
    lamp: SignalLamp,
    state: gtk::Label,
    chain: SignalPathChain,
    buffer: gtk::Label,
    underruns: gtk::Label,
    covers: Rc<CoverCache>,
    lyrics: Rc<LyricsModel>,
    playback: std::rc::Weak<Playback>,
    /// Whether the view is shown.
    active: Cell<bool>,
    /// What had the focus before the view opened.
    return_focus: std::cell::RefCell<Option<glib::WeakRef<gtk::Widget>>>,
    /// The cover's side now.
    side: Cell<i32>,
}

/// "Title · 6:03" of the track after the current one, or `None` at the end.
pub(crate) fn next_text(view: &PlayerView) -> Option<String> {
    let next = view.queue.items.get(view.queue.current? + 1)?;
    Some(match next.duration {
        Some(d) => fill(&tr("{} · {}"), &[&next.title, &format::duration(d)]),
        None => next.title.clone(),
    })
}

fn segment(text: &str) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::with_label(text);
    button.set_valign(gtk::Align::Center);
    button
}

impl NowPlayingView {
    /// The view over `backend`, following `playback` and `lyrics`.
    pub(crate) fn new(
        backend: &Backend,
        playback: &Rc<Playback>,
        lyrics: &Rc<LyricsModel>,
        covers: &Rc<CoverCache>,
    ) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("hk-now-playing-view");

        // Top strip.
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        strip.add_css_class("hk-np-strip");
        let library = gtk::Button::new();
        let library_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        library_content.append(&gtk::Image::from_icon_name("pan-down-symbolic"));
        library_content.append(&gtk::Label::new(Some(&tr("Library"))));
        library.set_child(Some(&library_content));
        library.add_css_class("hk-np-library");
        library.set_valign(gtk::Align::Center);
        library.set_action_name(Some("win.now-playing"));
        library.set_tooltip_text(Some(&tr("Back to the library (F11)")));
        strip.append(&library);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        strip.append(&spacer);
        let segments = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        segments.add_css_class("hk-seg");
        segments.set_valign(gtk::Align::Center);
        segments.update_property(&[gtk::accessible::Property::Label(&tr("Show"))]);
        let segment_lyrics = segment(&tr("Lyrics"));
        segment_lyrics.set_active(true);
        let segment_queue = segment(&tr("Queue"));
        segment_queue.set_group(Some(&segment_lyrics));
        let segment_analyzer = segment(&tr("Analyzer"));
        segment_analyzer.set_group(Some(&segment_lyrics));
        segments.append(&segment_lyrics);
        segments.append(&segment_queue);
        segments.append(&segment_analyzer);
        strip.append(&segments);
        let controls = gtk::WindowControls::new(gtk::PackType::End);
        controls.set_valign(gtk::Align::Center);
        controls.add_css_class("hk-np-controls");
        strip.append(&controls);
        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&strip));
        content.append(&handle);

        // Body: the track at the left, the lyrics or the queue at the right.
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 72);
        body.add_css_class("hk-np-body");
        body.set_vexpand(true);
        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.add_css_class("hk-np-track");
        left.set_valign(gtk::Align::Center);
        left.set_size_request(COVER_MIN, -1);
        let cover = CoverArt::new(COVER_MAX, 10.0);
        cover.add_css_class("hk-np-cover");
        cover.set_halign(gtk::Align::Start);
        left.append(&cover);
        let title = gtk::Label::new(None);
        title.add_css_class("hk-np-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        title.set_lines(2);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_max_width_chars(1);
        title.set_hexpand(false);
        left.append(&title);
        let artist = label("", &["hk-np-artist"]);
        artist.set_max_width_chars(1);
        left.append(&artist);
        let album = label("", &["hk-np-album"]);
        album.set_max_width_chars(1);
        left.append(&album);
        let artist_links = NowLinks::new(&["hk-np-artist"]);
        artist_links.set_backend(backend);
        left.insert_child_after(artist_links.widget(), Some(&artist));
        artist.set_visible(false);
        let album_links = NowLinks::new(&["hk-np-album"]);
        album_links.widget().set_visible(false);
        left.insert_child_after(album_links.widget(), Some(&album));
        let stars = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        stars.add_css_class("hk-np-stars");
        let rating = RatingStars::default();
        rating.set_valign(gtk::Align::Center);
        stars.append(&rating);
        let plays = label("", &["hk-np-plays"]);
        stars.append(&plays);
        left.append(&stars);
        body.append(&left);

        let pages = gtk::Stack::new();
        pages.set_hexpand(true);
        let lines = LyricLines::new(LinesStyle::Glass);
        let b = backend.clone();
        lines.connect_seek(move |to| b.send(PlayerRequest::Seek(to)));
        let lyrics_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        lyrics_page.append(lines.widget());
        let note = label("", &["hk-np-note"]);
        lyrics_page.append(&note);
        pages.add_named(&lyrics_page, Some("lyrics"));
        let none = gtk::Label::new(Some(&tr("No lyrics for this track")));
        none.add_css_class("hk-np-none");
        none.set_xalign(0.0);
        none.set_valign(gtk::Align::Center);
        pages.add_named(&none, Some("none"));
        let queue = gtk::ListBox::new();
        queue.set_selection_mode(gtk::SelectionMode::None);
        queue.set_activate_on_single_click(false);
        queue.add_css_class("hk-np-queue");
        queue.update_property(&[gtk::accessible::Property::Label(&tr("Queue"))]);
        let b = backend.clone();
        queue.connect_row_activated(move |_, row| {
            if let Ok(entry) = row.widget_name().parse::<u64>() {
                b.send(PlayerRequest::PlayEntry(entry));
            }
        });
        let queue_scroller = gtk::ScrolledWindow::new();
        queue_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        queue_scroller.set_child(Some(&queue));
        pages.add_named(&queue_scroller, Some("queue"));
        let (analyzer_page, spectrum, meter) = analyzer_page();
        pages.add_named(&analyzer_page, Some("analyzer"));
        body.append(&pages);
        content.append(&body);

        // The signal strip.
        let signal = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        signal.add_css_class("hk-np-signal");
        let head = gtk::Box::new(gtk::Orientation::Vertical, 4);
        head.set_valign(gtk::Align::Center);
        head.append(&label(&tr("Signal path"), &["hk-section-label"]));
        let lamp_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let lamp = SignalLamp::default();
        lamp.set_valign(gtk::Align::Center);
        lamp_line.append(&lamp);
        let state = label("", &["hk-signal", "idle"]);
        state.set_ellipsize(gtk::pango::EllipsizeMode::None);
        lamp_line.append(&state);
        head.append(&lamp_line);
        signal.append(&head);
        let chain = SignalPathChain::new();
        chain.widget().set_hexpand(true);
        signal.append(chain.widget());
        let stats = gtk::Box::new(gtk::Orientation::Vertical, 4);
        stats.set_valign(gtk::Align::Center);
        let buffer = label("", &["hk-mono", "hk-np-stat"]);
        buffer.set_xalign(1.0);
        let underruns = label("", &["hk-mono", "hk-np-stat"]);
        underruns.set_xalign(1.0);
        stats.append(&buffer);
        stats.append(&underruns);
        signal.append(&stats);
        content.append(&signal);

        let backdrop = Backdrop::new();
        backdrop.set_hexpand(true);
        backdrop.set_vexpand(true);
        let root = gtk::Overlay::new();
        root.add_css_class("hk-glass");
        root.add_css_class("hk-now-playing-root");
        root.set_child(Some(&backdrop));
        root.add_overlay(&content);
        root.set_measure_overlay(&content, true);
        root.set_vexpand(true);
        root.set_visible(false);
        root.set_accessible_role(gtk::AccessibleRole::Group);
        root.update_property(&[gtk::accessible::Property::Label(&tr("Now Playing"))]);

        let view = Rc::new(Self {
            root,
            backdrop,
            library,
            controls,
            cover,
            title,
            artist,
            album,
            artist_links,
            album_links,
            rating,
            plays,
            segment_lyrics,
            segment_queue,
            segment_analyzer,
            pages,
            spectrum,
            meter,
            lines,
            note,
            queue,
            lamp,
            state,
            chain,
            buffer,
            underruns,
            covers: covers.clone(),
            lyrics: lyrics.clone(),
            playback: Rc::downgrade(playback),
            active: Cell::new(false),
            return_focus: std::cell::RefCell::new(None),
            side: Cell::new(COVER_MAX),
        });

        for button in [
            &view.segment_lyrics,
            &view.segment_queue,
            &view.segment_analyzer,
        ] {
            let weak = Rc::downgrade(&view);
            button.connect_toggled(move |button| {
                if let Some(view) = weak.upgrade()
                    && button.is_active()
                {
                    view.show_page();
                }
            });
        }
        // The cover takes what the height leaves: the strip, the signal strip,
        // the padding and the lines under it.
        let weak = Rc::downgrade(&view);
        *view.backdrop.imp().resized.borrow_mut() = Some(Box::new(move |width, height| {
            let Some(view) = weak.upgrade() else { return };
            let by_height = height - 46 - 78 - 48 - 150;
            let by_width = (width - 144 - 72) * 2 / 5;
            let side = by_height.min(by_width).clamp(COVER_MIN, COVER_MAX);
            if view.side.replace(side) != side {
                // Not during the allocation itself.
                let cover = view.cover.clone();
                glib::idle_add_local_once(move || cover.set_shape(side, 10.0));
            }
        }));
        // The measurements are asked for while the analyzer is on screen.
        let b = backend.clone();
        analyzer_page.connect_map(move |_| {
            let reduced = crate::style::current().reduced_motion;
            b.follow_analyzer(Some(if reduced { 10 } else { 30 }));
        });
        let b = backend.clone();
        analyzer_page.connect_unmap(move |_| b.follow_analyzer(None));
        let weak = Rc::downgrade(&view);
        playback.connect_event(move |update| {
            if let (Some(view), Update::Analyzer(frame)) = (weak.upgrade(), update) {
                view.show_analyzer(frame);
            }
        });
        let weak = Rc::downgrade(&view);
        playback.connect(move |player, change| {
            if let Some(view) = weak.upgrade() {
                view.update(player, change);
            }
        });
        let weak = Rc::downgrade(&view);
        lyrics.connect(move |model, change| {
            let Some(view) = weak.upgrade() else { return };
            match change {
                LyricsChange::Lyrics => view.show_lyrics(),
                LyricsChange::Line => view.lines.set_line(model.line()),
            }
        });
        view.show_lyrics();
        view
    }

    /// The view's root, hidden until [`set_active`](Self::set_active).
    pub(crate) fn widget(&self) -> &gtk::Overlay {
        &self.root
    }

    /// The widget that takes the focus when the view opens.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.library.clone().upcast()
    }

    /// Keeps `widget` to give the focus back to when the view closes.
    pub(crate) fn remember_focus(&self, widget: Option<gtk::Widget>) {
        self.return_focus.replace(widget.map(|w| w.downgrade()));
    }

    /// The widget to give the focus back to.
    pub(crate) fn take_focus(&self) -> Option<gtk::Widget> {
        self.return_focus.take().and_then(|w| w.upgrade())
    }

    /// Whether the view is shown.
    pub(crate) fn is_active(&self) -> bool {
        self.active.get()
    }

    /// Shows or hides the view; `integrated` shows the window controls in its
    /// strip.
    pub(crate) fn set_active(&self, active: bool, integrated: bool) {
        self.active.set(active);
        self.root.set_visible(active);
        self.controls.set_visible(integrated);
        if active && let Some(playback) = self.playback.upgrade() {
            let view = playback.view();
            self.show_stats(&view);
        }
    }

    fn show_page(&self) {
        if self.segment_queue.is_active() {
            self.pages.set_visible_child_name("queue");
        } else if self.segment_analyzer.is_active() {
            self.pages.set_visible_child_name("analyzer");
        } else {
            self.show_lyrics();
        }
    }

    fn show_lyrics(&self) {
        let lyrics = self.lyrics.lyrics();
        self.lines.set_lyrics(lyrics.as_deref(), self.lyrics.line());
        if self.segment_queue.is_active() || self.segment_analyzer.is_active() {
            return;
        }
        self.pages.set_visible_child_name(match &lyrics {
            Some(_) => "lyrics",
            None => "none",
        });
        if let Some(lyrics) = &lyrics {
            let mut note = source_text(Some(lyrics));
            if lyrics.synced {
                note = fill(
                    &tr("{} · offset {}"),
                    &[&note, &offset_text(lyrics.offset_ms)],
                );
            }
            self.note.set_text(&note);
        }
    }

    fn update(&self, view: &PlayerView, change: Change) {
        match change {
            Change::Track => {
                self.show_track(view);
                self.show_queue(view);
            }
            Change::Queue => self.show_queue(view),
            Change::Signal | Change::Output | Change::State => self.show_signal(view),
            Change::Stats if self.active.get() => self.show_stats(view),
            _ => {}
        }
    }

    fn show_track(&self, view: &PlayerView) {
        let Some(now) = &view.now else {
            self.cover.show(0, None);
            self.title.set_text(&tr("Nothing playing"));
            self.artist.set_text("");
            self.album.set_text("");
            self.artist_links.set("", None, &[]);
            self.album_links.widget().set_visible(false);
            self.album.set_visible(true);
            self.rating.set_rating(None);
            self.plays.set_text("");
            self.backdrop.set_tint(None);
            return;
        };
        self.covers
            .show(&self.cover, now.cover, now.cover_seed, COVER_MAX);
        self.title.set_text(&now.track.title);
        self.artist.set_text(&now.track.artist);
        let mut parts: Vec<String> = Vec::new();
        if !now.album_title.is_empty() {
            parts.push(now.album_title.clone());
        }
        if let Some(year) = now.year.or(now.track.year) {
            parts.push(year.to_string());
        }
        if let Some(number) = now.track.number {
            parts.push(fill(&tr("Track {}"), &[&number.to_string()]));
        }
        self.album.set_text(&parts.join(" · "));
        self.artist_links.set(&now.track.artist, None, &[]);
        let linked = now.track.album.0 != 0 && !now.album_title.is_empty();
        if linked {
            self.album_links
                .set("", Some((&now.album_title, now.track.album)), &parts[1..]);
        }
        self.album.set_visible(!linked);
        self.album_links.widget().set_visible(linked);
        self.rating.set_rating(now.track.rating);
        self.plays.set_text(&fill(
            &ntr("{} play", "{} plays", now.track.plays as usize),
            &[&format::count(now.track.plays)],
        ));
        // The tint follows the cover; a thumbnail that arrives later is
        // taken when the view opens again.
        self.backdrop.set_tint(Some(self.cover.tint()));
    }

    fn show_queue(&self, view: &PlayerView) {
        crate::widgets::remove_rows(&self.queue);
        let queue = &view.queue;
        for (index, track) in queue.items.iter().enumerate() {
            let current = queue.current == Some(index);
            self.queue.append(&queue_row(
                track,
                current,
                queue.current.is_some_and(|c| index < c),
            ));
        }
    }

    fn show_signal(&self, view: &PlayerView) {
        self.lamp.set_state(view.output.signal);
        let path = view.signal.as_deref();
        let text = state_label(view.output.signal, path).unwrap_or_default();
        if self.state.text() != text {
            self.state.set_text(&text);
        }
        for class in ["idle", "bit-perfect", "processed"] {
            self.state.remove_css_class(class);
        }
        self.state.add_css_class(match view.output.signal {
            SignalState::BitPerfect => "bit-perfect",
            SignalState::Processed { .. } => "processed",
            _ => "idle",
        });
        self.chain.show(path, &view.output.device);
    }

    fn show_stats(&self, view: &PlayerView) {
        let (buffer, underruns) = match &view.stats {
            Some(stats) if view.state != PlaybackState::Idle => (
                fill(&tr("Buffer {} ms"), &[&stats.buffer_ms.to_string()]),
                fill(&tr("Underruns {}"), &[&format::count(stats.underruns)]),
            ),
            _ => (String::new(), String::new()),
        };
        if self.buffer.text() != buffer {
            self.buffer.set_text(&buffer);
        }
        if self.underruns.text() != underruns {
            self.underruns.set_text(&underruns);
        }
    }

    /// What the view shows, in words, for tests: the title, the lyrics page,
    /// the lines with the current one marked, and the signal chain.
    pub(crate) fn texts(&self) -> Vec<String> {
        let mut texts = vec![
            self.title.text().to_string(),
            self.album.text().to_string(),
            self.pages
                .visible_child_name()
                .map(|n| n.to_string())
                .unwrap_or_default(),
            self.note.text().to_string(),
        ];
        let (lines, current) = self.lines.texts();
        for (i, line) in lines.iter().enumerate() {
            texts.push(if Some(i) == current {
                format!("> {line}")
            } else {
                line.clone()
            });
        }
        texts.extend(self.chain.texts());
        texts
    }

    /// Shows a measurement of the analyzer, while its page is on screen.
    fn show_analyzer(&self, frame: &crate::backend::AnalyzerFrame) {
        if self.spectrum.is_mapped() {
            self.spectrum.show(frame);
            self.meter.show(frame);
        }
    }

    /// The segment that shows the analyzer, the spectrum and the meter, for
    /// tests.
    pub(crate) fn analyzer(&self) -> (&gtk::ToggleButton, &SpectrumView, &LevelMeter) {
        (&self.segment_analyzer, &self.spectrum, &self.meter)
    }

    /// The lyrics lines, for tests.
    pub(crate) fn lines(&self) -> &Rc<LyricLines> {
        &self.lines
    }
}

/// The analyzer's page: the spectrum and the channel levels, each under a
/// section label with its measure at the right, and where the sound is
/// measured.
fn analyzer_page() -> (gtk::Box, SpectrumView, LevelMeter) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.add_css_class("hk-np-analyzer");
    page.set_valign(gtk::Align::Center);
    let header = |title: &str, measure: &str| {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("hk-np-analyzer-head");
        let title = label(title, &["hk-section-label"]);
        title.set_hexpand(true);
        row.append(&title);
        let measure = label(measure, &["hk-mono", "hk-np-analyzer-measure"]);
        measure.set_xalign(1.0);
        row.append(&measure);
        row
    };
    page.append(&header(
        &tr("Spectrum"),
        &tr("1/5 OCTAVE · PEAK HOLD 1.5 s"),
    ));
    let spectrum = SpectrumView::default();
    page.append(&spectrum);
    let level = header(&tr("Level"), &tr("RMS 300 ms · dBFS"));
    level.add_css_class("hk-np-analyzer-level");
    page.append(&level);
    let meter = LevelMeter::default();
    page.append(&meter);
    let note = label(
        &tr("Measured after the last processing stage, before the device"),
        &["hk-mono", "hk-np-analyzer-note"],
    );
    page.append(&note);
    (page, spectrum, meter)
}

/// A row of the queue in Now Playing: number or position, title and artist,
/// duration; the current one bold. Double-click or Enter plays it.
fn queue_row(track: &TrackRow, current: bool, history: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    if let Some(entry) = track.entry {
        row.set_widget_name(&entry.to_string());
    }
    if current {
        row.add_css_class("current");
    }
    if history {
        row.add_css_class("history");
    }
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let text = if track.artist.is_empty() {
        track.title.clone()
    } else {
        fill(&tr("{} · {}"), &[&track.title, &track.artist])
    };
    let title = label(&text, &[]);
    title.set_hexpand(true);
    title.set_max_width_chars(1);
    line.append(&title);
    let length = label(
        &track.duration.map(format::duration).unwrap_or_default(),
        &["hk-mono", "hk-np-length"],
    );
    line.append(&length);
    row.set_child(Some(&line));
    row.update_property(&[gtk::accessible::Property::Label(&text)]);
    row
}
