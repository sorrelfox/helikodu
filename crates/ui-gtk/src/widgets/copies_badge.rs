// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The badge after a track's title when the library has byte-identical copies
//! of its file: a small two-squares icon in `t3`. Its tooltip and accessible
//! name say how many copies there are; a click opens the copies window
//! (`win.show-copies` with the track). It takes no keyboard focus, so a list
//! keeps its row-to-row navigation: the context menu's "Copies…" and Alt+C
//! open the same window from the keyboard.

use gtk::prelude::*;

use crate::backend::TrackKey;
use crate::i18n::{fill, ntr};

/// The action a badge activates, with the track key as its target.
pub(crate) const ACTION: &str = "win.show-copies";

/// "N identical copies".
pub(crate) fn describe(copies: u32) -> String {
    fill(
        &ntr("{} identical copy", "{} identical copies", copies as usize),
        &[&copies.to_string()],
    )
}

/// A badge, not yet showing a track.
pub(crate) fn new() -> gtk::Button {
    let icon = gtk::Image::from_icon_name("helikodu-copies-symbolic");
    icon.set_pixel_size(14);
    let badge = gtk::Button::new();
    badge.set_child(Some(&icon));
    badge.set_has_frame(false);
    badge.set_focusable(false);
    badge.set_focus_on_click(false);
    badge.set_valign(gtk::Align::Center);
    badge.add_css_class("hk-copies-badge");
    badge
}

/// Shows the copies of `track` on `badge`: hidden when it has none.
pub(crate) fn show(badge: &gtk::Button, track: TrackKey, copies: u32) {
    let shown = copies > 0 && track.0 != 0;
    badge.set_visible(shown);
    if !shown {
        return;
    }
    let text = describe(copies);
    if badge.tooltip_text().as_deref() != Some(text.as_str()) {
        badge.set_tooltip_text(Some(&text));
        badge.update_property(&[gtk::accessible::Property::Label(&text)]);
    }
    badge.set_action_target_value(Some(&track.0.to_variant()));
    // Named after the target: an action with a parameter refuses a button
    // without one.
    if badge.action_name().is_none() {
        badge.set_action_name(Some(ACTION));
    }
}
