// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The player deck: elapsed time, the waveform seek bar and the remaining time;
//! below, the playing track, the transport, the volume and the output button.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{CompositeTemplate, glib};

use crate::backend::{Backend, CoverKey, PlaybackState, PlayerRequest, Repeat, SignalState};
use crate::format;
use crate::i18n::{fill, tr};
use crate::models::CoverCache;
use crate::playback::{Change, Playback, PlayerView};
use crate::widgets::{CoverArt, SignalLamp, WaveformSeekBar};

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/github/sorrelfox/Helikodu/ui/deck.ui")]
    pub(crate) struct Deck {
        #[template_child]
        pub(super) elapsed: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) seek_bar: TemplateChild<WaveformSeekBar>,
        #[template_child]
        pub(super) remaining_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) remaining: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) cover: TemplateChild<CoverArt>,
        #[template_child]
        pub(super) title: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) play: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) previous: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) next: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) shuffle: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) repeat: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) stop_mark: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) volume: TemplateChild<gtk::Scale>,
        #[template_child]
        pub(super) volume_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) output: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) lamp: TemplateChild<SignalLamp>,
        #[template_child]
        pub(super) device: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) signal: TemplateChild<gtk::Label>,
        /// Show the total length instead of the time left.
        pub(super) show_total: Cell<bool>,
        /// Set while the volume slider follows the player, so it does not echo.
        pub(super) updating: Cell<bool>,
        pub(super) covers: RefCell<Option<Rc<CoverCache>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Deck {
        const NAME: &'static str = "HkDeck";
        type Type = super::Deck;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            CoverArt::ensure_type();
            SignalLamp::ensure_type();
            WaveformSeekBar::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Deck {
        fn constructed(&self) {
            self.parent_constructed();
            self.cover.set_shape(40, 4.0);
            self.cover.add_css_class("small");
            self.obj().set_accessible_role(gtk::AccessibleRole::Group);
            self.obj()
                .update_property(&[gtk::accessible::Property::Label(&tr("Player"))]);
        }
    }

    impl WidgetImpl for Deck {}
    impl BoxImpl for Deck {}
}

glib::wrapper! {
    /// The player deck.
    pub(crate) struct Deck(ObjectSubclass<imp::Deck>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for Deck {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Deck {
    /// Connects the controls to `backend` and follows `playback`.
    pub(crate) fn bind(&self, backend: &Backend, playback: &Rc<Playback>, covers: &Rc<CoverCache>) {
        let imp = self.imp();
        imp.covers.replace(Some(covers.clone()));
        let b = backend.clone();
        let pb = Rc::downgrade(playback);
        imp.shuffle.connect_clicked(move |_| {
            if let Some(pb) = pb.upgrade() {
                b.send(PlayerRequest::SetShuffle(!pb.view().shuffle));
            }
        });
        let b = backend.clone();
        let pb = Rc::downgrade(playback);
        imp.repeat.connect_clicked(move |_| {
            if let Some(pb) = pb.upgrade() {
                b.send(PlayerRequest::SetRepeat(pb.view().repeat.next()));
            }
        });
        let b = backend.clone();
        imp.seek_bar
            .connect_seek(move |to| b.send(PlayerRequest::Seek(to)));
        let b = backend.clone();
        let weak = self.downgrade();
        imp.volume.connect_value_changed(move |scale| {
            let Some(deck) = weak.upgrade() else { return };
            deck.show_volume_icon(scale.value());
            if !deck.imp().updating.get() {
                b.send(PlayerRequest::SetVolume(scale.value()));
            }
        });
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let volume = imp.volume.get();
        scroll.connect_scroll(move |_, _, dy| {
            volume.set_value(volume.value() - dy * 0.05);
            glib::Propagation::Stop
        });
        imp.volume.add_controller(scroll);
        let weak = self.downgrade();
        let pb = Rc::downgrade(playback);
        imp.remaining_button.connect_clicked(move |_| {
            if let (Some(deck), Some(pb)) = (weak.upgrade(), pb.upgrade()) {
                let imp = deck.imp();
                imp.show_total.set(!imp.show_total.get());
                deck.show_times(&pb.view());
            }
        });
        let weak = self.downgrade();
        playback.connect(move |view, change| {
            if let Some(deck) = weak.upgrade() {
                deck.update(view, change);
            }
        });
    }

    fn show_volume_icon(&self, value: f64) {
        let icon = if value <= 0.0 {
            "audio-volume-muted-symbolic"
        } else if value < 0.34 {
            "audio-volume-low-symbolic"
        } else if value < 0.67 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.imp().volume_icon.set_icon_name(Some(icon));
    }

    fn show_times(&self, view: &PlayerView) {
        let imp = self.imp();
        match (view.now.is_some(), view.length) {
            (true, Some(length)) => {
                imp.elapsed.set_text(&format::duration(view.elapsed));
                let right = if imp.show_total.get() {
                    format::duration(length)
                } else {
                    format::remaining(view.elapsed, length)
                };
                imp.remaining.set_text(&right);
            }
            (true, None) => {
                imp.elapsed.set_text(&format::duration(view.elapsed));
                imp.remaining.set_text("-:--");
            }
            _ => {
                imp.elapsed.set_text("-:--");
                imp.remaining.set_text("-:--");
            }
        }
        imp.remaining_button
            .set_tooltip_text(Some(&if imp.show_total.get() {
                tr("Show the time left")
            } else {
                tr("Show the total length")
            }));
        imp.seek_bar
            .set_position(view.elapsed, view.now.as_ref().and(view.length));
    }

    fn update(&self, view: &PlayerView, change: Change) {
        let imp = self.imp();
        match change {
            Change::Position => self.show_times(view),
            Change::State | Change::Track => {
                let playing = view.state == PlaybackState::Playing;
                let (icon, name) = if playing {
                    ("media-playback-pause-symbolic", tr("Pause"))
                } else {
                    ("media-playback-start-symbolic", tr("Play"))
                };
                imp.play.set_icon_name(icon);
                imp.play.set_tooltip_text(Some(&name));
                imp.play
                    .update_property(&[gtk::accessible::Property::Label(&name)]);
                let has_track = view.now.is_some();
                imp.play.set_sensitive(has_track);
                imp.previous.set_sensitive(has_track);
                imp.next.set_sensitive(has_track);
                match &view.now {
                    Some(now) => {
                        imp.title.set_text(&now.track.title);
                        imp.subtitle.set_text(&fill(
                            &tr("{} · {}"),
                            &[&now.track.artist, &now.album_title],
                        ));
                        self.show_cover(now.cover, now.cover_seed);
                    }
                    None => {
                        imp.title.set_text(&tr("Nothing playing"));
                        imp.subtitle.set_text(&tr("Choose an album to play"));
                        self.show_cover(None, 0);
                    }
                }
                self.show_times(view);
            }
            Change::Volume => {
                imp.updating.set(true);
                imp.volume.set_value(view.volume);
                imp.updating.set(false);
                self.show_volume_icon(view.volume);
            }
            Change::Output => {
                let output = &view.output;
                imp.device.set_text(&output.device);
                imp.lamp.set_state(output.signal);
                let state = match output.signal {
                    SignalState::Idle => Some(tr("Idle")),
                    SignalState::BitPerfect => Some(tr("Bit-perfect")),
                    SignalState::Processed { dsp: true } => Some(tr("DSP active")),
                    SignalState::Processed { dsp: false } => Some(tr("Converted")),
                    SignalState::NotConnected => Some(tr("Not connected")),
                    SignalState::Unreported => None,
                };
                let format = match (output.sample_rate, output.bits) {
                    (Some(rate), Some(bits)) => Some(format!("{} / {bits}", format::khz(rate))),
                    (Some(rate), None) => Some(format::khz(rate)),
                    _ => None,
                };
                let text = match (state, format) {
                    (Some(s), Some(f)) => format!("{s} · {f}"),
                    (Some(s), None) => s,
                    (None, Some(f)) => f,
                    (None, None) => String::new(),
                };
                imp.signal.set_text(&text);
                for class in ["idle", "bit-perfect", "processed"] {
                    imp.signal.remove_css_class(class);
                }
                imp.signal.add_css_class(match output.signal {
                    SignalState::BitPerfect => "bit-perfect",
                    SignalState::Processed { .. } => "processed",
                    _ => "idle",
                });
                let name = fill(&tr("Output: {}, {}"), &[&output.device, &text]);
                imp.output
                    .update_property(&[gtk::accessible::Property::Label(&name)]);
            }
            Change::Modes => {
                let (name, active) = if view.shuffle {
                    (tr("Shuffle: on"), true)
                } else {
                    (tr("Shuffle: off"), false)
                };
                imp.shuffle.set_tooltip_text(Some(&name));
                imp.shuffle
                    .update_property(&[gtk::accessible::Property::Label(&name)]);
                if active {
                    imp.shuffle.add_css_class("active");
                } else {
                    imp.shuffle.remove_css_class("active");
                }
                let (icon, name) = match view.repeat {
                    Repeat::Off => ("media-playlist-repeat-symbolic", tr("Repeat: off")),
                    Repeat::All => ("media-playlist-repeat-symbolic", tr("Repeat: all")),
                    Repeat::One => (
                        "media-playlist-repeat-song-symbolic",
                        tr("Repeat: this track"),
                    ),
                };
                imp.repeat.set_icon_name(icon);
                imp.repeat.set_tooltip_text(Some(&name));
                imp.repeat
                    .update_property(&[gtk::accessible::Property::Label(&name)]);
                if view.repeat == Repeat::Off {
                    imp.repeat.remove_css_class("active");
                } else {
                    imp.repeat.add_css_class("active");
                }
                imp.stop_mark.set_visible(view.stop_after);
                let play_name = if view.stop_after {
                    tr("Play or pause; stops after this track")
                } else {
                    tr("Play or pause")
                };
                imp.play
                    .update_property(&[gtk::accessible::Property::Description(&play_name)]);
            }
            Change::Queue => {}
        }
    }

    fn show_cover(&self, cover: Option<CoverKey>, seed: u64) {
        let imp = self.imp();
        match imp.covers.borrow().as_ref() {
            Some(covers) => covers.show(&imp.cover, cover, seed, 40),
            None => imp.cover.show(seed, None),
        }
    }

    /// Moves the volume slider by `delta` (0 to 1 is the whole range).
    pub(crate) fn adjust_volume(&self, delta: f64) {
        let volume = &self.imp().volume;
        volume.set_value(volume.value() + delta);
    }

    /// The play button, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.imp().play.get().upcast()
    }
}
