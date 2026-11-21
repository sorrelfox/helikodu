// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Small widgets shared by the views, most of them drawn by hand with colours
//! from the palette.

pub(crate) mod avatar;
pub(crate) mod bipolar_scale;
pub(crate) mod brand_mark;
pub(crate) mod copies_badge;
pub(crate) mod cover_art;
pub(crate) mod empty_state;
pub(crate) mod eq_graph;
pub(crate) mod insert_popover;
pub(crate) mod level_meter;
pub(crate) mod loudness_curve;
pub(crate) mod lyric_lines;
pub(crate) mod menu_names;
pub(crate) mod now_links;
pub(crate) mod pane_layouts;
pub(crate) mod pattern_entry;
pub(crate) mod playing_indicator;
pub(crate) mod rating_stars;
pub(crate) mod side_column;
pub(crate) mod signal_lamp;
pub(crate) mod signal_path_chain;
pub(crate) mod signal_path_list;
pub(crate) mod spectrum_view;
pub(crate) mod tile_grid;
pub(crate) mod title_line;
pub(crate) mod waveform_seek_bar;
pub(crate) mod wrap_box;

use gtk::prelude::*;

pub(crate) use bipolar_scale::BipolarScale;
pub(crate) use brand_mark::BrandMark;
pub(crate) use cover_art::CoverArt;
pub(crate) use playing_indicator::{
    PlayingIndicator, set_running as set_playing_indicators_running,
};
pub(crate) use rating_stars::RatingStars;
pub(crate) use signal_lamp::SignalLamp;
pub(crate) use tile_grid::TileGrid;
pub(crate) use title_line::TitleLine;
pub(crate) use waveform_seek_bar::WaveformSeekBar;
pub(crate) use wrap_box::WrapBox;

/// Redraws `widget` whenever the look changes, for as long as it exists.
pub(crate) fn redraw_on_theme_change(widget: &impl IsA<gtk::Widget>) {
    let weak = widget.upcast_ref::<gtk::Widget>().downgrade();
    crate::style::connect_changed(move |_| {
        if let Some(widget) = weak.upgrade() {
            widget.queue_draw();
        }
    });
}

/// A label that continues on further lines when it does not fit, for a
/// sentence that must be read whole (a longer language would otherwise cut it
/// off).
pub(crate) fn wrapped_label(text: &str, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    for class in classes {
        label.add_css_class(class);
    }
    label
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

/// The title of a page's header. It keeps its whole text: what follows it on
/// the line gives way first (see [`view_note`]).
pub(crate) fn view_title(text: &str) -> gtk::Label {
    let title = label(text, &["hk-view-title"]);
    title.set_ellipsize(gtk::pango::EllipsizeMode::None);
    title.set_hexpand(false);
    title.set_accessible_role(gtk::AccessibleRole::Heading);
    title
}

/// The count or note after a page's title, with `classes`: it takes the room
/// left on the line, and is the first to give way when the line is narrow,
/// before the title and the controls.
pub(crate) fn view_note(classes: &[&str]) -> gtk::Label {
    let note = label("", classes);
    note.set_valign(gtk::Align::Baseline);
    note.set_hexpand(true);
    note.set_max_width_chars(1);
    note
}

/// Fills `line` with a translated sentence that holds widgets: the texts of
/// `pattern` become labels with `classes`, and each placeholder the widget
/// of `widgets` it stands for, in the order the translation puts them (see
/// [`crate::i18n::pieces`]). Returns the labels, which can name the widgets
/// for assistive technology.
/// The words are never cut off: a sentence missing words cannot be read.
pub(crate) fn sentence(
    line: &gtk::Box,
    pattern: &str,
    widgets: &[&gtk::Widget],
    classes: &[&str],
) -> Vec<gtk::Label> {
    let mut labels = Vec::new();
    for piece in crate::i18n::pieces(pattern, widgets.len()) {
        match piece {
            crate::i18n::Piece::Text(text) => {
                let words = label(&text, classes);
                words.set_ellipsize(gtk::pango::EllipsizeMode::None);
                line.append(&words);
                labels.push(words);
            }
            crate::i18n::Piece::Slot(i) => line.append(widgets[i]),
        }
    }
    labels
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

/// Gives the list in `scroller` (whose height is capped by its maximum content
/// height) a scrollbar only when the list is taller than that cap. A popover
/// whose content comes to need more room than it was shown with is closed by
/// GTK at once, and the scrollbar of a short list does: it is taller than one
/// row and grows when the pointer comes near it.
pub(crate) fn scroll_when_taller(scroller: &gtk::ScrolledWindow) {
    let Some(child) = scroller.child() else {
        return;
    };
    let (_, natural, _, _) = child.measure(gtk::Orientation::Vertical, -1);
    let cap = scroller.max_content_height();
    scroller.set_vscrollbar_policy(if cap >= 0 && natural > cap {
        gtk::PolicyType::Automatic
    } else {
        gtk::PolicyType::Never
    });
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
