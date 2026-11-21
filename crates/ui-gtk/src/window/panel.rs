// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The right panel: tabs Queue, Lyrics and Signal path. The Queue tab shows the
//! playing track on top, then the history, the current track and what follows;
//! the Signal path tab is in [`signal_path`].

pub(crate) mod lyrics;
pub(crate) mod queue_end;
mod signal_path;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{CompositeTemplate, glib};

use crate::backend::{Backend, PlayerRequest, TrackRow};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::playback::{Change, Playback, PlayerView};
use crate::widgets::now_links::NowLinks;
use crate::widgets::{CoverArt, PlayingIndicator, RatingStars, TitleLine, label};

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/github/sorrelfox/Helikodu/ui/panel.ui")]
    pub(crate) struct Panel {
        #[template_child]
        pub(super) tabs: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) queue_tab: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(super) lyrics_tab: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(super) signal_tab: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(super) hide_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) controls: TemplateChild<gtk::WindowControls>,
        #[template_child]
        pub(super) pages: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) now_playing: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) cover: TemplateChild<CoverArt>,
        #[template_child]
        pub(super) title: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) artist: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) album: TemplateChild<gtk::Label>,
        /// The artists and the album as links, in place of the two labels.
        pub(super) links: RefCell<Option<(NowLinks, NowLinks)>>,
        #[template_child]
        pub(super) rating: TemplateChild<RatingStars>,
        #[template_child]
        pub(super) format: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) queue_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub(super) summary: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) clear_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) signal_page: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) lyrics_page: TemplateChild<gtk::Box>,
        pub(super) lyrics: RefCell<Option<Rc<lyrics::LyricsPage>>>,
        pub(super) covers: RefCell<Option<Rc<CoverCache>>>,
        pub(super) signal: RefCell<Option<Rc<signal_path::SignalPathPage>>>,
        pub(super) backend: RefCell<Option<Backend>>,
        pub(super) playback: RefCell<std::rc::Weak<Playback>>,
        /// Whether endless play or the Auto-DJ had nothing more to add when
        /// the queue was last shown, so its toast shows once.
        pub(super) nothing_to_add: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Panel {
        const NAME: &'static str = "HkPanel";
        type Type = super::Panel;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            CoverArt::ensure_type();
            RatingStars::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Panel {
        fn constructed(&self) {
            self.parent_constructed();
            self.cover.set_shape(104, 6.0);
            let pages = self.pages.get();
            for (tab, page) in [
                (self.queue_tab.get(), "queue"),
                (self.lyrics_tab.get(), "lyrics"),
                (self.signal_tab.get(), "signal"),
            ] {
                let pages = pages.clone();
                tab.connect_toggled(move |tab| {
                    if tab.is_active() {
                        pages.set_visible_child_name(page);
                    }
                });
            }
        }
    }

    impl WidgetImpl for Panel {}
    impl BoxImpl for Panel {}
}

glib::wrapper! {
    /// The queue panel.
    pub(crate) struct Panel(ObjectSubclass<imp::Panel>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for Panel {
    fn default() -> Self {
        glib::Object::new()
    }
}

fn section(text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("section");
    row.set_activatable(false);
    row.set_focusable(false);
    row.set_child(Some(&label(text, &["hk-section-label"])));
    row.update_property(&[gtk::accessible::Property::Label(text)]);
    row
}

fn track_row(track: &TrackRow, classes: &[&str], added: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    if let Some(entry) = track.entry {
        // The entry, for activation.
        row.set_widget_name(&entry.to_string());
    }
    for class in classes {
        row.add_css_class(class);
    }
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    if classes.contains(&"current") {
        // The playing row: the indicator in the lead box, accent text.
        row.add_css_class("hk-playing");
        let lead = PlayingIndicator::default();
        lead.add_css_class("hk-queue-lead");
        line.append(&lead);
    } else {
        // Rows Helikodu added come from anywhere: no track number.
        let number = track.number.filter(|_| !added);
        let lead = gtk::Label::new(Some(&number.map(|n| n.to_string()).unwrap_or_default()));
        lead.add_css_class("hk-queue-lead");
        lead.set_xalign(0.5);
        line.append(&lead);
    }
    let text = if track.from_context || track.artist.is_empty() {
        track.title.clone()
    } else {
        fill(&tr("{} · {}"), &[&track.title, &track.artist])
    };
    let title = TitleLine::new(&text, &[]);
    title.set_copies(track.key, track.copies);
    line.append(&title);
    if added {
        line.append(&queue_end::spark());
        row.add_css_class("added");
        row.update_property(&[gtk::accessible::Property::Description(&tr(
            "Added by Helikodu",
        ))]);
    }
    let length = label(
        &track.duration.map(format::duration).unwrap_or_default(),
        &["hk-t3", "hk-secondary", "hk-numeric"],
    );
    length.set_xalign(1.0);
    line.append(&length);
    row.set_child(Some(&line));
    row.update_property(&[gtk::accessible::Property::Label(&text)]);
    row
}

impl Panel {
    /// Follows `playback`; the queue's buttons and rows send to `backend`.
    pub(crate) fn bind(&self, backend: &Backend, playback: &Rc<Playback>, covers: &Rc<CoverCache>) {
        let imp = self.imp();
        imp.covers.replace(Some(covers.clone()));
        imp.backend.replace(Some(backend.clone()));
        imp.playback.replace(Rc::downgrade(playback));
        let artists = NowLinks::new(&["hk-t2"]);
        artists.set_backend(backend);
        imp.artist.set_visible(false);
        if let Some(parent) = imp.artist.parent().and_downcast::<gtk::Box>() {
            parent.insert_child_after(artists.widget(), Some(&imp.artist.get()));
            let album = NowLinks::new(&["hk-t3", "hk-secondary"]);
            album.widget().set_visible(false);
            parent.insert_child_after(album.widget(), Some(&imp.album.get()));
            imp.links.replace(Some((artists, album)));
        }
        let b = backend.clone();
        imp.clear_button
            .connect_clicked(move |_| b.send(PlayerRequest::ClearQueue));
        let b = backend.clone();
        imp.list.connect_row_activated(move |_, row| {
            if let Ok(entry) = row.widget_name().parse::<u64>() {
                b.send(PlayerRequest::PlayEntry(entry));
            }
        });
        let weak = self.downgrade();
        playback.connect(move |view, change| {
            if let Some(panel) = weak.upgrade()
                && matches!(change, Change::Queue | Change::Track | Change::Modes)
            {
                panel.update(view);
            }
        });
        // A stored rating of the playing track shows at once.
        let weak = self.downgrade();
        let source = Rc::downgrade(playback);
        playback.connect_event(move |update| {
            if let (crate::backend::Update::RatingChanged { .. }, Some(panel), Some(source)) =
                (update, weak.upgrade(), source.upgrade())
                && let Some(now) = source.view().now
            {
                panel.imp().rating.set_rating(now.track.rating);
            }
        });
        let signal = signal_path::SignalPathPage::new(playback);
        imp.signal_page.append(signal.widget());
        imp.signal.replace(Some(signal));
    }

    /// Fills the Lyrics tab, following `lyrics`; its lines seek with `backend`.
    pub(crate) fn bind_lyrics(
        &self,
        backend: &Backend,
        playback: &Rc<Playback>,
        model: &Rc<crate::models::lyrics::LyricsModel>,
        covers: &Rc<CoverCache>,
    ) {
        let imp = self.imp();
        let b = backend.clone();
        let page = lyrics::LyricsPage::new(playback, model, covers, move |to| {
            b.send(PlayerRequest::Seek(to));
        });
        imp.lyrics_page.append(page.widget());
        imp.lyrics.replace(Some(page));
    }

    /// The Lyrics tab, once bound.
    pub(crate) fn lyrics_page(&self) -> Option<Rc<lyrics::LyricsPage>> {
        self.imp().lyrics.borrow().clone()
    }

    /// Shows the Lyrics tab.
    pub(crate) fn show_lyrics(&self) {
        self.imp().lyrics_tab.set_active(true);
    }

    /// Shows the Signal path tab.
    pub(crate) fn show_signal_path(&self) {
        self.imp().signal_tab.set_active(true);
    }

    /// What the Signal path tab shows, in words.
    pub(crate) fn signal_path_texts(&self) -> Vec<String> {
        self.imp()
            .signal
            .borrow()
            .as_ref()
            .map(|page| page.texts())
            .unwrap_or_default()
    }

    /// The window controls of the end side, shown in the integrated style.
    pub(crate) fn set_integrated(&self, integrated: bool) {
        let imp = self.imp();
        imp.controls.set_visible(integrated);
        imp.hide_button.set_visible(!integrated);
    }

    /// Shows the toast about nothing more to add once, when it happens while
    /// the queue is not in view.
    fn note_nothing_to_add(
        &self,
        view: &PlayerView,
        settings: &helikodu_model::settings::PlaybackSettings,
    ) {
        let imp = self.imp();
        let nothing = view.queue.nothing_to_add
            && matches!(
                view.modes.repeat,
                crate::backend::RepeatMode::Endless | crate::backend::RepeatMode::AutoDj
            );
        if nothing == imp.nothing_to_add.replace(nothing) || !nothing {
            return;
        }
        let revealed = self
            .parent()
            .and_downcast::<gtk::Revealer>()
            .is_none_or(|r| r.reveals_child());
        if self.is_mapped() && revealed && imp.queue_tab.is_active() {
            return;
        }
        let Some(window) = self.root().and_downcast::<crate::window::MainWindow>() else {
            return;
        };
        let weak = window.downgrade();
        window.toast_marked(
            &queue_end::toast_text(view.modes.repeat, settings),
            crate::window::ToastLead::Warning,
            Some(&tr("Settings…")),
            true,
            move || {
                if let Some(window) = weak.upgrade() {
                    crate::dialogs::settings::open_at(
                        &window,
                        crate::dialogs::settings::Page::Playback,
                        queue_end::SETTINGS_ROW,
                    );
                }
            },
        );
    }

    /// Shows the queue again, after a change of the settings it words.
    pub(crate) fn refresh_queue(&self) {
        let playback = self.imp().playback.borrow().upgrade();
        if let Some(playback) = playback {
            self.update(&playback.view());
        }
    }

    /// The queue list, whose rows are named by their entry.
    pub(crate) fn queue_list(&self) -> gtk::ListBox {
        self.imp().list.get()
    }

    /// The tab strip, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.imp().queue_tab.get().upcast()
    }

    fn update(&self, view: &PlayerView) {
        let imp = self.imp();
        match &view.now {
            Some(now) => {
                imp.now_playing.set_visible(true);
                match imp.covers.borrow().as_ref() {
                    Some(covers) => covers.show(&imp.cover, now.cover, now.cover_seed, 104),
                    None => imp.cover.show(now.cover_seed, None),
                }
                imp.title.set_text(&now.track.title);
                imp.artist.set_text(&now.track.artist);
                imp.album.set_text(&match now.year {
                    Some(year) => fill(&tr("{} · {}"), &[&now.album_title, &year.to_string()]),
                    None => now.album_title.clone(),
                });
                if let Some((artists, album)) = imp.links.borrow().as_ref() {
                    artists.set(&now.track.artist, None, &[]);
                    let year: Vec<String> = now.year.iter().map(|y| y.to_string()).collect();
                    let linked = now.track.album.0 != 0;
                    if linked {
                        album.set("", Some((&now.album_title, now.track.album)), &year);
                    }
                    imp.album.set_visible(!linked);
                    album.widget().set_visible(linked);
                }
                imp.rating.set_rating(now.track.rating);
                let f = &now.format;
                let mut parts: Vec<String> = Vec::new();
                parts.extend(f.codec.clone());
                if let Some(rate) = f.sample_rate {
                    parts.push(fill(&tr("{} kHz"), &[&format::khz(rate)]));
                }
                if let Some(bits) = f.bits {
                    parts.push(fill(&tr("{}-bit"), &[&bits.to_string()]));
                } else if let Some(kbps) = f.bitrate_kbps {
                    parts.push(fill(&tr("{} kbit/s"), &[&kbps.to_string()]));
                }
                imp.format.set_text(&parts.join(" · "));
            }
            None => imp.now_playing.set_visible(false),
        }

        crate::widgets::remove_rows(&imp.list);
        let queue = &view.queue;
        let settings = imp
            .backend
            .borrow()
            .as_ref()
            .map(|b| b.services().settings().playback.clone())
            .unwrap_or_default();
        let end = queue_end::words(view.modes.repeat, queue, &settings);
        self.note_nothing_to_add(view, &settings);
        if queue.items.is_empty() && end.is_none() {
            imp.queue_stack.set_visible_child_name("empty");
            imp.summary.set_text("");
            return;
        }
        imp.queue_stack.set_visible_child_name("list");
        let added: HashSet<u64> = queue.added.iter().copied().collect();
        let is_added = |t: &TrackRow| t.entry.is_some_and(|e| added.contains(&e));
        let current = queue.current;
        let split = current.unwrap_or(0).min(queue.items.len());
        if split > 0 {
            imp.list.append(&section(&tr("History")));
            for track in &queue.items[..split] {
                imp.list
                    .append(&track_row(track, &["history"], is_added(track)));
            }
        }
        let upcoming_from = match current.filter(|&i| i < queue.items.len()) {
            Some(index) => {
                imp.list.append(&section(&tr("Now playing")));
                let track = &queue.items[index];
                imp.list
                    .append(&track_row(track, &["current"], is_added(track)));
                index + 1
            }
            None => 0,
        };
        let upcoming = &queue.items[upcoming_from.min(queue.items.len())..];
        let (helikodu, own): (Vec<&TrackRow>, Vec<&TrackRow>) =
            upcoming.iter().partition(|t| is_added(t));
        let (queued, context): (Vec<&TrackRow>, Vec<&TrackRow>) =
            own.into_iter().partition(|t| !t.from_context);
        if !queued.is_empty() {
            imp.list.append(&section(&tr("Next in queue")));
            for track in queued {
                imp.list.append(&track_row(track, &[], false));
            }
        }
        if !context.is_empty() {
            let heading = match &queue.context {
                Some(context) => fill(&tr("Continuing from {}"), &[context]),
                None => tr("Continuing"),
            };
            imp.list.append(&section(&heading));
            for track in context {
                imp.list.append(&track_row(track, &[], false));
            }
        }
        if !helikodu.is_empty() {
            imp.list.append(&section(&tr("Added by Helikodu")));
            for track in helikodu {
                imp.list.append(&track_row(track, &[], true));
            }
        }
        if let Some(end) = &end {
            let weak = self.downgrade();
            let change = move || {
                if let Some(window) = weak
                    .upgrade()
                    .and_then(|p| p.root())
                    .and_downcast::<crate::window::MainWindow>()
                {
                    crate::dialogs::settings::open_at(
                        &window,
                        crate::dialogs::settings::Page::Playback,
                        queue_end::SETTINGS_ROW,
                    );
                }
            };
            let backend = imp.backend.borrow().clone();
            let turn_off = move || {
                if let Some(backend) = &backend {
                    backend.send(PlayerRequest::SetRepeatMode(
                        crate::backend::RepeatMode::Off,
                    ));
                }
            };
            imp.list.append(&queue_end::row(end, change, turn_off));
        }
        imp.clear_button.set_sensitive(queue.items.len() > 1);
        let total: Duration = upcoming.iter().filter_map(|t| t.duration).sum();
        imp.summary.set_text(&fill(
            &ntr("{} upcoming · {}", "{} upcoming · {}", upcoming.len()),
            &[&format::count(upcoming.len()), &format::minutes(total)],
        ));
    }
}
