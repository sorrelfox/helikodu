// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Editing tags in the tracks table: F2 turns the cell under the focus (the
//! column last clicked, the title otherwise) into an entry. With several rows
//! selected, the value is shown as it is typed in the same column of the
//! other selected rows. Enter saves into every selected track, Tab saves and
//! goes on to the next column, Esc cancels. The status bar says what is
//! edited and the keys. Saving goes through the tag editor's write and
//! shows a toast with Undo.

use std::collections::BTreeMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::{TracksView, cell_text, column_id, column_title};
use crate::backend::services::{TagEdit, TagField};
use crate::backend::{TrackColumn, TrackKey};
use crate::dialogs::tag_editor::valid_date;
use crate::i18n::{fill, ntr, tr};
use crate::views::table::{TableCell, selected_positions};

/// The columns that hold tags, in the order Tab goes through them.
const EDITABLE: [TrackColumn; 6] = [
    TrackColumn::Number,
    TrackColumn::Title,
    TrackColumn::Artist,
    TrackColumn::Album,
    TrackColumn::Genre,
    TrackColumn::Year,
];

/// The tag field a column shows.
fn column_field(column: TrackColumn) -> Option<TagField> {
    Some(match column {
        TrackColumn::Number => TagField::Track,
        TrackColumn::Title => TagField::Title,
        TrackColumn::Artist => TagField::Artist,
        TrackColumn::Album => TagField::Album,
        TrackColumn::Genre => TagField::Genre,
        TrackColumn::Year => TagField::Year,
        _ => return None,
    })
}

/// The values typed into a column, as the tag editor takes them.
fn values(column: TrackColumn, text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    match column {
        TrackColumn::Artist | TrackColumn::Genre => text
            .split(';')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => vec![text.to_owned()],
    }
}

/// Why a typed value cannot be saved, if it cannot.
fn invalid(column: TrackColumn, text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match column {
        TrackColumn::Year if !valid_date(text) => Some(tr("A year, or a date as YYYY-MM-DD")),
        TrackColumn::Number if !text.bytes().all(|b| b.is_ascii_digit()) => {
            Some(tr("A track number is a whole number"))
        }
        _ => None,
    }
}

/// An edit in progress.
#[derive(Debug)]
pub(super) struct CellEdit {
    column: TrackColumn,
    /// The selected tracks, the edited row's first.
    keys: Vec<TrackKey>,
    anchor: TrackKey,
    entry: gtk::Entry,
    /// The text the cell showed when the edit began.
    start: String,
    /// The status bar's texts before the edit.
    status: (String, String),
}

impl TracksView {
    /// The column under the point `(x, y)` of the table, if a cell is there.
    pub(super) fn column_at(&self, x: f64, y: f64) -> Option<TrackColumn> {
        let cell = crate::views::table::cell_at(&self.table, x, y)?;
        EDITABLE
            .into_iter()
            .chain([
                TrackColumn::Format,
                TrackColumn::Rating,
                TrackColumn::Plays,
                TrackColumn::Length,
                TrackColumn::Added,
            ])
            .find(|c| cell.widget_name() == column_id(*c))
    }

    /// Whether a cell is being edited.
    pub(crate) fn is_editing(&self) -> bool {
        self.editing.borrow().is_some()
    }

    /// The shown cell of `column` at `position`.
    fn cell_of(&self, column: TrackColumn, position: u32) -> Option<TableCell> {
        self.cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
            .find(|c| c.position() == Some(position) && c.widget_name() == column_id(column))
    }

    /// Starts editing the column last clicked (the title when that holds no
    /// tag) in the focused row of the selection.
    pub(crate) fn start_edit(self: &Rc<Self>) {
        let column = self
            .clicked_column
            .get()
            .filter(|c| EDITABLE.contains(c))
            .unwrap_or(TrackColumn::Title);
        self.edit_column(column);
    }

    fn edit_column(self: &Rc<Self>, column: TrackColumn) {
        self.cancel_edit();
        let positions = selected_positions(&self.selection);
        let Some(&first) = positions.first() else {
            return;
        };
        let anchor_position = self
            .clicked_row
            .get()
            .filter(|p| positions.contains(p))
            .unwrap_or(first);
        let keys = self.model.keys();
        let Some(&anchor) = keys.get(anchor_position as usize) else {
            return;
        };
        let mut selected: Vec<TrackKey> = vec![anchor];
        selected.extend(
            positions
                .iter()
                .filter_map(|p| keys.get(*p as usize).copied())
                .filter(|k| *k != anchor),
        );
        let visible = self
            .columns
            .iter()
            .any(|(c, view)| *c == column && view.is_visible());
        if !visible {
            return;
        }
        let Some(cell) = self.cell_of(column, anchor_position) else {
            // Bring the row into view first, then edit it.
            if let Some((_, view)) = self.columns.iter().find(|(c, _)| *c == column) {
                self.table.scroll_to(
                    anchor_position,
                    Some(view),
                    gtk::ListScrollFlags::FOCUS,
                    None,
                );
            }
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(view) = weak.upgrade()
                    && view.cell_of(column, anchor_position).is_some()
                {
                    view.edit_column(column);
                }
            });
            return;
        };
        let start = self
            .model
            .cached(anchor)
            .map(|row| cell_text(&row, column))
            .unwrap_or_default();
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-cell-edit");
        entry.set_text(&start);
        entry.set_valign(gtk::Align::Center);
        entry.update_property(&[gtk::accessible::Property::Label(&column_title(column))]);
        cell.set_text("");
        cell.set_child(&entry);
        let status = (
            self.status_selection.text().to_string(),
            self.status_sort.text().to_string(),
        );
        let n = selected.len();
        let what = column_title(column);
        self.status_selection.set_text(&if n == 1 {
            fill(&tr("Editing {}"), &[&what])
        } else {
            fill(
                &ntr(
                    "Editing {} for the {} selected track",
                    "Editing {} for the {} selected tracks",
                    n,
                ),
                &[&what, &n.to_string()],
            )
        });
        self.status_sort.set_text(&tr(
            "Enter saves · Tab goes to the next column · Esc cancels",
        ));
        self.editing.replace(Some(CellEdit {
            column,
            keys: selected,
            anchor,
            entry: entry.clone(),
            start,
            status,
        }));
        let weak = Rc::downgrade(self);
        entry.connect_changed(move |entry| {
            if let Some(view) = weak.upgrade() {
                view.preview_edit(&entry.text());
            }
        });
        let weak = Rc::downgrade(self);
        entry.connect_activate(move |_| {
            if let Some(view) = weak.upgrade() {
                view.finish_edit(None);
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::Escape => {
                    view.cancel_edit();
                    view.table.grab_focus();
                    glib::Propagation::Stop
                }
                gdk::Key::Tab | gdk::Key::ISO_Left_Tab | gdk::Key::KP_Tab => {
                    let back = key == gdk::Key::ISO_Left_Tab
                        || state.contains(gdk::ModifierType::SHIFT_MASK);
                    view.finish_edit(Some(back));
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        entry.add_controller(keys);
        entry.grab_focus();
        self.preview_edit(&entry.text());
    }

    /// Shows `text` in the edited column of the other selected rows.
    fn preview_edit(&self, text: &str) {
        let editing = self.editing.borrow();
        let Some(edit) = editing.as_ref() else {
            return;
        };
        let problem = invalid(edit.column, text);
        if problem.is_some() {
            edit.entry.add_css_class("error");
        } else {
            edit.entry.remove_css_class("error");
        }
        edit.entry.set_tooltip_text(problem.as_deref());
        for cell in self
            .cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
        {
            if cell.widget_name() != column_id(edit.column) {
                continue;
            }
            let Some(item) = cell.item() else { continue };
            if item.key() != edit.anchor && edit.keys.contains(&item.key()) {
                cell.set_text(text);
                cell.add_css_class("hk-edit-preview");
            }
        }
    }

    /// The edit's look on a cell that shows `key` in `column` again.
    pub(super) fn decorate_cell(&self, cell: &TableCell, key: TrackKey, column: TrackColumn) {
        let editing = self.editing.borrow();
        match editing.as_ref() {
            Some(edit) if edit.column == column && key == edit.anchor => {
                let holds_entry = cell
                    .child()
                    .is_some_and(|c| c.has_css_class("hk-cell-edit"));
                if holds_entry {
                    cell.set_text("");
                }
            }
            Some(edit)
                if edit.column == column && key != edit.anchor && edit.keys.contains(&key) =>
            {
                cell.set_text(&edit.entry.text());
                cell.add_css_class("hk-edit-preview");
            }
            _ => cell.remove_css_class("hk-edit-preview"),
        }
    }

    /// Ends the edit when the row of its entry leaves the screen.
    pub(super) fn cell_unbound(&self, cell: &TableCell) {
        let holds_entry = cell
            .child()
            .is_some_and(|c| c.has_css_class("hk-cell-edit"));
        if holds_entry {
            self.cancel_edit();
        }
    }

    /// Ends the edit without saving.
    pub(crate) fn cancel_edit(&self) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        self.end_edit(&edit);
    }

    fn end_edit(&self, edit: &CellEdit) {
        if let Some(cell) = edit.entry.parent().and_downcast::<TableCell>() {
            edit.entry.unparent();
            cell.set_text(&edit.start);
        }
        for cell in self
            .cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
        {
            if cell.has_css_class("hk-edit-preview") {
                cell.remove_css_class("hk-edit-preview");
                if let Some(item) = cell.item() {
                    item.emit_by_name::<()>("loaded", &[]);
                }
            }
        }
        self.status_selection.set_text(&edit.status.0);
        self.status_sort.set_text(&edit.status.1);
    }

    /// Saves the edit into the selected tracks; with `next`, goes on to the
    /// next column (or the one before for `Some(true)`).
    fn finish_edit(self: &Rc<Self>, next: Option<bool>) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        let text = edit.entry.text().to_string();
        if let Some(problem) = invalid(edit.column, &text) {
            // Keep editing: the value cannot be written.
            edit.entry.set_tooltip_text(Some(&problem));
            edit.entry.error_bell();
            self.editing.replace(Some(edit));
            return;
        }
        let column = edit.column;
        let keys = edit.keys.clone();
        let unchanged = text.trim() == edit.start.trim()
            && keys.iter().all(|k| {
                self.model
                    .cached(*k)
                    .is_some_and(|row| cell_text(&row, column).trim() == text.trim())
            });
        self.end_edit(&edit);
        let following = next.and_then(|back| {
            let index = EDITABLE.iter().position(|c| *c == column)?;
            let shown = |c: &TrackColumn| {
                self.columns
                    .iter()
                    .any(|(col, view)| col == c && view.is_visible())
            };
            if back {
                EDITABLE[..index].iter().rev().find(|c| shown(c)).copied()
            } else {
                EDITABLE[index + 1..].iter().find(|c| shown(c)).copied()
            }
        });
        if !unchanged && let Some(field) = column_field(column) {
            self.save_edit(keys, field, values(column, &text));
        }
        match following {
            Some(next) => self.edit_column(next),
            None => {
                self.table.grab_focus();
            }
        }
    }

    fn save_edit(self: &Rc<Self>, keys: Vec<TrackKey>, field: TagField, values: Vec<String>) {
        let services = self.backend.services();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let n = keys.len();
            let job = move || {
                services.edit_tracks(&keys)?;
                let mut fields = BTreeMap::new();
                fields.insert(field, values);
                Ok::<_, crate::backend::BackendError>(services.write_tags(&TagEdit {
                    tracks: keys,
                    fields,
                    ..TagEdit::default()
                }))
            };
            let result = gio::spawn_blocking(job).await;
            let Some(view) = weak.upgrade() else { return };
            let window = view.root.root().and_downcast::<crate::window::MainWindow>();
            match result {
                Ok(Ok(results)) => {
                    let failed: Vec<_> = results.iter().filter(|r| r.error.is_some()).collect();
                    if let Some(window) = &window {
                        if let Some(first) = failed.first() {
                            window.toast(
                                &fill(
                                    &tr("{} of {} files could not be saved. {}: {}"),
                                    &[
                                        &failed.len().to_string(),
                                        &n.to_string(),
                                        &first.file,
                                        first.error.as_deref().unwrap_or_default(),
                                    ],
                                ),
                                true,
                            );
                        } else {
                            window.toast_tags_undo(&fill(
                                &ntr("Saved {} file", "Saved {} files", n),
                                &[&n.to_string()],
                            ));
                        }
                    }
                    view.reload();
                }
                Ok(Err(error)) => {
                    if let Some(window) = &window {
                        window.toast(&error.0, true);
                    }
                }
                Err(_) => {}
            }
        });
    }

    /// Starts editing `column` of the selected rows (for tests).
    pub(crate) fn demo_edit(self: &Rc<Self>, column: TrackColumn) -> Option<gtk::Entry> {
        self.edit_column(column);
        self.editing.borrow().as_ref().map(|e| e.entry.clone())
    }

    /// The texts of the cells of `column` shown in the edit's look (for
    /// tests).
    pub(crate) fn previewed(&self, column: TrackColumn) -> Vec<String> {
        self.cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
            .filter(|c| c.widget_name() == column_id(column) && c.has_css_class("hk-edit-preview"))
            .map(|c| c.shown_text())
            .collect()
    }

    /// The status bar's texts (for tests).
    pub(crate) fn status_texts(&self) -> (String, String) {
        (
            self.status_selection.text().to_string(),
            self.status_sort.text().to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_values_split_and_check() {
        assert_eq!(
            values(TrackColumn::Artist, "A; B ;"),
            vec!["A".to_owned(), "B".to_owned()]
        );
        assert_eq!(values(TrackColumn::Title, "A; B"), vec!["A; B".to_owned()]);
        assert!(values(TrackColumn::Album, "  ").is_empty());
        assert!(invalid(TrackColumn::Year, "2018").is_none());
        assert!(invalid(TrackColumn::Year, "18th").is_some());
        assert!(invalid(TrackColumn::Number, "3a").is_some());
        assert!(invalid(TrackColumn::Number, "").is_none());
        assert_eq!(column_field(TrackColumn::Format), None);
    }
}
