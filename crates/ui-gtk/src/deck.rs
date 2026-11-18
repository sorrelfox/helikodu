// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The player deck: elapsed time, the waveform seek bar and the remaining time;
//! below, the playing track, the transport, the volume and the output button.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{CompositeTemplate, glib};

mod modes;

use crate::backend::{Backend, CoverKey, PlaybackState, PlayerRequest, SignalState};
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
        /// Now Playing's start group: "Next" and the next track.
        pub(super) next_box: RefCell<Option<(gtk::Box, gtk::Label)>>,
        /// The loop chip before the remaining time, and its text.
        pub(super) loop_chip: RefCell<Option<(gtk::Box, gtk::Label)>>,
        /// The sleep chip before the volume, and its time left.
        pub(super) sleep_chip: RefCell<Option<(gtk::Button, gtk::Label)>>,
        /// The sleep timer popover.
        pub(super) sleep_popover: RefCell<Option<Rc<crate::popovers::sleep_timer::SleepPopover>>>,
        /// The countdown of the sleep chip while a timer runs.
        pub(super) sleep_tick: RefCell<Option<glib::SourceId>>,
        /// The player's state, for the mode buttons' handlers.
        pub(super) playback: RefCell<std::rc::Weak<Playback>>,
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
        imp.playback.replace(Rc::downgrade(playback));
        self.setup_modes(backend, playback);
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
        self.setup_play_menu();
    }

    /// The play button's menu: right click, long press or the Menu key.
    fn setup_play_menu(&self) {
        let play = self.imp().play.get();
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_SECONDARY);
        let weak = self.downgrade();
        click.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(deck) = weak.upgrade() {
                deck.popup_play_menu();
            }
        });
        play.add_controller(click);
        let press = gtk::GestureLongPress::new();
        let weak = self.downgrade();
        press.connect_pressed(move |gesture, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(deck) = weak.upgrade() {
                deck.popup_play_menu();
            }
        });
        play.add_controller(press);
        let keys = gtk::EventControllerKey::new();
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            if crate::views::actions::is_menu_key(key, state)
                && let Some(deck) = weak.upgrade()
            {
                deck.popup_play_menu();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        play.add_controller(keys);
    }

    /// Opens the play button's menu above it: Stop, Stop after ▸, Sleep
    /// timer…, Jump to the playing track.
    pub(crate) fn popup_play_menu(&self) -> gtk::PopoverMenu {
        let menu = gtk::gio::Menu::new();
        let stop = gtk::gio::Menu::new();
        stop.append(Some(&tr("Stop")), Some("app.stop"));
        stop.append_submenu(
            Some(&tr("Stop after")),
            &crate::play_modes::stop_after_menu(),
        );
        stop.append(Some(&tr("Sleep timer…")), Some("win.sleep-timer"));
        menu.append_section(None, &stop);
        let jump = gtk::gio::Menu::new();
        jump.append(
            Some(&tr("Jump to the playing track")),
            Some("win.jump-to-playing"),
        );
        menu.append_section(None, &jump);
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.add_css_class("hk-play-menu");
        // On the deck, which disposes of it, pointing at the play button.
        popover.set_parent(self);
        let play = self.imp().play.get();
        if let Some(bounds) = play.compute_bounds(self) {
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                bounds.x() as i32,
                bounds.y() as i32,
                bounds.width() as i32,
                bounds.height() as i32,
            )));
        }
        popover.set_position(gtk::PositionType::Top);
        popover.set_has_arrow(false);
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        crate::views::actions::popup_when_built(&popover);
        popover
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
                imp.play.set_icon_name(if playing {
                    "media-playback-pause-symbolic"
                } else {
                    "media-playback-start-symbolic"
                });
                self.show_play_name(view);
                // The sleep chip counts down only while playing.
                if view.sleep.is_some() {
                    self.tick_sleep(view);
                    self.keep_sleep_ticking(view);
                }
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
                        imp.subtitle
                            .set_text(&tr("Pick an album, or drop files on the window"));
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
            Change::Output | Change::Signal => {
                let output = &view.output;
                // The backend's description of the device the path ends in,
                // when it gave one, rather than its stable name.
                let device = view
                    .signal
                    .as_ref()
                    .and_then(|p| p.device.description.clone())
                    .unwrap_or_else(|| output.device.clone());
                imp.device.set_text(&device);
                imp.lamp.set_state(output.signal);
                let state = crate::widgets::signal_path_list::state_label(
                    output.signal,
                    view.signal.as_deref(),
                );
                let lost = output.signal == SignalState::NotConnected;
                if lost {
                    imp.device.add_css_class("hk-t2");
                } else {
                    imp.device.remove_css_class("hk-t2");
                }
                let format = match (output.sample_rate, output.bits) {
                    _ if lost => None,
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
                let name = fill(&tr("Output: {}, {}"), &[&device, &text]);
                imp.output
                    .update_property(&[gtk::accessible::Property::Label(&name)]);
            }
            Change::Modes => self.show_modes(view),
            Change::Loop => self.show_loop(view),
            Change::Sleep => self.show_sleep(view),
            Change::Queue | Change::Stats | Change::Cursor => {}
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

    /// The output button's device and signal text.
    pub(crate) fn output_text(&self) -> (String, String) {
        let imp = self.imp();
        (imp.device.text().into(), imp.signal.text().into())
    }

    /// The play button, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.imp().play.get().upcast()
    }

    /// Calls `activated` when the cover is clicked (it opens Now Playing).
    pub(crate) fn connect_cover_activated(&self, activated: impl Fn() + 'static) {
        let cover = self.imp().cover.get();
        cover.set_cursor_from_name(Some("pointer"));
        cover.set_tooltip_text(Some(&tr("Show Now Playing")));
        let click = gtk::GestureClick::new();
        click.connect_released(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            activated();
        });
        cover.add_controller(click);
    }

    /// In Now Playing the start group shows the next track (`Some`, as
    /// "Title · duration", empty when none follows) instead of the playing one;
    /// `None` shows the playing one again.
    pub(crate) fn show_next(&self, next: Option<&str>) {
        let imp = self.imp();
        let Some(group) = imp.cover.parent().and_downcast::<gtk::Box>() else {
            return;
        };
        if imp.next_box.borrow().is_none() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            line.set_valign(gtk::Align::Center);
            line.set_visible(false);
            let micro = gtk::Label::new(Some(&tr("Next")));
            micro.add_css_class("hk-micro-label");
            micro.add_css_class("hk-deck-next-label");
            line.append(&micro);
            let title = gtk::Label::new(None);
            title.add_css_class("hk-deck-subtitle");
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.set_max_width_chars(40);
            title.set_xalign(0.0);
            line.append(&title);
            group.append(&line);
            imp.next_box.replace(Some((line, title)));
        }
        let shown = next.is_some();
        let mut child = group.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            let is_next = imp
                .next_box
                .borrow()
                .as_ref()
                .is_some_and(|(b, _)| widget == *b);
            widget.set_visible(if is_next { shown } else { !shown });
        }
        if let (Some((line, title)), Some(text)) = (imp.next_box.borrow().as_ref(), next) {
            title.set_text(text);
            line.update_property(&[gtk::accessible::Property::Label(&fill(
                &tr("Next: {}"),
                &[text],
            ))]);
        }
    }
}
