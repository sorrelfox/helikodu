// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Insert popover beside every pattern and expression entry: a search
//! entry with the focus, then the fields, the user's fields and the functions
//! (each function with an example and what it gives), and a link to the
//! chapter on expressions in the user guide. Choosing a row inserts it at the
//! cursor and closes the popover: a field as `{name}` (or `name` inside
//! braces), a function as its call with its first argument selected, so that
//! it can be typed over.
//!
//! The user's fields are those in use when the popover opens, so a field saved
//! in Your fields is offered at once.

pub(crate) mod catalogue;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::i18n::tr;
use crate::widgets::label;
use catalogue::{Choice, Insert};

/// The text a choice goes into.
#[derive(Debug, Clone)]
pub(crate) enum InsertTarget {
    /// A one-line entry (any [`gtk::Editable`]).
    Editable(gtk::Editable),
    /// A text view of several lines.
    TextView(gtk::TextView),
}

/// The text of a target, where its cursor is and what is selected, in
/// characters.
#[derive(Debug, Clone, Default)]
struct Place {
    text: String,
    start: usize,
    end: usize,
}

impl InsertTarget {
    /// The target for `entry`.
    pub(crate) fn entry(entry: &impl IsA<gtk::Editable>) -> Self {
        Self::Editable(entry.clone().upcast())
    }

    fn place(&self) -> Place {
        match self {
            Self::Editable(editable) => {
                let (start, end) = editable.selection_bounds().unwrap_or_else(|| {
                    let at = editable.position();
                    (at, at)
                });
                Place {
                    text: editable.text().to_string(),
                    start: usize::try_from(start.min(end)).unwrap_or(0),
                    end: usize::try_from(start.max(end)).unwrap_or(0),
                }
            }
            Self::TextView(view) => {
                let buffer = view.buffer();
                let (start, end) = buffer.selection_bounds().unwrap_or_else(|| {
                    let at = buffer.iter_at_mark(&buffer.get_insert());
                    (at, at)
                });
                Place {
                    text: buffer
                        .text(&buffer.start_iter(), &buffer.end_iter(), false)
                        .to_string(),
                    start: usize::try_from(start.offset()).unwrap_or(0),
                    end: usize::try_from(end.offset()).unwrap_or(0),
                }
            }
        }
    }

    /// Replaces `place`'s selection by `text`, then selects the part `select`
    /// of it (in characters from its start), or puts the cursor after it, and
    /// gives the target the focus.
    fn put(&self, place: &Place, text: &str, select: Option<(usize, usize)>) {
        let count = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
        let length = text.chars().count();
        let (from, to) = select.unwrap_or((length, length));
        match self {
            Self::Editable(editable) => {
                if let Some(entry) = editable.downcast_ref::<gtk::Entry>() {
                    entry.grab_focus_without_selecting();
                } else if let Some(text) = editable.downcast_ref::<gtk::Text>() {
                    text.grab_focus_without_selecting();
                } else {
                    editable.grab_focus();
                }
                editable.delete_text(count(place.start), count(place.end));
                let mut at = count(place.start);
                editable.insert_text(text, &mut at);
                editable.select_region(count(place.start + from), count(place.start + to));
            }
            Self::TextView(view) => {
                view.grab_focus();
                let buffer = view.buffer();
                let mut start = buffer.iter_at_offset(count(place.start));
                let mut end = buffer.iter_at_offset(count(place.end));
                buffer.delete(&mut start, &mut end);
                let mut at = buffer.iter_at_offset(count(place.start));
                buffer.insert(&mut at, text);
                let a = buffer.iter_at_offset(count(place.start + from));
                let b = buffer.iter_at_offset(count(place.start + to));
                buffer.select_range(&b, &a);
            }
        }
    }
}

/// What the popover of one button keeps.
struct State {
    target: InsertTarget,
    /// The target as it was when the popover opened.
    place: RefCell<Place>,
    popover: gtk::Popover,
    search: gtk::SearchEntry,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    nothing: gtk::Label,
    /// The choices of the rows, by their index in the list; `None` for a
    /// section label.
    choices: RefCell<Vec<Option<Choice>>>,
}

/// The "Insert" button with a chevron and its popover, for `target`.
pub(crate) fn insert_button(target: InsertTarget) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&gtk::Label::new(Some(&tr("Insert"))));
    content.append(&gtk::Image::from_icon_name("pan-down-symbolic"));
    button.set_child(Some(&content));
    button.add_css_class("hk-menu-btn");
    button.add_css_class("hk-insert-button");
    button.set_valign(gtk::Align::Start);
    button.update_property(&[gtk::accessible::Property::Label(&tr(
        "Insert a field or a function",
    ))]);
    let state = Rc::new(build(target));
    state.connect();
    button.set_popover(Some(&state.popover));
    // The button keeps the state for as long as it lives.
    button.set_create_popup_func(move |_| {
        state.place.replace(state.target.place());
        state.search.set_text("");
        state.fill("");
    });
    button
}

fn build(target: InsertTarget) -> State {
    let popover = gtk::Popover::new();
    popover.add_css_class("hk-insert");
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_halign(gtk::Align::End);
    popover.update_property(&[gtk::accessible::Property::Label(&tr(
        "Insert a field or a function",
    ))]);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_size_request(440, -1);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&tr("Find a field or a function")));
    search.update_property(&[gtk::accessible::Property::Label(&tr(
        "Find a field or a function",
    ))]);
    search.add_css_class("hk-input");
    search.add_css_class("hk-insert-search");
    search.set_widget_name("insert-search");
    content.append(&search);
    let list = gtk::ListBox::new();
    list.add_css_class("hk-insert-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_widget_name("insert-list");
    let nothing = label(&tr("No field or function has this name."), &["hk-t3"]);
    nothing.add_css_class("hk-insert-nothing");
    nothing.set_visible(false);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&list);
    column.append(&nothing);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_min_content_height(452);
    scroller.set_max_content_height(452);
    scroller.set_child(Some(&column));
    content.append(&scroller);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let guide = gtk::Button::new();
    guide.add_css_class("flat");
    guide.add_css_class("hk-insert-guide");
    guide.set_widget_name("insert-guide");
    let guide_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    guide_content.append(&gtk::Image::from_icon_name("helikodu-book-symbolic"));
    guide_content.append(&label(&tr("Expressions in the user guide"), &["hk-t2"]));
    guide.set_child(Some(&guide_content));
    content.append(&guide);
    popover.set_child(Some(&content));
    let state = State {
        target,
        place: RefCell::new(Place::default()),
        popover: popover.clone(),
        search: search.clone(),
        list: list.clone(),
        scroller: scroller.clone(),
        nothing,
        choices: RefCell::new(Vec::new()),
    };
    let weak_popover = popover.downgrade();
    guide.connect_clicked(move |button| {
        let window = button.root().and_downcast::<gtk::Window>();
        if let Some(popover) = weak_popover.upgrade() {
            popover.popdown();
        }
        if let Some(window) = window {
            crate::dialogs::user_guide::open_chapter(&window, "expressions");
        }
    });
    state
}

impl State {
    /// Fills the list with the choices that match `typed`.
    fn fill(&self, typed: &str) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let user = helikodu_model::user_fields::current();
        let sections = [
            (tr("Fields"), catalogue::fields(), true),
            (
                tr("Your fields"),
                catalogue::user_fields(user.definitions()),
                false,
            ),
            (tr("Functions"), catalogue::functions(), true),
        ];
        let mut choices = Vec::new();
        for (title, all, counted) in sections {
            let found: Vec<Choice> = all
                .into_iter()
                .filter(|c| catalogue::matches(c, typed))
                .collect();
            if found.is_empty() {
                continue;
            }
            self.list
                .append(&section_row(&title, counted.then_some(found.len())));
            choices.push(None);
            for choice in found {
                self.list.append(&choice_row(&choice));
                choices.push(Some(choice));
            }
        }
        let empty = choices.is_empty();
        self.choices.replace(choices);
        self.nothing.set_visible(empty);
        self.list.set_visible(!empty);
        self.select_next(-1, 1);
    }

    /// Wires the search and the list, once.
    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |search| {
            if let Some(state) = weak.upgrade() {
                state.fill(&search.text());
            }
        });
        let weak = Rc::downgrade(self);
        self.search.connect_activate(move |_| {
            if let Some(state) = weak.upgrade()
                && let Some(row) = state.list.selected_row()
            {
                state.choose(row.index());
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(state) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let from = state.list.selected_row().map_or(-1, |r| r.index());
            match key {
                gdk::Key::Down => state.select_next(from, 1),
                gdk::Key::Up => state.select_next(from, -1),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.search.add_controller(keys);
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(state) = weak.upgrade() {
                state.choose(row.index());
            }
        });
        let weak = Rc::downgrade(self);
        self.popover.connect_map(move |_| {
            if let Some(state) = weak.upgrade() {
                state.search.grab_focus();
            }
        });
    }

    /// Selects the next row that holds a choice from `from` in `step`.
    fn select_next(&self, from: i32, step: i32) {
        let choices = self.choices.borrow();
        let mut at = from + step;
        while at >= 0 && (at as usize) < choices.len() {
            if choices[at as usize].is_some() {
                if let Some(row) = self.list.row_at_index(at) {
                    self.list.select_row(Some(&row));
                    self.scroll_to(&row);
                }
                return;
            }
            at += step;
        }
    }

    /// Scrolls the list so that `row` is seen whole.
    fn scroll_to(&self, row: &gtk::ListBoxRow) {
        let Some(bounds) = row.compute_bounds(&self.list) else {
            return;
        };
        let adjustment = self.scroller.vadjustment();
        let (top, bottom) = (
            f64::from(bounds.y()),
            f64::from(bounds.y() + bounds.height()),
        );
        let page = adjustment.page_size();
        if top < adjustment.value() {
            adjustment.set_value(top);
        } else if bottom > adjustment.value() + page {
            adjustment.set_value(bottom - page);
        }
    }

    /// Inserts the choice of the row at `index` and closes the popover.
    fn choose(&self, index: i32) {
        let Some(choice) = usize::try_from(index)
            .ok()
            .and_then(|i| self.choices.borrow().get(i).cloned().flatten())
        else {
            return;
        };
        let place = self.place.borrow().clone();
        let (text, select) = catalogue::insertion(&place.text, place.start, &choice.insert);
        self.popover.popdown();
        let target = self.target.clone();
        glib::idle_add_local_once(move || target.put(&place, &text, select));
    }
}

/// A section label with its count at the right.
fn section_row(title: &str, count: Option<usize>) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    row.set_selectable(false);
    row.set_focusable(false);
    row.add_css_class("hk-insert-section");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = label(title, &["hk-section-label"]);
    name.set_hexpand(true);
    name.set_accessible_role(gtk::AccessibleRole::Heading);
    content.append(&name);
    if let Some(count) = count {
        content.append(&label(&count.to_string(), &["hk-t3", "hk-insert-count"]));
    }
    row.set_child(Some(&content));
    row
}

/// A row of a field (28 high: the name in mono, what it holds) or of a
/// function (44 high: the call and an example, what it gives under them).
fn choice_row(choice: &Choice) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("hk-insert-row");
    row.update_property(&[
        gtk::accessible::Property::Label(&choice.name),
        gtk::accessible::Property::Description(&choice.description),
    ]);
    match choice.insert {
        Insert::Field(_) => {
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let name = label(&choice.name, &["hk-mono", "hk-insert-name"]);
            name.set_size_request(104, -1);
            content.append(&name);
            let description = label(&choice.description, &["hk-t2"]);
            description.set_hexpand(true);
            content.append(&description);
            row.set_child(Some(&content));
        }
        Insert::Function(_) => {
            row.add_css_class("function");
            let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let call = label(&choice.name, &["hk-mono", "hk-insert-name"]);
            call.set_hexpand(true);
            top.append(&call);
            let example = label(&choice.example, &["hk-mono", "hk-t3", "hk-insert-example"]);
            example.set_ellipsize(gtk::pango::EllipsizeMode::None);
            top.append(&example);
            content.append(&top);
            content.append(&label(&choice.description, &["hk-t2", "hk-insert-what"]));
            row.set_child(Some(&content));
        }
    }
    row
}
