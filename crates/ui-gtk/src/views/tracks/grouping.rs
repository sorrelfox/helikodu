// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tracks grouped by a field: the "Group by" button before Columns, its menu
//! (no groups; album artist, album, genre, year, format, label; the user's
//! fields; an expression), and the group rows of the table.
//!
//! The library makes the groups ([`LibrarySource::group_tracks`]): the table
//! asks for its tracks in the order its columns sort them, and the library puts
//! them in groups ordered by their values, the tracks without a value first. The
//! model holds a group row before the tracks of each open group
//! ([`group_key`]); opening or closing a group splices its tracks in or out.
//! Activating a group row (double click, Enter) opens or closes it, as do Left
//! and Right; its context menu acts on all its tracks.
//!
//! [`LibrarySource::group_tracks`]: crate::backend::LibrarySource::group_tracks

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use helikodu_model::expression::Template;

use super::TracksView;
use crate::backend::{TrackGroup, TrackGrouping, TrackKey};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::{GroupHeader, group_index, group_key};
use crate::views::table::TableCell;
use crate::widgets::label;

/// The fields "Insert" offers in the expression popover, by their names.
const INSERT_FIELDS: &[&str] = &[
    "albumartist",
    "artist",
    "album",
    "title",
    "year",
    "genre",
    "composer",
    "codec",
    "label",
];

/// The menu's name of a built-in choice.
fn choice_name(by: &TrackGrouping) -> String {
    match by {
        TrackGrouping::AlbumArtist => tr("Album artist"),
        TrackGrouping::Album => tr("Album"),
        TrackGrouping::Genre => tr("Genre"),
        TrackGrouping::Year => tr("Year"),
        TrackGrouping::Format => tr("Format"),
        TrackGrouping::Label => tr("Label"),
        TrackGrouping::Field(name) | TrackGrouping::Expression(name) => name.clone(),
    }
}

/// The status bar's words for a list grouped by `by`.
fn status_text(by: &TrackGrouping) -> String {
    match by {
        TrackGrouping::AlbumArtist => tr("Grouped by album artist"),
        TrackGrouping::Album => tr("Grouped by album"),
        TrackGrouping::Genre => tr("Grouped by genre"),
        TrackGrouping::Year => tr("Grouped by year"),
        TrackGrouping::Format => tr("Grouped by format"),
        TrackGrouping::Label => tr("Grouped by label"),
        TrackGrouping::Field(name) | TrackGrouping::Expression(name) => {
            fill(&tr("Grouped by {}"), &[name])
        }
    }
}

/// The state of the menu's radio items for `by`.
fn target(by: Option<&TrackGrouping>) -> String {
    match by {
        None => String::new(),
        Some(TrackGrouping::Field(name)) => format!("field:{name}"),
        Some(TrackGrouping::Expression(text)) => format!("expression:{text}"),
        Some(built_in) => built_in.field_name().to_owned(),
    }
}

/// The choice of a radio item's target.
fn choice(target: &str) -> Option<TrackGrouping> {
    if let Some(name) = target.strip_prefix("field:") {
        return Some(TrackGrouping::Field(name.to_owned()));
    }
    if let Some(text) = target.strip_prefix("expression:") {
        return Some(TrackGrouping::Expression(text.to_owned()));
    }
    TrackGrouping::BUILT_IN
        .into_iter()
        .find(|b| b.field_name() == target)
}

/// The table's rows for `groups`: each group's row, and its tracks unless it
/// is closed; and the headers of the groups.
fn rows_of(
    groups: &[TrackGroup],
    closed: &HashSet<String>,
) -> (Vec<TrackKey>, Vec<Rc<GroupHeader>>) {
    let mut rows =
        Vec::with_capacity(groups.len() + groups.iter().map(|g| g.keys.len()).sum::<usize>());
    let mut headers = Vec::with_capacity(groups.len());
    for (index, group) in groups.iter().enumerate() {
        let open = !closed.contains(&group.value.to_lowercase());
        headers.push(Rc::new(header(group, open, index == 0)));
        rows.push(group_key(index));
        if open {
            rows.extend_from_slice(&group.keys);
        }
    }
    (rows, headers)
}

/// The header of `group`.
fn header(group: &TrackGroup, open: bool, first: bool) -> GroupHeader {
    let n = group.keys.len();
    GroupHeader {
        value: if group.value.is_empty() {
            tr("No value")
        } else {
            group.value.clone()
        },
        no_value: group.value.is_empty(),
        count: fill(&ntr("{} track", "{} tracks", n), &[&format::count(n)]),
        open,
        first,
    }
}

/// The grouping of a track view: its button and menu, what it is grouped by,
/// and the groups of the last load.
pub(super) struct Grouping {
    pub(super) button: gtk::MenuButton,
    lead: gtk::Label,
    value: gtk::Label,
    menu: gio::Menu,
    by_action: gio::SimpleAction,
    by: RefCell<Option<TrackGrouping>>,
    groups: RefCell<Vec<TrackGroup>>,
    /// The groups closed, by their values in lower case.
    closed: RefCell<HashSet<String>>,
    fields: RefCell<Vec<String>>,
    expression: RefCell<Option<gtk::Popover>>,
}

impl std::fmt::Debug for Grouping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grouping")
            .field("by", &self.by)
            .finish_non_exhaustive()
    }
}

impl Grouping {
    pub(super) fn new() -> Self {
        let button = gtk::MenuButton::new();
        button.add_css_class("hk-sort");
        button.add_css_class("hk-group-by");
        button.set_valign(gtk::Align::Center);
        button.set_always_show_arrow(true);
        button.set_tooltip_text(Some(&tr("Group the tracks by a field")));
        button.set_widget_name("group-by");
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let lead = label(&tr("Group"), &[]);
        lead.set_ellipsize(gtk::pango::EllipsizeMode::None);
        content.append(&lead);
        let value = label("", &[]);
        value.set_max_width_chars(16);
        value.set_visible(false);
        content.append(&value);
        button.set_child(Some(&content));
        button.update_property(&[gtk::accessible::Property::Label(&tr("Group by"))]);
        let by_action = gio::SimpleAction::new_stateful(
            "by",
            Some(glib::VariantTy::STRING),
            &String::new().to_variant(),
        );
        Self {
            button,
            lead,
            value,
            menu: gio::Menu::new(),
            by_action,
            by: RefCell::new(None),
            groups: RefCell::new(Vec::new()),
            closed: RefCell::new(HashSet::new()),
            fields: RefCell::new(Vec::new()),
            expression: RefCell::new(None),
        }
    }

    /// What the list is grouped by.
    pub(super) fn by(&self) -> Option<TrackGrouping> {
        self.by.borrow().clone()
    }

    /// Fills the menu: the fixed choices, the user's fields, an expression.
    fn fill_menu(&self) {
        let menu = &self.menu;
        menu.remove_all();
        let first = gio::Menu::new();
        let item = gio::MenuItem::new(Some(&tr("No groups")), None);
        item.set_action_and_target_value(Some("grouping.by"), Some(&"".to_variant()));
        first.append_item(&item);
        menu.append_section(None, &first);
        let fixed = gio::Menu::new();
        for by in TrackGrouping::BUILT_IN {
            let item = gio::MenuItem::new(Some(&choice_name(&by)), None);
            item.set_action_and_target_value(
                Some("grouping.by"),
                Some(&target(Some(&by)).to_variant()),
            );
            fixed.append_item(&item);
        }
        menu.append_section(None, &fixed);
        let fields = self.fields.borrow();
        if !fields.is_empty() {
            let own = gio::Menu::new();
            for name in fields.iter() {
                let item = gio::MenuItem::new(Some(name), None);
                item.set_action_and_target_value(
                    Some("grouping.by"),
                    Some(&target(Some(&TrackGrouping::Field(name.clone()))).to_variant()),
                );
                own.append_item(&item);
            }
            menu.append_section(Some(&tr("Your fields")), &own);
        }
        let last = gio::Menu::new();
        last.append(Some(&tr("An expression…")), Some("grouping.expression"));
        menu.append_section(None, &last);
    }

    /// Shows the choice on the button: "Group" alone without one.
    fn show_choice(&self) {
        let by = self.by.borrow();
        match by.as_ref() {
            None => {
                self.lead.set_text(&tr("Group"));
                self.lead.remove_css_class("hk-t2");
                self.value.set_visible(false);
            }
            Some(by) => {
                self.lead.set_text(&tr("Group by"));
                self.lead.add_css_class("hk-t2");
                self.value.set_text(&choice_name(by));
                let mono = matches!(by, TrackGrouping::Field(_) | TrackGrouping::Expression(_));
                if mono {
                    self.value.add_css_class("hk-mono");
                    self.value.add_css_class("hk-group-by-field");
                } else {
                    self.value.remove_css_class("hk-mono");
                    self.value.remove_css_class("hk-group-by-field");
                }
                self.value.set_visible(true);
            }
        }
        self.by_action.set_state(&target(by.as_ref()).to_variant());
    }
}

impl TracksView {
    /// Sets up the grouping: its actions, menu, keys and redrawing.
    pub(super) fn connect_grouping(self: &Rc<Self>) {
        let grouping = &self.grouping;
        grouping.fill_menu();
        crate::widgets::menu_names::set_menu_later(&grouping.button, &grouping.menu);
        let group = gio::SimpleActionGroup::new();
        let weak = Rc::downgrade(self);
        grouping.by_action.connect_activate(move |_, value| {
            let Some(view) = weak.upgrade() else { return };
            let by = value
                .and_then(|v| v.str().map(str::to_owned))
                .and_then(|t| choice(&t));
            view.set_grouping(by);
        });
        group.add_action(&grouping.by_action);
        let expression = gio::SimpleAction::new("expression", None);
        let weak = Rc::downgrade(self);
        expression.connect_activate(move |_, _| {
            let Some(view) = weak.upgrade() else { return };
            // After the menu has closed.
            glib::idle_add_local_once(move || view.show_expression());
        });
        group.add_action(&expression);
        let insert = gio::SimpleAction::new("insert", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        insert.connect_activate(move |_, value| {
            let (Some(view), Some(name)) = (
                weak.upgrade(),
                value.and_then(|v| v.str().map(str::to_owned)),
            ) else {
                return;
            };
            view.insert_field(&name);
        });
        group.add_action(&insert);
        self.root.insert_action_group("grouping", Some(&group));

        // Left and Right close and open the focused group row.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let open = match key {
                gdk::Key::Left | gdk::Key::KP_Left => false,
                gdk::Key::Right | gdk::Key::KP_Right => true,
                _ => return glib::Propagation::Proceed,
            };
            if !state.is_empty() {
                return glib::Propagation::Proceed;
            }
            if view.group_key_pressed(open) {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.table.add_controller(keys);

        // The group rows are drawn where they lie in the visible area: again
        // when it scrolls sideways or the columns change.
        if let Some(adjustment) = self.table.hadjustment() {
            let weak = Rc::downgrade(self);
            adjustment.connect_value_changed(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.redraw_group_rows();
                }
            });
        }
        for (_, column) in &self.columns {
            for property in ["fixed-width", "visible"] {
                let weak = Rc::downgrade(self);
                column.connect_notify_local(Some(property), move |_, _| {
                    if let Some(view) = weak.upgrade() {
                        view.redraw_group_rows();
                    }
                });
            }
        }
        let weak = Rc::downgrade(self);
        self.table
            .columns()
            .connect_items_changed(move |_, _, _, _| {
                if let Some(view) = weak.upgrade() {
                    view.redraw_group_rows();
                }
            });
        self.load_group_fields();
    }

    /// Reads the user's fields for the menu again.
    pub(super) fn load_group_fields(self: &Rc<Self>) {
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let fields = backend
                .query(|library| library.group_fields())
                .await
                .unwrap_or_default();
            let Some(view) = weak.upgrade() else { return };
            if *view.grouping.fields.borrow() != fields {
                view.grouping.fields.replace(fields);
                view.grouping.fill_menu();
            }
        });
    }

    /// Groups the list by `by`, or not for `None`.
    pub(crate) fn set_grouping(self: &Rc<Self>, by: Option<TrackGrouping>) {
        if *self.grouping.by.borrow() == by {
            self.grouping.show_choice();
            return;
        }
        self.grouping.by.replace(by);
        self.grouping.closed.borrow_mut().clear();
        self.grouping.show_choice();
        self.show_selection();
        self.reload();
    }

    /// The rows to show for the tracks `keys` of a load and their `groups`,
    /// when the list is grouped; the model takes the groups' headers.
    pub(super) fn grouped_rows(
        &self,
        keys: Vec<TrackKey>,
        groups: Option<Vec<TrackGroup>>,
    ) -> Vec<TrackKey> {
        let Some(groups) = groups.filter(|_| self.grouping.by().is_some()) else {
            self.grouping.groups.borrow_mut().clear();
            self.model.set_groups(Vec::new());
            return keys;
        };
        let (rows, headers) = rows_of(&groups, &self.grouping.closed.borrow());
        self.grouping.groups.replace(groups);
        self.model.set_groups(headers);
        rows
    }

    /// The status bar's text while nothing is selected.
    pub(super) fn grouping_status(&self) -> String {
        self.grouping
            .by()
            .map(|by| status_text(&by))
            .unwrap_or_default()
    }

    /// `keys` with each group row replaced by all the tracks of its group.
    pub(super) fn with_group_tracks(&self, keys: Vec<TrackKey>) -> Vec<TrackKey> {
        if !keys.iter().any(|k| group_index(*k).is_some()) {
            return keys;
        }
        let groups = self.grouping.groups.borrow();
        let mut out = Vec::with_capacity(keys.len());
        let mut seen = HashSet::new();
        for key in keys {
            let tracks: &[TrackKey] = match group_index(key) {
                Some(index) => groups.get(index).map_or(&[], |g| g.keys.as_slice()),
                None => std::slice::from_ref(&key),
            };
            for track in tracks {
                if seen.insert(*track) {
                    out.push(*track);
                }
            }
        }
        out
    }

    /// Opens or closes the group whose row is at `position`, if it is one;
    /// whether it was a group row.
    pub(super) fn toggle_group_at(&self, position: u32) -> bool {
        let Some(open) = self.group_header_at(position).map(|h| h.open) else {
            return false;
        };
        self.set_group_open(position, !open);
        true
    }

    /// The header of the group row at `position`.
    fn group_header_at(&self, position: u32) -> Option<GroupHeader> {
        let key = *self.model.keys().get(position as usize)?;
        let index = group_index(key)?;
        let groups = self.grouping.groups.borrow();
        let group = groups.get(index)?;
        let open = !self
            .grouping
            .closed
            .borrow()
            .contains(&group.value.to_lowercase());
        Some(header(group, open, index == 0))
    }

    /// Opens or closes the group whose row is at `position`, keeping the row
    /// focused and selected.
    pub(super) fn set_group_open(&self, position: u32, open: bool) {
        let Some(before) = self.group_header_at(position) else {
            return;
        };
        if before.open == open {
            return;
        }
        let Some(index) = self
            .model
            .keys()
            .get(position as usize)
            .and_then(|k| group_index(*k))
        else {
            return;
        };
        let (value, tracks) = {
            let groups = self.grouping.groups.borrow();
            let Some(group) = groups.get(index) else {
                return;
            };
            (group.value.to_lowercase(), group.keys.clone())
        };
        if open {
            self.grouping.closed.borrow_mut().remove(&value);
        } else {
            self.grouping.closed.borrow_mut().insert(value);
        }
        let header = Rc::new(GroupHeader { open, ..before });
        self.model.set_group(index, header);
        let removed = if open { 0 } else { tracks.len() };
        let mut added = vec![group_key(index)];
        if open {
            added.extend_from_slice(&tracks);
        }
        self.model.splice_keys(
            position,
            u32::try_from(removed + 1).unwrap_or(u32::MAX),
            &added,
        );
        self.table.scroll_to(
            position,
            None,
            gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT,
            None,
        );
    }

    /// Opens (Right) or closes (Left, mirrored right to left) the focused
    /// group row; whether a group row has the focus.
    pub(crate) fn group_key_pressed(&self, right: bool) -> bool {
        if self.grouping.by().is_none() {
            return false;
        }
        let rtl = self.table.direction() == gtk::TextDirection::Rtl;
        let Some(position) = self.focused_group_row() else {
            return false;
        };
        self.set_group_open(position, right != rtl);
        true
    }

    /// The position of the focused row, if it is a group row.
    fn focused_group_row(&self) -> Option<u32> {
        let focus = self.table.root()?.focus()?;
        if !focus.is_ancestor(&self.table) {
            return None;
        }
        let mut pending = vec![focus];
        while let Some(widget) = pending.pop() {
            if let Some(cell) = widget.downcast_ref::<TableCell>() {
                return cell.group().and(cell.position());
            }
            let mut child = widget.first_child();
            while let Some(c) = child {
                child = c.next_sibling();
                pending.push(c);
            }
        }
        None
    }

    /// Draws the shown group rows again.
    fn redraw_group_rows(&self) {
        for cell in self
            .cells
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
        {
            if cell.group().is_some() {
                cell.queue_draw();
            }
        }
    }

    /// The popover of "An expression…": the expression box, "Insert", and a
    /// line that says how to use it or why it cannot be read.
    fn show_expression(self: &Rc<Self>) {
        let popover = self
            .grouping
            .expression
            .borrow()
            .clone()
            .unwrap_or_else(|| self.build_expression());
        if let Some(entry) = popover
            .child()
            .and_then(|c| c.first_child())
            .and_then(|c| c.first_child())
        {
            if let (Some(TrackGrouping::Expression(text)), Some(entry)) =
                (self.grouping.by(), entry.downcast_ref::<gtk::Entry>())
            {
                entry.set_text(&text);
            }
            entry.grab_focus();
        }
        popover.popup();
    }

    fn build_expression(self: &Rc<Self>) -> gtk::Popover {
        let popover = gtk::Popover::new();
        popover.add_css_class("hk-group-expression");
        popover.set_widget_name("group-expression");
        popover.set_parent(&self.grouping.button);
        popover.set_position(gtk::PositionType::Bottom);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.add_css_class("hk-mono");
        entry.set_width_chars(28);
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("{left(year, 3)}0s"));
        entry.update_property(&[gtk::accessible::Property::Label(&tr("Expression"))]);
        line.append(&entry);
        let menu = gio::Menu::new();
        for name in INSERT_FIELDS {
            let item = gio::MenuItem::new(Some(name), None);
            item.set_action_and_target_value(Some("grouping.insert"), Some(&(*name).to_variant()));
            menu.append_item(&item);
        }
        let fields = self.grouping.fields.borrow();
        if !fields.is_empty() {
            let own = gio::Menu::new();
            for name in fields.iter() {
                let item = gio::MenuItem::new(Some(name), None);
                item.set_action_and_target_value(Some("grouping.insert"), Some(&name.to_variant()));
                own.append_item(&item);
            }
            menu.append_section(Some(&tr("Your fields")), &own);
        }
        let insert = gtk::MenuButton::new();
        insert.set_label(&tr("Insert"));
        insert.add_css_class("hk-menu-btn");
        insert.set_always_show_arrow(true);
        crate::widgets::menu_names::set_menu_later(&insert, &menu);
        line.append(&insert);
        body.append(&line);
        let note = crate::widgets::wrapped_label(
            &tr("Press Enter to group the tracks by what the expression gives."),
            &["hk-t3", "hk-secondary"],
        );
        note.set_widget_name("group-expression-note");
        note.set_max_width_chars(44);
        body.append(&note);
        popover.set_child(Some(&body));
        let weak = Rc::downgrade(self);
        let weak_popover = popover.downgrade();
        entry.connect_activate(move |entry| {
            let Some(view) = weak.upgrade() else { return };
            let text = entry.text().trim().to_owned();
            if text.is_empty() {
                return;
            }
            match Template::parse(&text) {
                Ok(_) => {
                    if let Some(popover) = weak_popover.upgrade() {
                        popover.popdown();
                    }
                    view.set_grouping(Some(TrackGrouping::Expression(text)));
                }
                Err(error) => note.set_text(&error.to_string()),
            }
        });
        self.grouping.expression.replace(Some(popover.clone()));
        popover
    }

    /// Inserts the field `name` at the cursor of the expression box.
    fn insert_field(&self, name: &str) {
        let Some(entry) = self
            .grouping
            .expression
            .borrow()
            .as_ref()
            .and_then(|p| p.child())
            .and_then(|c| c.first_child())
            .and_then(|c| c.first_child())
            .and_downcast::<gtk::Entry>()
        else {
            return;
        };
        let mut position = entry.position();
        entry.insert_text(&format!("{{{name}}}"), &mut position);
        entry.set_position(position);
        entry.grab_focus_without_selecting();
    }

    /// The group rows shown, as their headers read: value and count (for
    /// tests).
    pub(crate) fn group_rows(&self) -> Vec<(String, String, bool)> {
        self.model
            .keys()
            .iter()
            .enumerate()
            .filter(|(_, k)| group_index(**k).is_some())
            .filter_map(|(p, _)| {
                let header = self.group_header_at(u32::try_from(p).ok()?)?;
                Some((header.value, header.count, header.open))
            })
            .collect()
    }

    /// The position of the row of the group shown as `value` (for tests).
    pub(crate) fn group_row_position(&self, value: &str) -> Option<u32> {
        self.model
            .keys()
            .iter()
            .enumerate()
            .filter(|(_, k)| group_index(**k).is_some())
            .find_map(|(p, _)| {
                let p = u32::try_from(p).ok()?;
                (self.group_header_at(p)?.value == value).then_some(p)
            })
    }

    /// Activates the row of the group shown as `value`, as a double click
    /// or Enter does; whether there is one (for tests).
    pub(crate) fn demo_activate_group(&self, value: &str) -> bool {
        let Some(position) = self.group_row_position(value) else {
            return false;
        };
        self.table.emit_by_name::<()>("activate", &[&position]);
        true
    }

    /// Focuses and selects the row of the group shown as `value`; whether
    /// there is one (for tests).
    pub(crate) fn demo_focus_group(&self, value: &str) -> bool {
        let Some(position) = self.group_row_position(value) else {
            return false;
        };
        self.table.grab_focus();
        self.table.scroll_to(
            position,
            None,
            gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT,
            None,
        );
        true
    }

    /// The tracks the context menu acts on (for tests).
    pub(crate) fn selected_tracks(&self) -> Vec<TrackKey> {
        self.selected_keys()
    }

    /// The text of the "Group by" button (for tests).
    pub(crate) fn group_button_text(&self) -> String {
        let value = if self.grouping.value.is_visible() {
            format!(" {}", self.grouping.value.text())
        } else {
            String::new()
        };
        format!("{}{value}", self.grouping.lead.text())
    }

    /// The status bar's left text (for tests).
    pub(crate) fn status_text(&self) -> String {
        self.status_selection.text().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_round_trip_through_the_menu_targets() {
        for by in [
            TrackGrouping::AlbumArtist,
            TrackGrouping::Label,
            TrackGrouping::Field("decade".into()),
            TrackGrouping::Expression("{left(year,3)}0s".into()),
        ] {
            assert_eq!(choice(&target(Some(&by))), Some(by));
        }
        assert_eq!(choice(""), None);
    }

    #[test]
    fn closed_groups_keep_only_their_row() {
        let groups = vec![
            TrackGroup {
                value: String::new(),
                keys: vec![TrackKey(1)],
            },
            TrackGroup {
                value: "1990s".into(),
                keys: vec![TrackKey(2), TrackKey(3)],
            },
        ];
        let closed: HashSet<String> = ["1990s".to_owned()].into();
        let (rows, headers) = rows_of(&groups, &closed);
        assert_eq!(rows, [group_key(0), TrackKey(1), group_key(1)]);
        assert!(headers[0].no_value && headers[0].first && headers[0].open);
        assert!(!headers[1].open && !headers[1].first);
        assert_eq!(group_index(rows[2]), Some(1));
    }
}
