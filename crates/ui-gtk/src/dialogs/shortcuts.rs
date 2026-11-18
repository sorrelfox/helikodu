// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keyboard shortcuts (Ctrl+?): every action that has a shortcut, in two
//! columns of groups as on the Shortcuts settings page: Playback at the left,
//! Library and lists and then Window at the right. Each row names the action
//! and shows its keys as key caps, or "Not set". The keys are read from the
//! application when the window opens, so they are the ones that work. A
//! filter at the top narrows the rows by the action's name or its keys.

use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;

use super::common::{dialog_window, find_open, header_strip, key_caps};
use crate::i18n::tr;
use crate::shortcuts::Group;
use crate::widgets::label;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "shortcuts";

/// The actions shown, by group, with their names.
fn actions() -> Vec<(Group, &'static str, String)> {
    crate::shortcuts::bindings()
        .into_iter()
        .map(|b| (b.group, b.action, b.name))
        .collect()
}

/// The key caps of an accelerator such as `<Control><Shift>Right`: the
/// modifiers, then the key, as the desktop names them.
pub(crate) fn caps(accelerator: &str) -> Vec<String> {
    let Some((key, mods)) = gtk::accelerator_parse(accelerator) else {
        return Vec::new();
    };
    let mut caps = Vec::new();
    for (mask, name) in [
        (gdk::ModifierType::CONTROL_MASK, tr("Ctrl")),
        (gdk::ModifierType::ALT_MASK, tr("Alt")),
        (gdk::ModifierType::SHIFT_MASK, tr("Shift")),
        (gdk::ModifierType::SUPER_MASK, tr("Super")),
    ] {
        if mods.contains(mask) {
            caps.push(name);
        }
    }
    let name = match key {
        gdk::Key::space => tr("Space"),
        gdk::Key::Return => tr("Return"),
        gdk::Key::Left => "←".to_owned(),
        gdk::Key::Right => "→".to_owned(),
        gdk::Key::Up => "↑".to_owned(),
        gdk::Key::Down => "↓".to_owned(),
        // `?` needs Shift on most layouts; the label names the character.
        gdk::Key::question => "?".to_owned(),
        other => gtk::accelerator_get_label(other, gdk::ModifierType::empty()).to_string(),
    };
    caps.push(name);
    caps
}

/// The accelerators of `action` in `application`. Space, which plays and
/// pauses, is a shortcut of the main window rather than an accelerator.
fn accelerators(application: Option<&gtk::Application>, action: &str) -> Vec<String> {
    let mut accels: Vec<String> = application
        .map(|a| {
            a.accels_for_action(action)
                .iter()
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    if action == "app.play-pause" && !accels.iter().any(|a| a == "space") {
        accels.insert(0, "space".into());
    }
    accels
}

/// One row: the action at the left, its keys at the right.
struct Row {
    widget: gtk::Box,
    group: Group,
    /// The action's name and its keys in lower case, for the filter.
    words: String,
}

fn row(name: &str, accels: &[String]) -> (gtk::Box, String) {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.add_css_class("hk-shortcut-row");
    let title = label(name, &[]);
    title.set_hexpand(true);
    title.set_xalign(0.0);
    line.append(&title);
    let mut words = name.to_lowercase();
    match accels.first() {
        Some(accel) => {
            let caps = caps(accel);
            let refs: Vec<&str> = caps.iter().map(String::as_str).collect();
            let keys = key_caps(&refs);
            keys.update_property(&[gtk::accessible::Property::Label(&caps.join("+"))]);
            line.append(&keys);
            for accel in accels {
                words.push(' ');
                words.push_str(&caps_text(accel).to_lowercase());
            }
        }
        None => line.append(&label(&tr("Not set"), &["hk-t3"])),
    }
    (line, words)
}

/// The keys of `accel` as one text, such as `Ctrl+Shift+→`.
fn caps_text(accel: &str) -> String {
    caps(accel).join("+")
}

/// Opens the window over `parent`, or brings it to the front.
pub(crate) fn open(parent: &gtk::Window) -> gtk::Window {
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        return window;
    }
    let window = dialog_window(
        Some(parent),
        NAME,
        &tr("Keyboard shortcuts"),
        (880, 640),
        false,
    );
    let application = parent.application();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let (header, _) = header_strip(&window, &tr("Keyboard shortcuts"), None);
    body.append(&header);

    let filter = gtk::SearchEntry::new();
    filter.add_css_class("hk-input");
    filter.set_placeholder_text(Some(&tr("Find an action or its keys")));
    filter.set_margin_top(18);
    filter.set_margin_start(28);
    filter.set_margin_end(28);
    filter.set_margin_bottom(6);
    body.append(&filter);

    let columns = gtk::Box::new(gtk::Orientation::Horizontal, 40);
    columns.set_homogeneous(true);
    columns.set_margin_start(28);
    columns.set_margin_end(28);
    columns.set_margin_bottom(24);
    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    columns.append(&left);
    columns.append(&right);

    let mut rows = Vec::new();
    let mut heads = Vec::new();
    for (group, title) in Group::all() {
        let column = if group == Group::Playback {
            &left
        } else {
            &right
        };
        let head = label(&title, &["hk-section-label", "hk-list-section"]);
        head.set_xalign(0.0);
        column.append(&head);
        heads.push((group, head));
        for (_, action, name) in actions().into_iter().filter(|(g, _, _)| *g == group) {
            let (widget, words) = row(&name, &accelerators(application.as_ref(), action));
            column.append(&widget);
            rows.push(Row {
                widget,
                group,
                words,
            });
        }
    }
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&columns));
    body.append(&scroller);
    window.set_child(Some(&body));

    let rows = Rc::new(rows);
    filter.connect_search_changed(move |entry| {
        let text = entry.text().trim().to_lowercase();
        for row in rows.iter() {
            row.widget
                .set_visible(text.split_whitespace().all(|w| row.words.contains(w)));
        }
        for (group, head) in &heads {
            head.set_visible(
                rows.iter()
                    .any(|r| r.group == *group && r.widget.is_visible()),
            );
        }
    });
    let weak = window.downgrade();
    filter.connect_stop_search(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    window.present();
    filter.grab_focus();
    window
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_is_named_once() {
        let all = actions();
        for (i, (_, action, _)) in all.iter().enumerate() {
            assert!(
                all.iter().skip(i + 1).all(|(_, other, _)| other != action),
                "{action} twice"
            );
        }
    }
}
