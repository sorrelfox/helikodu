// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Building blocks the dialogs share: the window frame and its header strip,
//! settings rows, switch rows, segmented controls, option cards, dropdowns,
//! key caps and a few formatting helpers.

use std::path::Path;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::i18n::tr;
use crate::widgets::{icon_button, label};

/// A dialog window over `parent`: themed, closed by Escape, destroyed with
/// its parent.
pub(crate) fn dialog_window(
    parent: Option<&gtk::Window>,
    name: &str,
    title: &str,
    size: (i32, i32),
    modal: bool,
) -> gtk::Window {
    let window = gtk::Window::builder()
        .title(title)
        .default_width(size.0)
        .default_height(size.1)
        .modal(modal)
        .destroy_with_parent(true)
        .build();
    window.set_widget_name(name);
    if let Some(parent) = parent {
        window.set_transient_for(Some(parent));
        if let Some(application) = parent.application() {
            window.set_application(Some(&application));
        }
    }
    window.add_css_class("hk-dialog");
    crate::style::register_window(&window);
    let keys = gtk::EventControllerKey::new();
    let weak = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape
            && let Some(window) = weak.upgrade()
        {
            window.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    // No title bar: the header strip drags the window and holds the close
    // button.
    let empty = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    empty.set_visible(false);
    window.set_titlebar(Some(&empty));
    window
}

/// The dialog of `name` already open over `parent`, if any.
pub(crate) fn find_open(parent: &gtk::Window, name: &str) -> Option<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| w.widget_name() == name && w.transient_for().as_ref() == Some(parent))
}

/// A 46 px strip: a title, an optional note at the right and a close button.
/// It drags the window.
pub(crate) fn header_strip(
    window: &gtk::Window,
    title: &str,
    note: Option<&str>,
) -> (gtk::WindowHandle, gtk::Box) {
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    strip.add_css_class("hk-strip");
    let heading = label(title, &["hk-title"]);
    heading.set_hexpand(true);
    strip.append(&heading);
    if let Some(note) = note {
        strip.append(&label(note, &["hk-t3", "hk-secondary"]));
    }
    let close = icon_button("window-close-symbolic", &tr("Close"));
    let weak = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    strip.append(&close);
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&strip));
    (handle, strip)
}

/// A label that wraps, for explanations.
pub(crate) fn note(text: &str) -> gtk::Label {
    let note = gtk::Label::new(Some(text));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    note.add_css_class("hk-note");
    note
}

/// A settings row: a 150 px label column, 20 px gap, then `control`.
pub(crate) fn row(
    title: &str,
    explanation: Option<&str>,
    control: &impl IsA<gtk::Widget>,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    row.add_css_class("hk-setting");
    let head = gtk::Box::new(gtk::Orientation::Vertical, 3);
    head.set_size_request(150, -1);
    head.set_hexpand(false);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_wrap(true);
    name.set_max_width_chars(18);
    name.add_css_class("hk-row-label");
    head.append(&name);
    if let Some(explanation) = explanation {
        let text = note(explanation);
        text.set_max_width_chars(22);
        head.append(&text);
    }
    row.append(&head);
    let control = control.as_ref();
    control.set_hexpand(true);
    row.append(control);
    row
}

/// A vertical box for the control area of a row: the control, then a note.
pub(crate) fn with_note(control: &impl IsA<gtk::Widget>, text: &str) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.append(control);
    column.append(&note(text));
    column
}

/// A switch row: a title over an explanation at the left, the switch at the
/// right. The whole row toggles the switch.
pub(crate) fn switch_row(title: &str, explanation: Option<&str>) -> (gtk::Box, gtk::Switch) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("hk-switch-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_wrap(true);
    text.append(&name);
    if let Some(explanation) = explanation {
        text.append(&note(explanation));
    }
    row.append(&text);
    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    switch.add_css_class("hk-switch");
    switch.update_property(&[gtk::accessible::Property::Label(title)]);
    row.append(&switch);
    let click = gtk::GestureClick::new();
    let weak = switch.downgrade();
    click.connect_released(move |gesture, _, x, y| {
        let Some(switch) = weak.upgrade() else { return };
        let Some(row) = gesture.widget() else { return };
        // A click on the switch itself toggles it already.
        if let Some(picked) = row.pick(x, y, gtk::PickFlags::DEFAULT)
            && (picked == switch.clone().upcast::<gtk::Widget>() || picked.is_ancestor(&switch))
        {
            return;
        }
        if switch.is_sensitive() {
            switch.set_active(!switch.is_active());
        }
    });
    row.add_controller(click);
    (row, switch)
}

/// A segmented control: toggle buttons in one group, in a box.
pub(crate) fn segmented(options: &[String]) -> (gtk::Box, Vec<gtk::ToggleButton>) {
    let group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    group.add_css_class("hk-seg");
    group.set_halign(gtk::Align::Start);
    group.set_valign(gtk::Align::Center);
    let mut buttons: Vec<gtk::ToggleButton> = Vec::new();
    for option in options {
        let button = gtk::ToggleButton::with_label(option);
        if let Some(first) = buttons.first() {
            button.set_group(Some(first));
        }
        group.append(&button);
        buttons.push(button);
    }
    (group, buttons)
}

/// Selects `index` in a segmented control and calls `changed` with each new
/// choice the user makes.
pub(crate) fn bind_segmented(
    buttons: &[gtk::ToggleButton],
    index: usize,
    changed: impl Fn(usize) + 'static,
) {
    if let Some(button) = buttons.get(index) {
        button.set_active(true);
    }
    let changed = Rc::new(changed);
    for (i, button) in buttons.iter().enumerate() {
        let changed = changed.clone();
        button.connect_toggled(move |button| {
            if button.is_active() {
                changed(i);
            }
        });
    }
}

/// An option card: a radio button with a title, a sentence under it, in a
/// bordered box that shows the choice.
pub(crate) fn option_card(
    title: &str,
    description: &str,
    group: Option<&gtk::CheckButton>,
) -> (gtk::Box, gtk::CheckButton) {
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    card.add_css_class("hk-option-card");
    card.set_hexpand(true);
    let check = gtk::CheckButton::new();
    check.set_valign(gtk::Align::Start);
    if let Some(group) = group {
        check.set_group(Some(group));
    }
    check.update_property(&[gtk::accessible::Property::Label(title)]);
    card.append(&check);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.add_css_class("hk-strong");
    text.append(&name);
    let sentence = gtk::Label::new(Some(description));
    sentence.set_xalign(0.0);
    sentence.set_wrap(true);
    sentence.add_css_class("hk-t2");
    sentence.add_css_class("hk-secondary");
    text.append(&sentence);
    card.append(&text);
    let weak = card.downgrade();
    check.connect_toggled(move |check| {
        if let Some(card) = weak.upgrade() {
            if check.is_active() {
                card.add_css_class("selected");
            } else {
                card.remove_css_class("selected");
            }
        }
    });
    let click = gtk::GestureClick::new();
    let weak = check.downgrade();
    click.connect_released(move |_, _, _, _| {
        if let Some(check) = weak.upgrade()
            && check.is_sensitive()
        {
            check.set_active(true);
        }
    });
    card.add_controller(click);
    (card, check)
}

/// A dropdown of `items`, with `selected` chosen.
pub(crate) fn dropdown(items: &[String], selected: usize, width: i32) -> gtk::DropDown {
    let strings: Vec<&str> = items.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::from_strings(&strings);
    dropdown.add_css_class("hk-input");
    dropdown.set_selected(u32::try_from(selected).unwrap_or(0));
    dropdown.set_halign(gtk::Align::Start);
    dropdown.set_valign(gtk::Align::Center);
    if width > 0 {
        dropdown.set_size_request(width, -1);
    }
    dropdown
}

/// Calls `changed` with each index the user picks in `dropdown`.
pub(crate) fn on_selected(dropdown: &gtk::DropDown, changed: impl Fn(usize) + 'static) {
    dropdown.connect_selected_notify(move |dropdown| {
        if let Ok(index) = usize::try_from(dropdown.selected()) {
            changed(index);
        }
    });
}

/// A secondary button at 30 px.
pub(crate) fn compact_button(text: &str) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("hk-btn");
    button.add_css_class("compact");
    button.set_valign(gtk::Align::Center);
    button
}

/// A row of key caps.
pub(crate) fn key_caps(keys: &[&str]) -> gtk::Box {
    let caps = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    caps.set_valign(gtk::Align::Center);
    for key in keys {
        let cap = gtk::Label::new(Some(key));
        cap.add_css_class("hk-key");
        cap.add_css_class("hk-mono");
        caps.append(&cap);
    }
    caps
}

/// A 1 px divider between groups of rows.
pub(crate) fn divider() -> gtk::Separator {
    let line = gtk::Separator::new(gtk::Orientation::Horizontal);
    line.add_css_class("hk-divider");
    line
}

/// `path` with the home folder written as `~`.
pub(crate) fn tilde(path: &Path) -> String {
    let home = glib::home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// A size in bytes as people read it, in binary units: `412 MiB`, `1.2 GiB`.
pub(crate) fn bytes(size: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Copies `text` to the clipboard of `widget`'s display.
pub(crate) fn copy_text(widget: &impl IsA<gtk::Widget>, text: &str) {
    widget.as_ref().display().clipboard().set_text(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_binary_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(412 << 20), "412 MiB");
        assert_eq!(bytes(1_288_490_189), "1.2 GiB");
        assert_eq!(bytes(12 << 20), "12 MiB");
    }
}
