// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The empty-state block: an icon tile, a title, one or two lines and at most
//! two buttons, centred in the content area.

use gtk::prelude::*;

/// A centred empty-state block. `buttons` are added in order; the first one
/// should be the primary action.
pub(crate) fn empty_state(
    icon: &str,
    title: &str,
    text: &str,
    buttons: &[gtk::Button],
) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 0);
    block.add_css_class("hk-empty");
    block.set_halign(gtk::Align::Center);
    block.set_valign(gtk::Align::Center);
    block.set_hexpand(true);
    block.set_vexpand(true);
    block.set_size_request(-1, -1);
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("hk-empty-tile");
    tile.set_halign(gtk::Align::Center);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(24);
    image.set_vexpand(true);
    image.set_valign(gtk::Align::Center);
    tile.append(&image);
    block.append(&tile);
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("hk-view-title");
    heading.set_margin_top(18);
    heading.set_wrap(true);
    heading.set_justify(gtk::Justification::Center);
    block.append(&heading);
    let body = gtk::Label::new(Some(text));
    body.add_css_class("hk-t2");
    body.set_margin_top(8);
    body.set_wrap(true);
    body.set_max_width_chars(52);
    body.set_justify(gtk::Justification::Center);
    block.append(&body);
    if !buttons.is_empty() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_halign(gtk::Align::Center);
        row.set_margin_top(20);
        for button in buttons {
            row.append(button);
        }
        block.append(&row);
    }
    block
}
