// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The entry of a naming pattern, as Organize files, Convert, Sync and export
//! and the managed folder have it: a mono entry and, beside it, the Insert
//! button with the popover of fields and functions (see
//! [`insert_popover`](super::insert_popover)).

use gtk::prelude::*;

use super::insert_popover::{InsertTarget, insert_button};

/// A pattern entry with its Insert button.
#[derive(Debug, Clone)]
pub(crate) struct PatternEntry {
    /// The entry and the button side by side; this goes into the layout.
    pub(crate) root: gtk::Box,
    /// The entry.
    pub(crate) entry: gtk::Entry,
    /// The Insert button.
    pub(crate) insert: gtk::MenuButton,
}

impl PatternEntry {
    /// An empty pattern entry; `name` is the entry's widget name.
    pub(crate) fn new(name: &str) -> Self {
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.add_css_class("hk-mono");
        entry.add_css_class("hk-pattern-entry");
        entry.set_hexpand(true);
        entry.set_widget_name(name);
        let insert = insert_button(InsertTarget::entry(&entry));
        insert.set_valign(gtk::Align::Center);
        insert.set_widget_name(&format!("{name}-insert"));
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.append(&entry);
        root.append(&insert);
        Self {
            root,
            entry,
            insert,
        }
    }

    /// Names the entry for assistive technologies after `label`.
    pub(crate) fn labelled_by(&self, label: &impl IsA<gtk::Accessible>) {
        self.entry
            .update_relation(&[gtk::accessible::Relation::LabelledBy(&[label.upcast_ref()])]);
    }
}

/// Puts the wavy underline of a part that cannot be read under the characters
/// `span` of `entry`'s text (counted from 0; an empty span marks one
/// character), or takes it away. No colour marks it.
pub(crate) fn underline(entry: &impl IsA<gtk::Editable>, span: Option<(usize, usize)>) {
    let editable = entry.as_ref();
    let Some(text) = editable
        .delegate()
        .and_downcast::<gtk::Text>()
        .or_else(|| editable.clone().downcast::<gtk::Text>().ok())
    else {
        return;
    };
    let attributes = gtk::pango::AttrList::new();
    if let Some((start, end)) = span {
        let content = text.text();
        let byte = |chars: usize| {
            content
                .char_indices()
                .nth(chars)
                .map_or(content.len(), |(i, _)| i)
        };
        let (from, to) = (byte(start), byte(end.max(start + 1)));
        let (from, to) = if from >= content.len() && !content.is_empty() {
            // At the end: mark the last character.
            (
                byte(content.chars().count().saturating_sub(1)),
                content.len(),
            )
        } else {
            (from, to)
        };
        let mut wavy = gtk::pango::AttrInt::new_underline(gtk::pango::Underline::Error);
        wavy.set_start_index(u32::try_from(from).unwrap_or(0));
        wavy.set_end_index(u32::try_from(to).unwrap_or(u32::MAX));
        attributes.insert(wavy);
    }
    text.set_attributes(Some(&attributes));
}
