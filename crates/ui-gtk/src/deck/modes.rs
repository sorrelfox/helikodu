// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The deck's play modes: the shuffle and repeat buttons (a click switches
//! between off and the mode used last; right click or long press opens the
//! mode's menu), the stop-after mark's name, the A-B loop on the seek bar with
//! its chip, and the sleep chip with its popover.

use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::Deck;
use crate::backend::{Backend, PlaybackState, PlayerRequest, RepeatMode, ShuffleMode, StopAfter};
use crate::i18n::{fill, tr};
use crate::play_modes;
use crate::playback::{Playback, PlayerView};
use crate::popovers::sleep_timer::SleepPopover;
use crate::widgets::label;

/// The size of every icon in the deck's buttons, whatever the icon theme
/// draws: the bundled mode icons and the theme's transport icons alike.
const ICON: i32 = 16;

/// Calls `open` on a right click or a long press of `button`.
fn on_secondary(button: &gtk::Button, open: impl Fn() + Clone + 'static) {
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    let o = open.clone();
    click.connect_pressed(move |gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        o();
    });
    button.add_controller(click);
    let press = gtk::GestureLongPress::new();
    press.connect_pressed(move |gesture, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        open();
    });
    button.add_controller(press);
}

/// The bounds of `widget` in `within`, as a rectangle to point at.
fn area(
    widget: &impl IsA<gtk::Widget>,
    within: &impl IsA<gtk::Widget>,
) -> Option<gtk::gdk::Rectangle> {
    let bounds = widget.compute_bounds(within)?;
    Some(gtk::gdk::Rectangle::new(
        bounds.x() as i32,
        bounds.y() as i32,
        bounds.width() as i32,
        bounds.height() as i32,
    ))
}

impl Deck {
    /// Sets up the mode buttons, the loop and the sleep timer.
    pub(super) fn setup_modes(&self, backend: &Backend, playback: &Rc<Playback>) {
        let imp = self.imp();
        // The mode buttons carry bundled icons (deck.ui), so that no theme
        // draws one smaller than the other (Breeze's shuffle is a small status
        // icon), and every icon of the deck has one size.
        imp.volume_icon.set_pixel_size(ICON);
        for button in [
            &imp.shuffle,
            &imp.repeat,
            &imp.previous,
            &imp.next,
            &imp.play,
        ] {
            button.add_css_class("hk-deck-icon");
        }

        let b = backend.clone();
        let pb = Rc::downgrade(playback);
        imp.shuffle.connect_clicked(move |_| {
            if let Some(pb) = pb.upgrade() {
                let now = pb.view().modes.shuffle;
                b.send(PlayerRequest::SetShuffleMode(play_modes::toggled_shuffle(
                    now,
                )));
            }
        });
        let b = backend.clone();
        let pb = Rc::downgrade(playback);
        imp.repeat.connect_clicked(move |_| {
            if let Some(pb) = pb.upgrade() {
                let now = pb.view().modes.repeat;
                b.send(PlayerRequest::SetRepeatMode(play_modes::toggled_repeat(
                    now,
                )));
            }
        });
        let weak = self.downgrade();
        on_secondary(&imp.shuffle, move || {
            if let Some(deck) = weak.upgrade() {
                let anchor = deck.imp().shuffle.get();
                deck.popup_menu(&anchor, &play_modes::shuffle_menu());
            }
        });
        let weak = self.downgrade();
        on_secondary(&imp.repeat, move || {
            if let Some(deck) = weak.upgrade() {
                let anchor = deck.imp().repeat.get();
                deck.popup_menu(&anchor, &play_modes::repeat_menu());
            }
        });

        // The loop's flags on the seek bar, and its chip before the time left.
        let b = backend.clone();
        imp.seek_bar.connect_loop_moved(move |start, end| {
            b.send(PlayerRequest::SetLoop(Some((start, end))));
        });
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        chip.set_widget_name("loop-chip");
        chip.add_css_class("hk-deck-chip");
        chip.add_css_class("hk-loop-chip");
        chip.set_valign(gtk::Align::Center);
        chip.set_visible(false);
        let icon = gtk::Image::from_icon_name("helikodu-repeat-symbolic");
        icon.set_pixel_size(12);
        icon.add_css_class("hk-chip-icon");
        chip.append(&icon);
        let text = label("", &["hk-mono", "hk-loop-text"]);
        chip.append(&text);
        let clear = gtk::Button::from_icon_name("window-close-symbolic");
        clear.add_css_class("hk-chip-clear");
        clear.set_action_name(Some("app.loop-clear"));
        clear.set_tooltip_text(Some(&tr("Clear the loop")));
        clear.update_property(&[gtk::accessible::Property::Label(&tr("Clear the loop"))]);
        chip.append(&clear);
        if let Some(strip) = imp.remaining_button.parent().and_downcast::<gtk::Box>() {
            strip.insert_child_after(&chip, Some(&imp.seek_bar.get()));
        }
        imp.loop_chip.replace(Some((chip, text)));

        // The sleep chip before the volume, and its popover.
        let sleep = gtk::Button::new();
        sleep.set_widget_name("sleep-chip");
        sleep.add_css_class("hk-deck-chip");
        sleep.add_css_class("hk-sleep-chip");
        sleep.set_valign(gtk::Align::Center);
        sleep.set_visible(false);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        let moon = gtk::Image::from_icon_name("helikodu-moon-symbolic");
        moon.set_pixel_size(14);
        moon.add_css_class("hk-chip-icon");
        line.append(&moon);
        let left = label("", &["hk-mono", "hk-sleep-chip-time"]);
        line.append(&left);
        sleep.set_child(Some(&line));
        sleep.set_tooltip_text(Some(&tr("Sleep timer")));
        let weak = self.downgrade();
        sleep.connect_clicked(move |_| {
            if let Some(deck) = weak.upgrade() {
                deck.open_sleep_timer();
            }
        });
        if let Some(group) = imp.volume_icon.parent().and_downcast::<gtk::Box>() {
            sleep.set_margin_end(8);
            group.prepend(&sleep);
        }
        // On the deck, not in the end group, whose width it would add to.
        let popover = SleepPopover::new(self, backend, playback);
        imp.sleep_popover.replace(Some(popover));
        let weak = self.downgrade();
        sleep.connect_map(move |_| {
            if let Some(deck) = weak.upgrade() {
                deck.refresh_sleep();
            }
        });
        if let Some(popover) = imp.sleep_popover.borrow().as_ref() {
            let weak = self.downgrade();
            popover.popover().connect_visible_notify(move |_| {
                if let Some(deck) = weak.upgrade() {
                    deck.refresh_sleep();
                }
            });
        }
        imp.sleep_chip.replace(Some((sleep, left)));
    }

    /// Opens `model` as a menu above `anchor`.
    fn popup_menu(&self, anchor: &impl IsA<gtk::Widget>, model: &gtk::gio::Menu) {
        let popover = gtk::PopoverMenu::from_model(Some(model));
        crate::widgets::menu_names::name_menus(&popover);
        popover.add_css_class("hk-play-menu");
        popover.set_parent(self);
        popover.set_pointing_to(area(anchor, self).as_ref());
        popover.set_position(gtk::PositionType::Top);
        popover.set_has_arrow(false);
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        crate::views::actions::popup_when_built(&popover);
    }

    /// Opens the sleep timer popover above the sleep chip, or where the chip
    /// is while no timer runs.
    pub(crate) fn open_sleep_timer(&self) -> Option<gtk::Popover> {
        let imp = self.imp();
        let popover = imp.sleep_popover.borrow().clone()?;
        let target: gtk::Widget = match imp.sleep_chip.borrow().as_ref() {
            Some((chip, _)) if chip.is_visible() => chip.clone().upcast(),
            _ => imp.volume_icon.get().upcast(),
        };
        if let Some(view) = imp.playback.borrow().upgrade().map(|p| p.view()) {
            popover.show(&view);
        }
        popover.open(area(&target, self));
        Some(popover.popover().clone())
    }

    /// The play button's name and tooltip, with stop-after when it is on.
    pub(super) fn show_play_name(&self, view: &PlayerView) {
        let imp = self.imp();
        let name = if view.state == PlaybackState::Playing {
            tr("Pause")
        } else {
            tr("Play")
        };
        let name = match play_modes::stop_after_note(view.stop_after_mode) {
            Some(note) => fill(&tr("{}, {}"), &[&name, &note]),
            None => name,
        };
        imp.play.set_tooltip_text(Some(&name));
        imp.play
            .update_property(&[gtk::accessible::Property::Label(&name)]);
    }

    /// The mode buttons and the stop-after mark.
    pub(super) fn show_modes(&self, view: &PlayerView) {
        let imp = self.imp();
        let modes = &view.modes;
        play_modes::remember(modes);
        let name = play_modes::shuffle_tooltip(modes.shuffle);
        imp.shuffle.set_tooltip_text(Some(&name));
        imp.shuffle
            .update_property(&[gtk::accessible::Property::Label(&name)]);
        if modes.shuffle == ShuffleMode::Off {
            imp.shuffle.remove_css_class("active");
        } else {
            imp.shuffle.add_css_class("active");
        }
        imp.repeat
            .set_icon_name(if modes.repeat == RepeatMode::One {
                "helikodu-repeat-one-symbolic"
            } else {
                "helikodu-repeat-symbolic"
            });
        let name = play_modes::repeat_tooltip(modes.repeat);
        imp.repeat.set_tooltip_text(Some(&name));
        imp.repeat
            .update_property(&[gtk::accessible::Property::Label(&name)]);
        if modes.repeat == RepeatMode::Off {
            imp.repeat.remove_css_class("active");
        } else {
            imp.repeat.add_css_class("active");
        }
        imp.stop_mark
            .set_visible(view.stop_after_mode != StopAfter::Off);
        self.show_play_name(view);
    }

    /// The loop's flags and its chip.
    pub(super) fn show_loop(&self, view: &PlayerView) {
        let imp = self.imp();
        let points = view.ab_loop.unwrap_or_default();
        imp.seek_bar.set_loop(points.start, points.end);
        let Some((chip, text)) = imp.loop_chip.borrow().clone() else {
            return;
        };
        let shown = points.start.is_some() || points.end.is_some();
        chip.set_visible(shown);
        if !shown {
            return;
        }
        let time =
            |d: Option<Duration>| d.map_or_else(|| "–:––".to_owned(), crate::format::duration);
        let value = fill(
            &tr("A {} – B {}"),
            &[&time(points.start), &time(points.end)],
        );
        text.set_text(&value);
        chip.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("Loop: {}"),
            &[&value],
        ))]);
    }

    /// The sleep chip, counting down once a second while a timer runs.
    pub(super) fn show_sleep(&self, view: &PlayerView) {
        let imp = self.imp();
        self.tick_sleep(view);
        if let Some(popover) = imp.sleep_popover.borrow().as_ref() {
            popover.show(view);
        }
        self.keep_sleep_ticking(view);
    }

    /// Whether the countdown is worth a frame a second: while the timer runs
    /// and the chip is on screen, and while playing or with the popover open.
    /// Paused and closed, or hidden, the deck draws nothing; the chip catches
    /// up when playback goes on or it is shown again.
    fn sleep_ticks(&self, view: &PlayerView) -> bool {
        let imp = self.imp();
        let shown = imp
            .sleep_chip
            .borrow()
            .as_ref()
            .is_some_and(|(chip, _)| chip.is_mapped());
        let popover_open = imp
            .sleep_popover
            .borrow()
            .as_ref()
            .is_some_and(|p| p.popover().is_visible());
        view.sleep.is_some() && (popover_open || (shown && view.state == PlaybackState::Playing))
    }

    /// Starts or stops the countdown as [`Self::sleep_ticks`] says.
    pub(super) fn keep_sleep_ticking(&self, view: &PlayerView) {
        let imp = self.imp();
        let wanted = self.sleep_ticks(view);
        let ticking = imp.sleep_tick.borrow().is_some();
        if wanted && !ticking {
            let weak = self.downgrade();
            let id = glib::timeout_add_seconds_local(1, move || {
                let Some(deck) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let view = deck.imp().playback.borrow().upgrade().map(|p| p.view());
                match view.filter(|v| deck.sleep_ticks(v)) {
                    Some(view) => {
                        deck.tick_sleep(&view);
                        if let Some(popover) = deck.imp().sleep_popover.borrow().as_ref()
                            && popover.popover().is_visible()
                        {
                            popover.show(&view);
                        }
                        glib::ControlFlow::Continue
                    }
                    None => {
                        deck.imp().sleep_tick.replace(None);
                        glib::ControlFlow::Break
                    }
                }
            });
            imp.sleep_tick.replace(Some(id));
        } else if !wanted && let Some(id) = imp.sleep_tick.take() {
            id.remove();
        }
    }

    /// Brings the countdown up to date and keeps it going when the chip or
    /// the popover is shown again.
    fn refresh_sleep(&self) {
        if let Some(view) = self.imp().playback.borrow().upgrade().map(|p| p.view()) {
            self.tick_sleep(&view);
            self.keep_sleep_ticking(&view);
        }
    }

    pub(super) fn tick_sleep(&self, view: &PlayerView) {
        let Some((chip, text)) = self.imp().sleep_chip.borrow().clone() else {
            return;
        };
        match view.sleep_left() {
            Some(left) => {
                // Whole seconds, rounded up, so that the chip never shows
                // 0:00 while time is left.
                let shown =
                    Duration::from_secs(left.as_secs() + u64::from(left.subsec_nanos() > 0));
                let time = crate::format::duration(shown);
                text.set_text(&time);
                chip.update_property(&[gtk::accessible::Property::Label(&fill(
                    &tr("Sleep timer: {} left"),
                    &[&time],
                ))]);
                chip.set_visible(true);
            }
            None => chip.set_visible(false),
        }
    }

    /// The texts of the loop chip and the sleep chip while they are shown.
    pub(crate) fn chip_texts(&self) -> (Option<String>, Option<String>) {
        let imp = self.imp();
        let loop_text = imp
            .loop_chip
            .borrow()
            .as_ref()
            .filter(|(chip, _)| chip.is_visible())
            .map(|(_, text)| text.text().to_string());
        let sleep_text = imp
            .sleep_chip
            .borrow()
            .as_ref()
            .filter(|(chip, _)| chip.is_visible())
            .map(|(_, text)| text.text().to_string());
        (loop_text, sleep_text)
    }

    /// The shuffle and repeat buttons, for tests and keyboard hooks.
    pub(crate) fn mode_buttons(&self) -> (gtk::Button, gtk::Button) {
        let imp = self.imp();
        (imp.shuffle.get(), imp.repeat.get())
    }
}
