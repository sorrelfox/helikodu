// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Accessible names for the items of GTK's menus.
//!
//! GTK names a menu item (`GtkModelButton`) through a "labelled by" relation
//! to its own label, which GTK 4.24 does not expose to screen readers: every
//! item of a `GtkPopoverMenu` is then read without a name. [`name_menus`]
//! gives each item its text as its accessible label instead, every time the
//! menu is shown, so items that change (check marks, renamed playlists) stay
//! right.

use gtk::prelude::*;

/// Marks a menu whose items are named, so that it is set up once.
const DONE: &str = "hk-named-menu";

/// Names the items of every menu in `widget` (a `GtkPopoverMenu`, or a
/// widget holding menus: a menu button, a menu bar, a whole window) each time
/// one is shown. Call it again after a menu model is replaced.
pub(crate) fn name_menus(widget: &impl IsA<gtk::Widget>) {
    let widget = widget.as_ref();
    // Items of a later model are named at the next showing.
    if let Some(menu) = widget.downcast_ref::<gtk::PopoverMenu>()
        && !menu.has_css_class(DONE)
    {
        menu.add_css_class(DONE);
        menu.connect_map(|menu| name_items(menu.upcast_ref()));
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        name_menus(&c);
        child = c.next_sibling();
    }
}

/// Gives every menu item below `widget` its text as its accessible label.
fn name_items(widget: &gtk::Widget) {
    if widget.type_().name() == "GtkModelButton" && widget.find_property("text").is_some() {
        name_item(widget);
        // GTK sets the relation again when the item's shortcut arrives or
        // its text changes.
        if !widget.has_css_class(DONE) {
            widget.add_css_class(DONE);
            for property in ["accel", "text"] {
                widget.connect_notify_local(Some(property), |w, _| name_item(w));
            }
        }
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        name_items(&c);
        child = c.next_sibling();
    }
}

/// Names one menu item: its text as the label, its shortcut (as the menu
/// shows it, such as "Ctrl+Q") as the description.
fn name_item(item: &gtk::Widget) {
    let text = without_mnemonic(&item.property::<Option<String>>("text").unwrap_or_default());
    if text.is_empty() {
        return;
    }
    item.reset_relation(gtk::AccessibleRelation::LabelledBy);
    item.update_property(&[gtk::accessible::Property::Label(&text)]);
    let shortcut = item
        .find_property("accel")
        .and_then(|_| item.property::<Option<String>>("accel"))
        .and_then(|accel| gtk::accelerator_parse(&accel))
        .map(|(key, modifiers)| gtk::accelerator_get_label(key, modifiers).to_string());
    match shortcut {
        Some(shortcut) if !shortcut.is_empty() => {
            item.update_property(&[gtk::accessible::Property::Description(&shortcut)]);
        }
        _ => item.reset_property(gtk::AccessibleProperty::Description),
    }
}

/// A menu label without its mnemonic underscore (`__` is a literal one).
fn without_mnemonic(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                out.push('_');
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::without_mnemonic;

    #[test]
    fn mnemonics_are_dropped() {
        assert_eq!(without_mnemonic("_Open Files…"), "Open Files…");
        assert_eq!(without_mnemonic("a__b"), "a_b");
        assert_eq!(without_mnemonic("Plain"), "Plain");
    }
}
