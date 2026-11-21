// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Labels popover: the labels of a selection of tracks, an album or an
//! artist, opened by "Labels…" in their context menus and by "Add label" on the
//! album and artist pages.
//!
//! - A section label names what it applies to; the entry under it has the focus,
//!   filters the list as the user types and gives the typed label on Enter.
//! - Each label has a check box: ticked when the whole selection has it, mixed
//!   when part of it has it (a click gives it to all, a second takes it from
//!   all), empty when none has it. A label the tracks have only from their album
//!   or artist is shown ticked and insensitive, with "from the album" or "from
//!   the artist": it is changed there.
//! - "New label “…”" gives the typed text while it is no label's name; "Show all
//!   labels" opens the Labels view.
//! - A click applies at once and the popover stays open. The first change shows
//!   an Undo toast that takes back everything done until the popover closes.
//!
//! Up and Down move between the entry and the rows; Space and Enter tick the
//! row with the focus; Escape closes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::backend::{
    Backend, LabelChoice, LabelChoices, LabelEdit, LabelFor, LabelState, LabelUndo,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;
use crate::window::labels::{Scope, SharedUndo, change_text};

thread_local! {
    /// The popover open now, if any.
    static OPEN: RefCell<Option<Rc<LabelsPopover>>> = const { RefCell::new(None) };
}

/// The popover open now (for tests).
pub(crate) fn current() -> Option<Rc<LabelsPopover>> {
    OPEN.with(|open| open.borrow().clone())
}

/// The Labels popover.
pub(crate) struct LabelsPopover {
    popover: gtk::Popover,
    entry: gtk::Entry,
    hint: gtk::Label,
    list: gtk::ListBox,
    backend: Backend,
    window: glib::WeakRef<MainWindow>,
    target: LabelFor,
    scope: Scope,
    choices: RefCell<LabelChoices>,
    /// What the toast of this opening undoes, and the toast's text.
    session: RefCell<Option<(SharedUndo, String)>>,
    generation: Cell<u64>,
    /// Whether it has been shown.
    shown: Cell<bool>,
}

impl std::fmt::Debug for LabelsPopover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelsPopover").finish_non_exhaustive()
    }
}

/// Opens the popover for `target`, which `scope` names, on `anchor`.
pub(crate) fn open(
    window: &MainWindow,
    target: LabelFor,
    scope: Scope,
    anchor: &gtk::Widget,
    at: Option<(f64, f64)>,
) {
    open_with(window, target, scope, anchor, at, true);
}

/// [`open`]; without `autohide` the popover stays open without a pointer or
/// keyboard grab, as tests without input need.
pub(crate) fn open_with(
    window: &MainWindow,
    target: LabelFor,
    scope: Scope,
    anchor: &gtk::Widget,
    at: Option<(f64, f64)>,
    autohide: bool,
) {
    let Some(backend) = window.backend() else {
        return;
    };
    if let Some(old) = current() {
        // One still loading never opens.
        old.shown.set(true);
        if old.popover.is_visible() {
            old.popover.popdown();
        } else if old.popover.parent().is_some() {
            old.popover.unparent();
        }
    }
    let popover = LabelsPopover::new(window, &backend, target, scope);
    popover.popover.set_autohide(autohide);
    popover.popover.set_parent(anchor);
    if let Some((x, y)) = at {
        popover
            .popover
            .set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }
    OPEN.with(|open| open.replace(Some(popover.clone())));
    // It opens once its rows are there, at its full size.
    popover.reload();
}

impl LabelsPopover {
    fn new(window: &MainWindow, backend: &Backend, target: LabelFor, scope: Scope) -> Rc<Self> {
        let popover = gtk::Popover::new();
        popover.add_css_class("hk-labels-popover");
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_halign(gtk::Align::Start);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.set_size_request(312 - 14, -1);
        let heading = label(&scope.heading(&target), &["hk-section-label"]);
        content.append(&heading);
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.set_placeholder_text(Some(&tr("Add a label")));
        entry.set_primary_icon_name(Some("helikodu-label-symbolic"));
        entry.set_max_length(100);
        entry.update_property(&[gtk::accessible::Property::Label(&tr("Add a label"))]);
        content.append(&entry);
        let hint = label(&tr("Type a label and press Enter."), &["hk-labels-hint"]);
        hint.set_visible(false);
        content.append(&hint);
        let list = gtk::ListBox::new();
        list.add_css_class("hk-label-choices");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.update_property(&[gtk::accessible::Property::Label(&tr("Labels"))]);
        list.set_header_func(|row, before| {
            // A line before "New label" and before "Show all labels".
            let name = row.widget_name();
            let line = before.is_some() && (name == "new" || name == "show-all");
            row.set_header(
                line.then(|| gtk::Separator::new(gtk::Orientation::Horizontal))
                    .as_ref(),
            );
        });
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height(380);
        scroller.set_child(Some(&list));
        content.append(&scroller);
        popover.set_child(Some(&content));

        let view = Rc::new(Self {
            popover,
            entry,
            hint,
            list,
            backend: backend.clone(),
            window: window.downgrade(),
            target,
            scope,
            choices: RefCell::new(LabelChoices::default()),
            session: RefCell::new(None),
            generation: Cell::new(0),
            shown: Cell::new(false),
        });
        view.connect();
        view
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.entry.connect_changed(move |entry| {
            entry.set_secondary_icon_name(
                (!entry.text().is_empty()).then_some("edit-clear-symbolic"),
            );
            if let Some(view) = weak.upgrade() {
                view.fill();
            }
        });
        self.entry.connect_icon_release(|entry, position| {
            if position == gtk::EntryIconPosition::Secondary {
                entry.set_text("");
            }
        });
        let weak = Rc::downgrade(self);
        self.entry.connect_activate(move |entry| {
            let Some(view) = weak.upgrade() else { return };
            let text = entry.text().trim().to_owned();
            if !text.is_empty() {
                view.give(&text);
                entry.set_text("");
            }
        });
        // Down moves into the rows; Up from the first row comes back.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if matches!(key, gdk::Key::Down | gdk::Key::KP_Down)
                && let Some(row) = view.first_row()
            {
                row.grab_focus();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.entry.add_controller(keys);
        let weak = Rc::downgrade(self);
        self.list.connect_keynav_failed(move |_, direction| {
            if direction == gtk::DirectionType::Up
                && let Some(view) = weak.upgrade()
            {
                view.entry.grab_focus();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(view) = weak.upgrade() {
                view.activate(row.widget_name().as_str());
            }
        });
        let weak = Rc::downgrade(self);
        self.popover.connect_closed(move |popover| {
            if let Some(view) = weak.upgrade() {
                // The toast keeps what it undoes; later changes get their own.
                view.session.replace(None);
            }
            OPEN.with(|open| {
                let mine = open
                    .borrow()
                    .as_ref()
                    .is_some_and(|o| o.popover == *popover);
                if mine {
                    open.replace(None);
                }
            });
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
    }

    fn first_row(&self) -> Option<gtk::ListBoxRow> {
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && row.is_visible()
                && row.is_sensitive()
            {
                return Some(row.clone());
            }
        }
        None
    }

    /// Reads the labels and their states again.
    fn reload(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let target = self.target.clone();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let choices = backend.query(move |l| l.label_choices(&target)).await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            match choices {
                Ok(choices) => {
                    view.choices.replace(choices);
                    view.fill();
                    if !view.shown.replace(true) {
                        view.popover.popup();
                        view.entry.grab_focus();
                    }
                }
                Err(error) => {
                    if let Some(window) = view.window.upgrade() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    /// Lists the labels that match the typed text.
    fn fill(self: &Rc<Self>) {
        let focused_row = self
            .popover
            .root()
            .and_then(|r| gtk::prelude::RootExt::focus(&r))
            .and_then(|f| {
                f.ancestor(gtk::ListBoxRow::static_type())
                    .map(|r| r.widget_name().to_string())
            });
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let typed = self.entry.text().trim().to_owned();
        let folded = typed.to_lowercase();
        let choices = self.choices.borrow().clone();
        self.hint.set_visible(choices.labels.is_empty());
        let mut exact = false;
        for choice in &choices.labels {
            let name = choice.name.to_lowercase();
            if !name.contains(&folded) {
                continue;
            }
            exact |= name == folded;
            self.list.append(&choice_row(choice, &folded));
        }
        if !typed.is_empty() && !exact {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name("new");
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let plus = gtk::Image::from_icon_name("list-add-symbolic");
            plus.set_pixel_size(14);
            line.append(&plus);
            let text = label(&fill(&tr("New label “{}”"), &[&typed]), &[]);
            text.set_hexpand(true);
            line.append(&text);
            line.append(&label(&tr("Enter"), &["hk-key", "hint"]));
            row.set_child(Some(&line));
            self.list.append(&row);
        }
        let row = gtk::ListBoxRow::new();
        row.set_widget_name("show-all");
        row.add_css_class("hk-show-all");
        row.set_child(Some(&label(&tr("Show all labels"), &[])));
        self.list.append(&row);
        self.list.invalidate_headers();
        if let Some(scroller) = self
            .list
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>()
        {
            crate::widgets::scroll_when_taller(&scroller);
        }
        // A popover shown keeps its size otherwise: it follows the rows.
        if self.popover.is_visible() {
            self.popover.present();
        }
        // The keyboard stays on the row it was on.
        if let Some(name) = focused_row {
            let mut child = self.list.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                if widget.widget_name() == name.as_str() {
                    widget.grab_focus();
                    break;
                }
            }
        }
    }

    fn activate(self: &Rc<Self>, name: &str) {
        match name {
            "new" => {
                let text = self.entry.text().trim().to_owned();
                if !text.is_empty() {
                    self.give(&text);
                    self.entry.set_text("");
                    self.entry.grab_focus();
                }
            }
            "show-all" => {
                self.popover.popdown();
                if let Some(window) = self.window.upgrade() {
                    window.go(
                        crate::window::Page::Library(crate::window::labels::PAGE),
                        true,
                    );
                }
            }
            _ => {
                let Some(key) = name
                    .strip_prefix("label:")
                    .and_then(|k| k.parse::<u64>().ok())
                else {
                    return;
                };
                let choice = self
                    .choices
                    .borrow()
                    .labels
                    .iter()
                    .find(|c| c.key.0 == key)
                    .cloned();
                if let Some(choice) = choice {
                    self.toggle(&choice);
                }
            }
        }
    }

    /// A click on a label: a ticked one is taken from all, a mixed or empty one
    /// given to all.
    fn toggle(self: &Rc<Self>, choice: &LabelChoice) {
        let edit = match choice.state {
            LabelState::All => LabelEdit::Take {
                target: self.target.clone(),
                name: choice.name.clone(),
            },
            LabelState::Some | LabelState::None => LabelEdit::Give {
                target: self.target.clone(),
                name: choice.name.clone(),
            },
            LabelState::FromAlbum | LabelState::FromArtist => return,
        };
        self.apply(edit);
    }

    fn give(self: &Rc<Self>, name: &str) {
        self.apply(LabelEdit::Give {
            target: self.target.clone(),
            name: name.to_owned(),
        });
    }

    fn apply(self: &Rc<Self>, edit: LabelEdit) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let text = change_text(&edit, &self.scope);
        let weak = Rc::downgrade(self);
        window.change_labels(edit, move |window, result| match result {
            Ok(done) => {
                if let Some(view) = weak.upgrade() {
                    view.note(window, done.undo, &text);
                    view.reload();
                } else {
                    window.toast_label_undo(&text, Rc::new(RefCell::new(done.undo)), None);
                }
            }
            Err(error) => window.label_problem(&error),
        });
    }

    /// Adds `undo` to what this opening's toast undoes, showing the toast for
    /// the first change and again when it has gone.
    fn note(self: &Rc<Self>, window: &MainWindow, undo: LabelUndo, text: &str) {
        let session = self.session.borrow().clone();
        let (shared, shown_text) = match session {
            Some((shared, shown)) => {
                shared.borrow_mut().absorb(undo);
                if window.toast_text().as_deref() == Some(shown.as_str()) {
                    return;
                }
                (shared, text.to_owned())
            }
            None => (Rc::new(RefCell::new(undo)), text.to_owned()),
        };
        self.session
            .replace(Some((shared.clone(), shown_text.clone())));
        let weak = Rc::downgrade(self);
        let after: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(view) = weak.upgrade() {
                // Undone: what follows is a change of its own.
                view.session.replace(None);
                view.reload();
            }
        });
        window.toast_label_undo(&shown_text, shared, Some(after));
    }

    /// The rows: name, the text at the right, the state and whether the row can
    /// be changed (for tests).
    pub(crate) fn rows(&self) -> Vec<(String, String, Option<LabelState>, bool)> {
        let choices = self.choices.borrow();
        let mut out = Vec::new();
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() else {
                continue;
            };
            let name = row.widget_name();
            let state = name
                .strip_prefix("label:")
                .and_then(|k| k.parse::<u64>().ok())
                .and_then(|k| choices.labels.iter().find(|c| c.key.0 == k))
                .map(|c| c.state);
            let texts: Vec<String> = descendants_texts(row.upcast_ref());
            let first = texts.first().cloned().unwrap_or_default();
            let last = if texts.len() > 1 {
                texts.last().cloned().unwrap_or_default()
            } else {
                String::new()
            };
            out.push((first, last, state, row.is_sensitive()));
        }
        out
    }

    /// Types `text` into the entry (for tests).
    pub(crate) fn type_text(&self, text: &str) {
        self.entry.set_text(text);
    }

    /// Presses Enter in the entry (for tests).
    pub(crate) fn press_enter(&self) {
        self.entry.emit_activate();
    }

    /// Activates the row whose text starts with `text` (for tests).
    pub(crate) fn click(self: &Rc<Self>, text: &str) -> bool {
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() else {
                continue;
            };
            if descendants_texts(row.upcast_ref())
                .first()
                .is_some_and(|t| t.starts_with(text))
            {
                if row.is_sensitive() {
                    self.activate(row.widget_name().as_str());
                }
                return true;
            }
        }
        false
    }

    /// The accessible check states of the rows' boxes: checked, mixed (for
    /// tests).
    pub(crate) fn checks(&self) -> Vec<(bool, bool)> {
        let mut out = Vec::new();
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(check) = widget
                .first_child()
                .and_then(|line| line.first_child())
                .and_downcast::<gtk::CheckButton>()
            {
                out.push((check.is_active(), check.is_inconsistent()));
            }
        }
        out
    }

    /// The popover (for tests and screenshots).
    pub(crate) fn widget(&self) -> gtk::Popover {
        self.popover.clone()
    }

    /// Whether the entry has the focus (for tests).
    pub(crate) fn entry_focused(&self) -> bool {
        self.entry
            .state_flags()
            .contains(gtk::StateFlags::FOCUS_WITHIN)
    }
}

/// The texts of the labels inside `widget`, in order.
fn descendants_texts(widget: &gtk::Widget) -> Vec<String> {
    let mut out = Vec::new();
    let mut child = widget.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        if let Some(l) = c.downcast_ref::<gtk::Label>() {
            out.push(l.text().to_string());
        }
        out.extend(descendants_texts(&c));
    }
    out
}

/// The row of one label: its check box, the name with the typed letters in the
/// accent, and the number of its tracks or where it comes from.
fn choice_row(choice: &LabelChoice, typed: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_widget_name(&format!("label:{}", choice.key.0));
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let check = gtk::CheckButton::new();
    // The row takes the clicks and the keys; the box only shows the state.
    check.set_can_target(false);
    check.set_can_focus(false);
    check.set_focusable(false);
    let (active, mixed) = match choice.state {
        LabelState::All | LabelState::FromAlbum | LabelState::FromArtist => (true, false),
        LabelState::Some => (false, true),
        LabelState::None => (false, false),
    };
    check.set_active(active);
    check.set_inconsistent(mixed);
    check.update_state(&[gtk::accessible::State::Checked(if mixed {
        gtk::AccessibleTristate::Mixed
    } else if active {
        gtk::AccessibleTristate::True
    } else {
        gtk::AccessibleTristate::False
    })]);
    line.append(&check);
    let name = label(&choice.name, &[]);
    name.set_hexpand(true);
    if !typed.is_empty() {
        name.set_attributes(Some(&crate::views::search::highlighted(
            &choice.name,
            &[typed.to_owned()],
        )));
    }
    line.append(&name);
    let (right, description) = match choice.state {
        LabelState::FromAlbum => (tr("from the album"), Some(tr("from the album"))),
        LabelState::FromArtist => (tr("from the artist"), Some(tr("from the artist"))),
        _ => (format::count(choice.tracks), None),
    };
    line.append(&label(&right, &["hk-count"]));
    row.set_child(Some(&line));
    if choice.state.inherited() {
        row.add_css_class("inherited");
        row.set_sensitive(false);
    }
    let state = match choice.state {
        LabelState::All => tr("given to all"),
        LabelState::Some => tr("given to some"),
        LabelState::None => tr("not given"),
        LabelState::FromAlbum | LabelState::FromArtist => description.unwrap_or_default(),
    };
    row.update_property(&[gtk::accessible::Property::Label(&fill(
        &ntr("{}, {} track, {}", "{}, {} tracks, {}", choice.tracks),
        &[&choice.name, &format::count(choice.tracks), &state],
    ))]);
    row
}
