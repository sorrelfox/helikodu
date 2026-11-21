// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The deck's speed chip, before the time left (after the loop chip when both
//! show), and its popover. The chip is there only while the speed is not 1×;
//! "Speed…" in the play button's menu and the Playback menu opens the same
//! popover while it is not. The times on the deck stay those of the track;
//! the chip says why they pass faster or slower than the clock.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::Deck;
use crate::backend::Backend;
use crate::i18n::{fill, tr};
use crate::playback::{Playback, PlayerView};
use crate::popovers::speed::{SpeedPopover, chip_name, speed_text};
use crate::widgets::label;

impl Deck {
    /// Sets up the speed chip and its popover.
    pub(super) fn setup_speed(&self, backend: &Backend, playback: &Rc<Playback>) {
        let imp = self.imp();
        let chip = gtk::Button::new();
        chip.set_widget_name("speed-chip");
        chip.add_css_class("hk-deck-chip");
        chip.add_css_class("hk-speed-chip");
        chip.set_valign(gtk::Align::Center);
        chip.set_visible(false);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let gauge = gtk::Image::from_icon_name("helikodu-gauge-symbolic");
        gauge.set_pixel_size(13);
        gauge.add_css_class("hk-chip-icon");
        line.append(&gauge);
        let text = label("", &["hk-mono", "hk-speed-text"]);
        line.append(&text);
        chip.set_child(Some(&line));
        let weak = self.downgrade();
        chip.connect_clicked(move |_| {
            if let Some(deck) = weak.upgrade() {
                deck.open_speed();
            }
        });
        if let Some(strip) = imp.remaining_button.parent().and_downcast::<gtk::Box>() {
            // After the loop chip, right before the time left.
            let before = imp.remaining_button.prev_sibling();
            strip.insert_child_after(&chip, before.as_ref());
        }
        let popover = SpeedPopover::new(self, backend, playback);
        imp.speed_popover.replace(Some(popover));
        imp.speed_chip.replace(Some((chip, text)));
    }

    /// The chip and the popover follow the speed and the output.
    pub(super) fn show_speed(&self, view: &PlayerView) {
        let imp = self.imp();
        if let Some((chip, text)) = imp.speed_chip.borrow().as_ref() {
            let normal = view.speed.is_normal();
            chip.set_visible(!normal);
            if !normal {
                let speed = speed_text(view.speed.rate);
                if view.speed_not_applied() {
                    text.set_text(&fill(&tr("{} · not applied"), &[&speed]));
                    chip.add_css_class("not-applied");
                } else {
                    text.set_text(&speed);
                    chip.remove_css_class("not-applied");
                }
                let name = chip_name(view);
                chip.set_tooltip_text(Some(&name));
                chip.update_property(&[gtk::accessible::Property::Label(&name)]);
            }
        }
        if let Some(popover) = imp.speed_popover.borrow().as_ref()
            && popover.popover().is_visible()
        {
            popover.show(view);
        }
    }

    /// The popover's listening time, while it is open.
    pub(super) fn tick_speed(&self, view: &PlayerView) {
        if let Some(popover) = self.imp().speed_popover.borrow().as_ref()
            && popover.popover().is_visible()
        {
            popover.show_time(view);
        }
    }

    /// Opens the speed popover above the speed chip, or above the time left
    /// while the speed is normal.
    pub(crate) fn open_speed(&self) -> Option<gtk::Popover> {
        let imp = self.imp();
        let popover = imp.speed_popover.borrow().clone()?;
        let target: gtk::Widget = match imp.speed_chip.borrow().as_ref() {
            Some((chip, _)) if chip.is_visible() => chip.clone().upcast(),
            _ => imp.remaining_button.get().upcast(),
        };
        // Right-aligned with the chip: the popover is centred on a point
        // half its width left of the chip's right edge.
        let half = crate::style::tokens::metric::sleep_timer::POPOVER_WIDTH / 2;
        let area = target.compute_bounds(self).map(|b| {
            let right = (b.x() + b.width()) as i32;
            gtk::gdk::Rectangle::new((right - half).max(0), b.y() as i32, 1, b.height() as i32)
        });
        popover.open(area);
        Some(popover.popover().clone())
    }

    /// The text of the speed chip while it is shown.
    pub(crate) fn speed_chip_text(&self) -> Option<String> {
        self.imp()
            .speed_chip
            .borrow()
            .as_ref()
            .filter(|(chip, _)| chip.is_visible())
            .map(|(_, text)| text.text().to_string())
    }
}
