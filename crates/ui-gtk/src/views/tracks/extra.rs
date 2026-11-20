// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The columns of the track table beyond the tags: Labels (a track's own labels
//! and those of its album and artists) and one column for each of the user's
//! fields, offered in the Columns menu under "Your fields".
//!
//! Their values come with the rows: while such a column is shown, the model asks
//! the backend for them in the same batch as the rows it fetches. A computed
//! field cannot sort the list, so its header is not clickable and says why; a
//! field that stands for a tag sorts like any text column.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::{TracksView, column_id, column_title};
use crate::backend::{ExtraColumns, FieldName, TrackColumn, YourField};
use crate::i18n::tr;
use crate::models::TrackItem;
use crate::views::table::TableCell;

/// The width of a column of one of the user's fields.
const FIELD_WIDTH: i32 = 76;

impl TracksView {
    /// Every column with its kind: the fixed ones and those of the user's
    /// fields.
    pub(super) fn all_columns(&self) -> Vec<(TrackColumn, gtk::ColumnViewColumn)> {
        self.columns
            .iter()
            .cloned()
            .chain(self.field_columns.borrow().iter().cloned())
            .collect()
    }

    /// The kind of the column `c`.
    pub(super) fn column_of(&self, c: &gtk::ColumnViewColumn) -> Option<TrackColumn> {
        self.all_columns()
            .into_iter()
            .find(|(_, own)| own == c)
            .map(|(column, _)| column)
    }

    /// Follows the Labels column and loads the user's fields for their columns.
    pub(super) fn connect_extra(self: &Rc<Self>) {
        if let Some((_, labels)) = self
            .columns
            .iter()
            .find(|(column, _)| *column == TrackColumn::Labels)
        {
            let weak = Rc::downgrade(self);
            labels.connect_visible_notify(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.extras_changed();
                }
            });
        }
        self.load_fields();
    }

    /// Asks the backend for the user's fields again, and adds, keeps or removes
    /// their columns and menu items.
    pub(crate) fn load_fields(self: &Rc<Self>) {
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(fields) = backend.query(|library| library.your_fields()).await else {
                return;
            };
            if let Some(view) = weak.upgrade() {
                view.set_fields(fields);
            }
        });
    }

    fn set_fields(self: &Rc<Self>, fields: Vec<YourField>) {
        if *self.fields.borrow() == fields {
            return;
        }
        let names: Vec<FieldName> = fields.iter().map(|f| FieldName::new(&f.name)).collect();
        // The columns of fields that are gone go, and so do their sort keys.
        let gone: Vec<(TrackColumn, gtk::ColumnViewColumn)> = {
            let mut columns = self.field_columns.borrow_mut();
            let (kept, gone): (Vec<_>, Vec<_>) = columns.drain(..).partition(
                |(column, _)| matches!(column, TrackColumn::Field(n) if names.contains(n)),
            );
            *columns = kept;
            gone
        };
        for (column, c) in &gone {
            self.table.remove_column(c);
            if let TrackColumn::Field(name) = column {
                self.column_actions.remove_action(name.id());
            }
        }
        let resort = !gone.is_empty()
            && self
                .sort
                .borrow()
                .iter()
                .any(|k| gone.iter().any(|(c, _)| *c == k.column));
        if resort {
            self.sort
                .borrow_mut()
                .retain(|k| !gone.iter().any(|(c, _)| *c == k.column));
        }
        // A kept field may have changed between a tag and an expression.
        for (column, c) in self.field_columns.borrow().iter() {
            let sorts = self.backend.library().sortable(*column);
            if sorts != c.sorter().is_some() {
                c.set_sorter(
                    sorts
                        .then(|| gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal))
                        .as_ref(),
                );
            }
        }
        // New fields get a column, hidden, before the Labels column.
        for name in &names {
            let column = TrackColumn::Field(*name);
            if self
                .field_columns
                .borrow()
                .iter()
                .any(|(c, _)| *c == column)
            {
                continue;
            }
            let factory = Self::factory(Rc::downgrade(self), column);
            let c = gtk::ColumnViewColumn::new(Some(&column_title(column)), Some(factory));
            c.set_id(Some(column_id(column)));
            c.set_visible(false);
            c.set_resizable(true);
            c.set_fixed_width(FIELD_WIDTH);
            if self.backend.library().sortable(column) {
                c.set_sorter(Some(&gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal)));
            }
            let weak = Rc::downgrade(self);
            c.connect_visible_notify(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.mark_edges();
                    view.extras_changed();
                }
            });
            match self.labels_position() {
                Some(position) => self.table.insert_column(position, &c),
                None => self.table.append_column(&c),
            }
            self.column_actions
                .add_action(&gio::PropertyAction::new(name.id(), &c, "visible"));
            self.field_columns.borrow_mut().push((column, c));
        }
        // The menu lists the fields in their order.
        self.field_columns.borrow_mut().sort_by_key(|(column, _)| {
            names
                .iter()
                .position(|n| matches!(column, TrackColumn::Field(c) if c == n))
        });
        self.fields_section.remove_all();
        for name in &names {
            // A menu model item has no font of its own: the name is plain, with
            // its underscores doubled, as a single one marks a mnemonic.
            let item = gio::MenuItem::new(
                Some(&name.as_str().replace('_', "__")),
                Some(&format!("columns.{}", name.id())),
            );
            self.fields_section.append_item(&item);
        }
        self.fields.replace(fields);
        self.extras_changed();
        self.show_sort();
        if resort {
            self.reload();
        }
    }

    /// The position of the Labels column among the table's columns.
    fn labels_position(&self) -> Option<u32> {
        let (_, labels) = self
            .columns
            .iter()
            .find(|(column, _)| *column == TrackColumn::Labels)?;
        let columns = self.table.columns();
        (0..columns.n_items()).find(|i| {
            columns
                .item(*i)
                .and_downcast::<gtk::ColumnViewColumn>()
                .is_some_and(|c| c == *labels)
        })
    }

    /// The columns beyond the row that are shown now.
    fn wanted_extras(&self) -> ExtraColumns {
        ExtraColumns {
            labels: self
                .columns
                .iter()
                .any(|(column, c)| *column == TrackColumn::Labels && c.is_visible()),
            fields: self
                .field_columns
                .borrow()
                .iter()
                .filter(|(_, c)| c.is_visible())
                .filter_map(|(column, _)| match column {
                    TrackColumn::Field(name) => Some(*name),
                    _ => None,
                })
                .collect(),
        }
    }

    /// A column beyond the row was shown or hidden: the rows on screen come
    /// again with the values it needs.
    fn extras_changed(&self) {
        if self.model.set_extras(self.wanted_extras()) {
            self.refresh_rows();
        }
        self.mark_unsortable();
    }

    /// Shows the value of a column beyond the row; the labels with all of them
    /// in the tooltip, as the cell may cut them short.
    pub(super) fn show_extra_cell(&self, cell: &TableCell, track: &TrackItem, column: TrackColumn) {
        let extra = self.model.extra(track.key());
        let text = match (column, extra.as_deref()) {
            (TrackColumn::Labels, Some(extra)) => extra.labels.clone(),
            (TrackColumn::Field(name), Some(extra)) => {
                extra.field(name).unwrap_or_default().to_owned()
            }
            _ => String::new(),
        };
        if column == TrackColumn::Labels {
            cell.set_tooltip_text((!text.is_empty()).then_some(text.as_str()));
        }
        cell.set_text(&text);
    }

    /// Gives the header of each computed field the reason it cannot sort.
    pub(super) fn mark_unsortable(&self) {
        let Some(header) = self.table.first_child() else {
            return;
        };
        let columns = self.table.columns();
        let computed: Vec<FieldName> = self
            .fields
            .borrow()
            .iter()
            .filter(|f| f.is_computed())
            .map(|f| FieldName::new(&f.name))
            .collect();
        let mut title = header.first_child();
        let mut i = 0;
        while let Some(widget) = title {
            if let Some(c) = columns.item(i).and_downcast::<gtk::ColumnViewColumn>()
                && let Some(TrackColumn::Field(name)) = self.column_of(&c)
            {
                widget.set_tooltip_text(
                    computed
                        .contains(&name)
                        .then(|| tr("A computed field cannot sort the list."))
                        .as_deref(),
                );
            }
            title = widget.next_sibling();
            i += 1;
        }
    }

    /// The names of the user's fields the Columns menu offers (for tests).
    pub(crate) fn field_column_names(&self) -> Vec<String> {
        (0..self.fields_section.n_items())
            .filter_map(|i| {
                self.fields_section
                    .item_attribute_value(i, "action", Some(glib::VariantTy::STRING))
                    .and_then(|v| v.get::<String>())
            })
            .collect()
    }

    /// Shows or hides the column of `column` (for tests).
    pub(crate) fn set_column_shown(&self, column: TrackColumn, shown: bool) {
        if let Some((_, c)) = self.all_columns().into_iter().find(|(k, _)| *k == column) {
            c.set_visible(shown);
        }
    }

    /// The texts of the shown cells of `column`, by row, and the tooltip of its
    /// header when it has one (for tests).
    pub(crate) fn column_texts(&self, column: TrackColumn) -> (Vec<(u32, String)>, Option<String>) {
        let mut texts: Vec<(u32, String)> = self
            .cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
            .filter(|cell| cell.widget_name() == column_id(column) && cell.is_mapped())
            .filter_map(|cell| Some((cell.position()?, cell.shown_text())))
            .collect();
        texts.sort();
        texts.dedup();
        let header = self.column_header(column);
        (
            texts,
            header.and_then(|h| h.tooltip_text()).map(|t| t.to_string()),
        )
    }

    /// Whether the header of `column` sorts (for tests).
    pub(crate) fn column_sorts(&self, column: TrackColumn) -> bool {
        self.all_columns()
            .into_iter()
            .find(|(k, _)| *k == column)
            .is_some_and(|(_, c)| c.sorter().is_some())
    }

    /// The Columns button of the toolbar (for screenshots).
    pub(crate) fn columns_button(&self) -> Option<gtk::MenuButton> {
        let toolbar = self.root.first_child()?;
        let mut child = toolbar.first_child();
        while let Some(widget) = child {
            if let Some(button) = widget.downcast_ref::<gtk::MenuButton>()
                && button.label().as_deref() == Some(tr("Columns").as_str())
            {
                return Some(button.clone());
            }
            child = widget.next_sibling();
        }
        None
    }

    fn column_header(&self, column: TrackColumn) -> Option<gtk::Widget> {
        let header = self.table.first_child()?;
        let columns = self.table.columns();
        let mut title = header.first_child();
        let mut i = 0;
        while let Some(widget) = title {
            if let Some(c) = columns.item(i).and_downcast::<gtk::ColumnViewColumn>()
                && self.column_of(&c) == Some(column)
            {
                return Some(widget);
            }
            title = widget.next_sibling();
            i += 1;
        }
        None
    }
}
