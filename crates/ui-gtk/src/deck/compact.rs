// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The compact deck of the narrow window: the cover, title and artist at the
//! left; previous, play and next in the middle; at the right a volume button
//! that opens a slider in a popover, and the output button with only its lamp,
//! the output's rate and bits, and the chevron. Shuffle and repeat are then in
//! the play button's menu.

use gtk::gio;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::Deck;
use crate::i18n::tr;

/// The widget names of the parts the compact deck adds.
const VOLUME_BUTTON: &str = "hk-compact-volume";
const OUTPUT_FORMAT: &str = "hk-compact-format";

/// The rate and bits of the output from the signal text: what follows " · ".
fn format_part(signal: &str) -> String {
    signal
        .rsplit_once(" · ")
        .map_or(signal, |(_, format)| format)
        .to_owned()
}

impl Deck {
    /// Makes the deck compact, or full again.
    pub(crate) fn set_compact(&self, compact: bool) {
        let imp = self.imp();
        if compact == self.is_compact() {
            return;
        }
        if compact {
            self.add_css_class("hk-compact");
        } else {
            self.remove_css_class("hk-compact");
        }
        imp.shuffle.set_visible(!compact);
        imp.repeat.set_visible(!compact);
        // The favourite button follows the title.
        if let Some(favourite) = imp.now_box.next_sibling() {
            favourite.set_visible(!compact);
        }
        imp.volume.set_visible(!compact);
        imp.volume_icon.set_visible(!compact);
        imp.device.set_visible(!compact);
        imp.signal.set_visible(!compact);
        self.volume_button().set_visible(compact);
        let format = self.format_label();
        format.set_text(&format_part(&imp.signal.text()));
        format.set_visible(compact);
    }

    /// Whether the deck is compact.
    pub(crate) fn is_compact(&self) -> bool {
        self.has_css_class("hk-compact")
    }

    /// Shuffle and repeat, for the play button's menu of the compact deck.
    pub(crate) fn compact_menu_section(&self) -> Option<gio::Menu> {
        if !self.is_compact() {
            return None;
        }
        let section = gio::Menu::new();
        section.append_submenu(Some(&tr("Shuffle")), &crate::play_modes::shuffle_menu());
        section.append_submenu(Some(&tr("Repeat")), &crate::play_modes::repeat_menu());
        Some(section)
    }

    /// The volume button, made the first time it is needed, before the output
    /// button; its slider shares the deck's volume.
    fn volume_button(&self) -> gtk::MenuButton {
        let imp = self.imp();
        let output = imp.output.get();
        let Some(end) = output.parent() else {
            return gtk::MenuButton::new();
        };
        let mut child = end.first_child();
        while let Some(widget) = child {
            if widget.widget_name() == VOLUME_BUTTON
                && let Ok(button) = widget.clone().downcast::<gtk::MenuButton>()
            {
                return button;
            }
            child = widget.next_sibling();
        }
        let button = gtk::MenuButton::new();
        button.set_widget_name(VOLUME_BUTTON);
        button.set_icon_name("audio-volume-high-symbolic");
        button.add_css_class("hk-menu");
        button.set_valign(gtk::Align::Center);
        button.set_tooltip_text(Some(&tr("Volume")));
        button.update_property(&[gtk::accessible::Property::Label(&tr("Volume"))]);
        let slider = gtk::Scale::new(gtk::Orientation::Vertical, Some(&imp.volume.adjustment()));
        slider.set_inverted(true);
        slider.set_size_request(-1, 120);
        slider.add_css_class("hk-volume");
        slider.update_property(&[gtk::accessible::Property::Label(&tr("Volume"))]);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&slider));
        popover.set_position(gtk::PositionType::Top);
        button.set_popover(Some(&popover));
        // The icon follows the deck's own volume icon.
        imp.volume_icon
            .bind_property("icon-name", &button, "icon-name")
            .sync_create()
            .build();
        button.insert_before(&end, Some(&output));
        button.set_visible(false);
        button
    }

    /// The output's rate and bits in the compact output button, made the first
    /// time it is needed; it follows the signal text.
    fn format_label(&self) -> gtk::Label {
        let imp = self.imp();
        let signal = imp.signal.get();
        let Some(column) = signal.parent().and_downcast::<gtk::Box>() else {
            return gtk::Label::new(None);
        };
        let mut child = column.first_child();
        while let Some(widget) = child {
            if widget.widget_name() == OUTPUT_FORMAT
                && let Ok(label) = widget.clone().downcast::<gtk::Label>()
            {
                return label;
            }
            child = widget.next_sibling();
        }
        let label = gtk::Label::new(None);
        label.set_widget_name(OUTPUT_FORMAT);
        label.add_css_class("hk-signal");
        label.set_xalign(0.0);
        label.set_visible(false);
        column.append(&label);
        let weak = label.downgrade();
        signal.connect_label_notify(move |signal| {
            if let Some(label) = weak.upgrade() {
                label.set_text(&format_part(&signal.text()));
            }
        });
        for class in ["idle", "bit-perfect", "processed"] {
            if signal.has_css_class(class) {
                label.add_css_class(class);
            }
        }
        let weak = label.downgrade();
        signal.connect_css_classes_notify(move |signal| {
            if let Some(label) = weak.upgrade() {
                for class in ["idle", "bit-perfect", "processed"] {
                    if signal.has_css_class(class) {
                        label.add_css_class(class);
                    } else {
                        label.remove_css_class(class);
                    }
                }
            }
        });
        label
    }

    /// The rate and bits the compact output button shows (for tests).
    pub(crate) fn compact_output_text(&self) -> Option<String> {
        self.is_compact()
            .then(|| self.format_label().text().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::format_part;

    #[test]
    fn the_compact_output_keeps_rate_and_bits() {
        assert_eq!(format_part("BIT-PERFECT · 96 / 24"), "96 / 24");
        assert_eq!(format_part("44.1 / 16"), "44.1 / 16");
    }
}
