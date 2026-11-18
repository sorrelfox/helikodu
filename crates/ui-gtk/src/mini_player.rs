// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mini player: a small window of its own with the cover, the playing
//! track, the seek bar, the transport and the output's lamp; under it a
//! drawer with the queue and the lyrics.
//!
//! It takes the place of the main window: opening it hides the main window,
//! "Back to the full window" brings that back. Its close button hides it; when
//! the main window is hidden too, that is closing the program's window, which
//! keeps playing in the tray where the settings say so and quits otherwise.
//! Ctrl+Shift+M (`app.mini-player`) opens and closes it.
//!
//! Like the deck it follows the colour scheme: glass in the dark scheme, the
//! window's own light surfaces in the light one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::backend::{Backend, PlaybackState, PlayerRequest, SignalState, TrackRow};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::models::lyrics::{LyricsChange, LyricsModel};
use crate::playback::{Change, Playback, PlayerView};
use crate::widgets::lyric_lines::{LinesStyle, LyricLines};
use crate::widgets::{CoverArt, PlayingIndicator, SignalLamp, WaveformSeekBar, icon_button, label};
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "mini-player";

/// The side of the cover.
const COVER: i32 = 194;
/// The window's width.
const WIDTH: i32 = 440;
/// The height of the drawer's list.
const DRAWER_BODY: i32 = 262;

/// The mini player window and its parts.
pub(crate) struct MiniPlayer {
    window: gtk::Window,
    main: glib::WeakRef<MainWindow>,
    backend: Backend,
    covers: Rc<CoverCache>,
    cover: CoverArt,
    title: gtk::Label,
    artist: gtk::Label,
    seek: WaveformSeekBar,
    elapsed: gtk::Label,
    remaining: gtk::Label,
    previous: gtk::Button,
    play: gtk::Button,
    next: gtk::Button,
    lamp: SignalLamp,
    format: gtk::Label,
    drawer_button: gtk::ToggleButton,
    drawer: gtk::Revealer,
    queue_tab: gtk::ToggleButton,
    lyrics_tab: gtk::ToggleButton,
    pages: gtk::Stack,
    note: gtk::Label,
    queue: gtk::ListBox,
    lines: Rc<LyricLines>,
    lyrics: Rc<LyricsModel>,
    /// The queue's summary, for the note while the Queue tab shows.
    queue_note: RefCell<String>,
    /// Whether the window is shown, so that hidden it draws nothing.
    shown: Cell<bool>,
}

impl std::fmt::Debug for MiniPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiniPlayer").finish_non_exhaustive()
    }
}

/// Adds `app.mini-player`, which opens or closes the mini player of the
/// program's main window.
pub(crate) fn install_actions(application: &gtk::Application) {
    let action = gio::SimpleAction::new("mini-player", None);
    let app = application.downgrade();
    action.connect_activate(move |_, _| {
        let Some(application) = app.upgrade() else {
            return;
        };
        let main = application
            .windows()
            .into_iter()
            .find_map(|w| w.downcast::<MainWindow>().ok());
        if let Some(main) = main {
            main.toggle_mini_player();
        }
    });
    application.add_action(&action);
}

impl MiniPlayer {
    /// The mini player of `main`, hidden until [`Self::present`].
    pub(crate) fn new(
        main: &MainWindow,
        backend: &Backend,
        playback: &Rc<Playback>,
        lyrics: &Rc<LyricsModel>,
        covers: &Rc<CoverCache>,
    ) -> Rc<Self> {
        let window = gtk::Window::builder()
            .title(tr("Helikodu"))
            .resizable(false)
            .decorated(false)
            .hide_on_close(true)
            .build();
        window.set_widget_name(NAME);
        if let Some(application) = main.application() {
            window.set_application(Some(&application));
        }
        crate::style::register_window(&window);
        window.add_css_class("hk-mini-window");

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-mini");
        root.set_size_request(WIDTH, -1);
        crate::style::glass_when_dark(&root);

        // The player: the cover, then the track, the seek bar and the transport.
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let cover = CoverArt::new(COVER, 0.0);
        let cover_button = gtk::Button::new();
        cover_button.add_css_class("hk-mini-cover");
        cover_button.set_child(Some(&cover));
        cover_button.set_tooltip_text(Some(&tr("Now Playing")));
        top.append(&cover_button);
        let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side.add_css_class("hk-mini-side");
        side.set_hexpand(true);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let title = label(&tr("Nothing playing"), &["hk-mini-title"]);
        let artist = label("", &["hk-mini-artist"]);
        text.append(&title);
        text.append(&artist);
        head.append(&text);
        let drawer_button = gtk::ToggleButton::new();
        drawer_button.set_icon_name("helikodu-drawer-symbolic");
        drawer_button.add_css_class("hk-icon");
        drawer_button.add_css_class("hk-mini-button");
        drawer_button.set_valign(gtk::Align::Start);
        let drawer_name = tr("Queue and lyrics");
        drawer_button.set_tooltip_text(Some(&drawer_name));
        drawer_button.update_property(&[gtk::accessible::Property::Label(&drawer_name)]);
        head.append(&drawer_button);
        let full = icon_button("view-fullscreen-symbolic", &tr("Back to the full window"));
        full.add_css_class("hk-mini-button");
        full.set_valign(gtk::Align::Start);
        head.append(&full);
        let close = icon_button("window-close-symbolic", &tr("Close"));
        close.add_css_class("hk-mini-button");
        close.set_valign(gtk::Align::Start);
        head.append(&close);
        side.append(&head);
        let seek = WaveformSeekBar::default();
        seek.set_vexpand(true);
        seek.set_valign(gtk::Align::End);
        side.append(&seek);
        let times = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let elapsed = label("-:--", &["hk-mono-meta", "hk-mini-time"]);
        elapsed.set_hexpand(true);
        let remaining = label("-:--", &["hk-mono-meta", "hk-mini-time"]);
        remaining.set_xalign(1.0);
        times.append(&elapsed);
        times.append(&remaining);
        side.append(&times);
        let transport = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        transport.set_margin_top(8);
        let previous = transport_button("media-skip-backward-symbolic", &tr("Previous track"));
        let play = transport_button("media-playback-start-symbolic", &tr("Play"));
        play.remove_css_class("hk-transport");
        play.add_css_class("hk-play");
        play.add_css_class("hk-mini-play");
        let next = transport_button("media-skip-forward-symbolic", &tr("Next track"));
        for button in [&previous, &play, &next] {
            transport.append(button);
        }
        let signal = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        signal.set_hexpand(true);
        signal.set_halign(gtk::Align::End);
        signal.set_valign(gtk::Align::Center);
        let lamp = SignalLamp::default();
        lamp.set_valign(gtk::Align::Center);
        let format_label = label("", &["hk-signal", "idle"]);
        signal.append(&lamp);
        signal.append(&format_label);
        transport.append(&signal);
        side.append(&transport);
        top.append(&side);
        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&top));
        root.append(&handle);

        // The drawer: Queue and Lyrics.
        let drawer_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let drawer_head = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        drawer_head.add_css_class("hk-mini-drawer-head");
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        tabs.add_css_class("hk-tabs");
        tabs.add_css_class("hk-mini-tabs");
        let queue_tab = gtk::ToggleButton::with_label(&tr("Queue"));
        let lyrics_tab = gtk::ToggleButton::with_label(&tr("Lyrics"));
        lyrics_tab.set_group(Some(&queue_tab));
        tabs.append(&queue_tab);
        tabs.append(&lyrics_tab);
        drawer_head.append(&tabs);
        let note = label("", &["hk-mini-note"]);
        note.set_hexpand(true);
        note.set_xalign(1.0);
        drawer_head.append(&note);
        drawer_box.append(&drawer_head);
        let pages = gtk::Stack::new();
        pages.set_size_request(-1, DRAWER_BODY);
        let queue = gtk::ListBox::new();
        queue.add_css_class("hk-queue");
        queue.add_css_class("hk-mini-queue");
        queue.set_activate_on_single_click(false);
        queue.set_selection_mode(gtk::SelectionMode::Single);
        queue.update_property(&[gtk::accessible::Property::Label(&tr("Queue"))]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&queue));
        pages.add_named(&scroller, Some("queue"));
        let lines = LyricLines::new(LinesStyle::Glass);
        lines.widget().add_css_class("hk-mini-lyrics");
        pages.add_named(lines.widget(), Some("lyrics"));
        let none = label(&tr("No lyrics for this track"), &["hk-mini-none"]);
        none.set_halign(gtk::Align::Center);
        none.set_valign(gtk::Align::Center);
        pages.add_named(&none, Some("no-lyrics"));
        drawer_box.append(&pages);
        let drawer = gtk::Revealer::new();
        drawer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        drawer.set_child(Some(&drawer_box));
        root.append(&drawer);
        window.set_child(Some(&root));

        let mini = Rc::new(Self {
            window,
            main: main.downgrade(),
            backend: backend.clone(),
            covers: covers.clone(),
            cover,
            title,
            artist,
            seek,
            elapsed,
            remaining,
            previous,
            play,
            next,
            lamp,
            format: format_label,
            drawer_button,
            drawer,
            queue_tab,
            lyrics_tab,
            pages,
            note,
            queue,
            lines,
            lyrics: lyrics.clone(),
            queue_note: RefCell::new(String::new()),
            shown: Cell::new(false),
        });
        mini.connect(&cover_button, &full, &close, playback);
        mini.restore_drawer();
        mini
    }

    fn connect(
        self: &Rc<Self>,
        cover: &gtk::Button,
        full: &gtk::Button,
        close: &gtk::Button,
        playback: &Rc<Playback>,
    ) {
        let weak = Rc::downgrade(self);
        cover.connect_clicked(move |_| {
            if let Some(mini) = weak.upgrade()
                && let Some(main) = mini.main.upgrade()
            {
                mini.hide();
                main.present();
                main.show_now_playing_view();
            }
        });
        let weak = Rc::downgrade(self);
        full.connect_clicked(move |_| {
            if let Some(mini) = weak.upgrade() {
                mini.back_to_full();
            }
        });
        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(mini) = weak.upgrade() {
                mini.window.close();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.connect_hide(move |_| {
            if let Some(mini) = weak.upgrade() {
                mini.hidden();
            }
        });
        for (button, request) in [
            (&self.previous, PlayerRequest::Previous),
            (&self.play, PlayerRequest::PlayPause),
            (&self.next, PlayerRequest::Next),
        ] {
            let backend = self.backend.clone();
            button.connect_clicked(move |_| backend.send(request.clone()));
        }
        let backend = self.backend.clone();
        self.seek
            .connect_seek(move |to| backend.send(PlayerRequest::Seek(to)));
        let backend = self.backend.clone();
        self.lines
            .connect_seek(move |to| backend.send(PlayerRequest::Seek(to)));
        // Space plays and pauses unless the focused widget takes it.
        let space = gtk::ShortcutController::new();
        space.set_propagation_phase(gtk::PropagationPhase::Bubble);
        space.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("space"),
            Some(gtk::NamedAction::new("app.play-pause")),
        ));
        self.window.add_controller(space);

        // The drawer and its tabs.
        let weak = Rc::downgrade(self);
        self.drawer_button.connect_toggled(move |button| {
            if let Some(mini) = weak.upgrade() {
                mini.drawer.set_reveal_child(button.is_active());
                mini.save_drawer();
            }
        });
        for tab in [&self.queue_tab, &self.lyrics_tab] {
            let weak = Rc::downgrade(self);
            tab.connect_toggled(move |tab| {
                if tab.is_active()
                    && let Some(mini) = weak.upgrade()
                {
                    mini.show_tab();
                    mini.save_drawer();
                }
            });
        }
        let reduced = crate::style::current().reduced_motion;
        self.drawer
            .set_transition_duration(if reduced { 0 } else { 150 });

        // The queue: a double click or Enter plays a row, Delete removes it.
        let backend = self.backend.clone();
        self.queue.connect_row_activated(move |_, row| {
            if let Ok(entry) = row.widget_name().parse::<u64>() {
                backend.send(PlayerRequest::PlayEntry(entry));
            }
        });
        let keys = gtk::EventControllerKey::new();
        let backend = self.backend.clone();
        keys.connect_key_pressed(move |controller, key, _, _| {
            let Some(list) = controller.widget().and_downcast::<gtk::ListBox>() else {
                return glib::Propagation::Proceed;
            };
            let entry = list
                .selected_row()
                .and_then(|row| row.widget_name().parse::<u64>().ok());
            match (key, entry) {
                (gdk::Key::Delete | gdk::Key::KP_Delete, Some(entry)) => {
                    backend.send(PlayerRequest::RemoveEntries(vec![entry]));
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.queue.add_controller(keys);

        let weak = Rc::downgrade(self);
        playback.connect(move |view, change| {
            if let Some(mini) = weak.upgrade() {
                mini.update(view, change);
            }
        });
        let weak = Rc::downgrade(self);
        self.lyrics.connect(move |model, change| {
            let Some(mini) = weak.upgrade() else { return };
            match change {
                LyricsChange::Lyrics => mini.show_lyrics(),
                LyricsChange::Line => mini.lines.set_line(model.line()),
            }
        });
        self.show_lyrics();
    }

    /// The window.
    pub(crate) fn window(&self) -> &gtk::Window {
        &self.window
    }

    /// Whether the window is shown.
    pub(crate) fn is_shown(&self) -> bool {
        self.window.is_visible()
    }

    /// Shows the mini player.
    pub(crate) fn present(&self) {
        self.shown.set(true);
        self.window.present();
    }

    /// Hides the mini player.
    pub(crate) fn hide(&self) {
        self.window.set_visible(false);
    }

    /// Hides the mini player and shows the main window.
    pub(crate) fn back_to_full(&self) {
        self.hide();
        if let Some(main) = self.main.upgrade() {
            main.present();
        }
    }

    /// After the window was hidden: closing it with the main window hidden
    /// too is closing the program's window.
    fn hidden(&self) {
        self.shown.set(false);
        if let Some(main) = self.main.upgrade() {
            main.mini_player_hidden();
        }
    }

    fn restore_drawer(&self) {
        let state = self.backend.ui_state().load().mini_player;
        if state.tab == "lyrics" {
            self.lyrics_tab.set_active(true);
        } else {
            self.queue_tab.set_active(true);
        }
        self.drawer_button.set_active(state.drawer);
        self.drawer.set_reveal_child(state.drawer);
        self.show_tab();
    }

    fn save_drawer(&self) {
        let store = self.backend.ui_state();
        let mut state = store.load();
        state.mini_player.drawer = self.drawer_button.is_active();
        state.mini_player.tab = if self.lyrics_tab.is_active() {
            "lyrics".into()
        } else {
            "queue".into()
        };
        if let Err(error) = store.save(&state) {
            log::warn!("cannot save the window state: {error}");
        }
    }

    fn show_tab(&self) {
        if self.lyrics_tab.is_active() {
            let lyrics = self.lyrics.lyrics();
            self.pages.set_visible_child_name(if lyrics.is_some() {
                "lyrics"
            } else {
                "no-lyrics"
            });
            self.note.set_text(&match lyrics.map(|l| l.synced) {
                Some(true) => tr("Synced"),
                Some(false) => tr("Not synced"),
                None => String::new(),
            });
        } else {
            self.pages.set_visible_child_name("queue");
            self.note.set_text(&self.queue_note.borrow());
        }
    }

    fn show_lyrics(&self) {
        let lyrics = self.lyrics.lyrics();
        self.lines.set_lyrics(lyrics.as_deref(), self.lyrics.line());
        self.show_tab();
    }

    fn update(&self, view: &PlayerView, change: Change) {
        match change {
            Change::Position => self.show_times(view),
            Change::State | Change::Track => {
                let playing = view.state == PlaybackState::Playing;
                self.play.set_icon_name(if playing {
                    "media-playback-pause-symbolic"
                } else {
                    "media-playback-start-symbolic"
                });
                let name = if playing { tr("Pause") } else { tr("Play") };
                self.play.set_tooltip_text(Some(&name));
                self.play
                    .update_property(&[gtk::accessible::Property::Label(&name)]);
                let has_track = view.now.is_some();
                for button in [&self.play, &self.previous, &self.next] {
                    button.set_sensitive(has_track);
                }
                match &view.now {
                    Some(now) => {
                        self.title.set_text(&now.track.title);
                        self.artist.set_text(&now.track.artist);
                        self.covers
                            .show(&self.cover, now.cover, now.cover_seed, COVER);
                    }
                    None => {
                        self.title.set_text(&tr("Nothing playing"));
                        self.artist.set_text("");
                        self.cover.show(0, None);
                    }
                }
                self.show_times(view);
                if change == Change::Track {
                    self.show_queue(view);
                }
            }
            Change::Queue => self.show_queue(view),
            Change::Output | Change::Signal => {
                let output = &view.output;
                self.lamp.set_state(output.signal);
                let lost = output.signal == SignalState::NotConnected;
                let text = match (output.sample_rate, output.bits) {
                    _ if lost => String::new(),
                    (Some(rate), Some(bits)) => format!("{} / {bits}", format::khz(rate)),
                    (Some(rate), None) => format::khz(rate),
                    _ => String::new(),
                };
                self.format.set_text(&text);
                for class in ["idle", "bit-perfect", "processed"] {
                    self.format.remove_css_class(class);
                }
                self.format.add_css_class(match output.signal {
                    SignalState::BitPerfect => "bit-perfect",
                    SignalState::Processed { .. } => "processed",
                    _ => "idle",
                });
            }
            _ => {}
        }
    }

    fn show_times(&self, view: &PlayerView) {
        match (view.now.is_some(), view.length) {
            (true, Some(length)) => {
                self.elapsed.set_text(&format::duration(view.elapsed));
                self.remaining
                    .set_text(&format::remaining(view.elapsed, length));
            }
            (true, None) => {
                self.elapsed.set_text(&format::duration(view.elapsed));
                self.remaining.set_text("-:--");
            }
            _ => {
                self.elapsed.set_text("-:--");
                self.remaining.set_text("-:--");
            }
        }
        self.seek
            .set_position(view.elapsed, view.now.as_ref().and(view.length));
    }

    /// The queue as in the panel, without the history.
    fn show_queue(&self, view: &PlayerView) {
        crate::widgets::remove_rows(&self.queue);
        let queue = &view.queue;
        if let Some(current) = queue.current.and_then(|i| queue.items.get(i)) {
            self.queue.append(&queue_row(current, true));
        }
        let upcoming_from = queue.current.map_or(0, |i| i + 1);
        let upcoming = queue.items.get(upcoming_from..).unwrap_or_default();
        let (queued, context): (Vec<&TrackRow>, Vec<&TrackRow>) =
            upcoming.iter().partition(|t| !t.from_context);
        if !queued.is_empty() {
            self.queue.append(&section(&tr("Next in queue")));
            for track in queued {
                self.queue.append(&queue_row(track, false));
            }
        }
        if !context.is_empty() {
            let heading = match &queue.context {
                Some(context) => fill(&tr("Continuing from {}"), &[context]),
                None => tr("Continuing"),
            };
            self.queue.append(&section(&heading));
            for track in context {
                self.queue.append(&queue_row(track, false));
            }
        }
        let total: Duration = upcoming.iter().filter_map(|t| t.duration).sum();
        let note = if queue.items.is_empty() {
            String::new()
        } else {
            fill(
                &ntr("{} upcoming · {}", "{} upcoming · {}", upcoming.len()),
                &[&format::count(upcoming.len()), &format::minutes(total)],
            )
        };
        self.queue_note.replace(note);
        if self.queue_tab.is_active() {
            self.note.set_text(&self.queue_note.borrow());
        }
    }

    /// The texts of the window, for tests: title, artist, the drawer's note
    /// and the queue's rows.
    pub(crate) fn texts(&self) -> (String, String, String, Vec<String>) {
        let mut rows = Vec::new();
        let mut child = self.queue.first_child();
        while let Some(row) = child {
            child = row.next_sibling();
            let mut texts = Vec::new();
            collect_labels(&row, &mut texts);
            rows.push(texts.join(" "));
        }
        (
            self.title.text().into(),
            self.artist.text().into(),
            self.note.text().into(),
            rows,
        )
    }

    /// Opens or closes the drawer, at `tab` (`queue` or `lyrics`).
    pub(crate) fn set_drawer(&self, open: bool, tab: &str) {
        if tab == "lyrics" {
            self.lyrics_tab.set_active(true);
        } else {
            self.queue_tab.set_active(true);
        }
        self.drawer_button.set_active(open);
    }
}

fn collect_labels(widget: &impl IsA<gtk::Widget>, texts: &mut Vec<String>) {
    let widget = widget.upcast_ref::<gtk::Widget>();
    if let Some(label) = widget.downcast_ref::<gtk::Label>()
        && !label.text().is_empty()
    {
        texts.push(label.text().into());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        collect_labels(&c, texts);
        child = c.next_sibling();
    }
}

fn transport_button(icon: &str, name: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("hk-transport");
    button.add_css_class("hk-mini-transport");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some(name));
    button.update_property(&[gtk::accessible::Property::Label(name)]);
    button
}

fn section(text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("section");
    row.set_activatable(false);
    row.set_selectable(false);
    row.set_focusable(false);
    row.set_child(Some(&label(text, &["hk-micro-label"])));
    row.update_property(&[gtk::accessible::Property::Label(text)]);
    row
}

fn queue_row(track: &TrackRow, current: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    if let Some(entry) = track.entry {
        row.set_widget_name(&entry.to_string());
    }
    if current {
        row.add_css_class("current");
    }
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let text = if track.from_context || track.artist.is_empty() {
        track.title.clone()
    } else {
        fill(&tr("{} · {}"), &[&track.title, &track.artist])
    };
    if current {
        // The playing row, as in the queue panel: the indicator in the lead
        // box, accent text.
        row.add_css_class("hk-playing");
        let lead = PlayingIndicator::default();
        lead.add_css_class("hk-queue-lead");
        line.append(&lead);
    } else {
        let lead = gtk::Label::new(Some(
            &track.number.map(|n| n.to_string()).unwrap_or_default(),
        ));
        lead.add_css_class("hk-queue-lead");
        lead.set_xalign(0.5);
        line.append(&lead);
    }
    let title = label(&text, &[]);
    title.set_hexpand(true);
    line.append(&title);
    let length = label(
        &track.duration.map(format::duration).unwrap_or_default(),
        &["hk-mini-length", "hk-numeric"],
    );
    length.set_xalign(1.0);
    line.append(&length);
    row.set_child(Some(&line));
    row.update_property(&[gtk::accessible::Property::Label(&text)]);
    row
}
