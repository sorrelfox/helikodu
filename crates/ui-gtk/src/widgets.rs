// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Small widgets shared by the views, most of them drawn by hand with colours
//! from the palette.

pub(crate) mod avatar;
pub(crate) mod bipolar_scale;
pub(crate) mod brand_mark;
pub(crate) mod cover_art;
pub(crate) mod empty_state;
pub(crate) mod playing_indicator;
pub(crate) mod rating_stars;
pub(crate) mod signal_lamp;
pub(crate) mod waveform_seek_bar;

use gtk::prelude::*;

pub(crate) use bipolar_scale::BipolarScale;
pub(crate) use brand_mark::BrandMark;
pub(crate) use cover_art::CoverArt;
pub(crate) use playing_indicator::PlayingIndicator;
pub(crate) use rating_stars::RatingStars;
pub(crate) use signal_lamp::SignalLamp;
pub(crate) use waveform_seek_bar::WaveformSeekBar;

/// Redraws `widget` whenever the look changes, for as long as it exists.
pub(crate) fn redraw_on_theme_change(widget: &impl IsA<gtk::Widget>) {
    let weak = widget.upcast_ref::<gtk::Widget>().downgrade();
    crate::style::connect_changed(move |_| {
        if let Some(widget) = weak.upgrade() {
            widget.queue_draw();
        }
    });
}

/// A single-line label that ellipsizes at the end.
pub(crate) fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_single_line_mode(true);
    for class in classes {
        label.add_css_class(class);
    }
    label
}

/// Removes every row of `list` and leaves its other children, such as a
/// context menu open on it (removing that would fail, again and again).
pub(crate) fn remove_rows(list: &gtk::ListBox) {
    let mut child = list.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if widget.is::<gtk::ListBoxRow>() {
            list.remove(&widget);
        }
    }
}

/// An icon-only button with its tooltip and accessible name.
pub(crate) fn icon_button(icon: &str, name: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("hk-icon");
    button.set_tooltip_text(Some(name));
    button.update_property(&[gtk::accessible::Property::Label(name)]);
    button.set_valign(gtk::Align::Center);
    button
}
