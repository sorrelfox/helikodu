// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Label chips: the labels of an album or an artist under its page's title,
//! each a link to the label in the Labels view, then "Add label", which opens
//! the Labels popover for the album or artist. A chip is told apart from the
//! technical badges by its shape, its icon and its sentence-case text.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::prelude::*;

use crate::backend::{Backend, LabelFor, LabelKey};
use crate::i18n::{fill, tr};
use crate::widgets::WrapBox;
use crate::window::MainWindow;
use crate::window::labels::Scope;

thread_local! {
    /// Every row of chips, to read again after a change.
    static ROWS: RefCell<Vec<Weak<LabelChips>>> = const { RefCell::new(Vec::new()) };
}

/// Reads the labels of every row of chips shown again.
pub(crate) fn reload_all() {
    let rows: Vec<Rc<LabelChips>> = ROWS.with(|rows| {
        let mut rows = rows.borrow_mut();
        rows.retain(|r| r.strong_count() > 0);
        rows.iter().filter_map(Weak::upgrade).collect()
    });
    for row in rows {
        row.reload();
    }
}

/// The rows of chips on screen now (for tests).
pub(crate) fn shown() -> Vec<Rc<LabelChips>> {
    ROWS.with(|rows| {
        rows.borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|r| r.row.is_mapped())
            .collect()
    })
}

/// A chip: a 12 px icon and the text.
fn chip(icon: &str, text: &str, classes: &[&str]) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("hk-label-chip");
    for class in classes {
        button.add_css_class(class);
    }
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(12);
    line.append(&image);
    let name = gtk::Label::new(Some(text));
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(28);
    line.append(&name);
    button.set_child(Some(&line));
    button.set_valign(gtk::Align::Center);
    button
}

/// The chips of one album or artist page.
pub(crate) struct LabelChips {
    row: WrapBox,
    backend: Backend,
    shown: RefCell<Option<(LabelFor, Scope)>>,
    generation: Cell<u64>,
    add: RefCell<Option<gtk::Button>>,
}

impl std::fmt::Debug for LabelChips {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelChips").finish_non_exhaustive()
    }
}

impl LabelChips {
    /// Chips in `row`, read through `backend`.
    pub(crate) fn new(row: &WrapBox, backend: &Backend) -> Rc<Self> {
        let chips = Rc::new(Self {
            row: row.clone(),
            backend: backend.clone(),
            shown: RefCell::new(None),
            generation: Cell::new(0),
            add: RefCell::new(None),
        });
        ROWS.with(|rows| rows.borrow_mut().push(Rc::downgrade(&chips)));
        chips
    }

    /// Shows the labels of `target`, which `scope` names.
    pub(crate) fn show(self: &Rc<Self>, target: LabelFor, scope: Scope) {
        let changed = self
            .shown
            .borrow()
            .as_ref()
            .is_none_or(|(shown, _)| *shown != target);
        if changed {
            // No chips of the page before while the new ones load.
            self.fill(&[]);
        }
        self.shown.replace(Some((target, scope)));
        self.reload();
    }

    /// Reads the labels of what is shown again.
    pub(crate) fn reload(self: &Rc<Self>) {
        let Some((target, _)) = self.shown.borrow().clone() else {
            return;
        };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let choices = backend.query(move |l| l.label_choices(&target)).await;
            let Some(chips) = weak.upgrade() else { return };
            if chips.generation.get() != generation {
                return;
            }
            let own: Vec<(LabelKey, String)> = choices
                .map(|c| c.own().map(|l| (l.key, l.name.clone())).collect())
                .unwrap_or_default();
            chips.fill(&own);
        });
    }

    fn fill(self: &Rc<Self>, labels: &[(LabelKey, String)]) {
        // "Add label" stays: the Labels popover may be open on it.
        let add = self.add_chip();
        let mut child = self.row.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if widget != *add.upcast_ref::<gtk::Widget>() {
                self.row.remove(&widget);
            }
        }
        for (key, name) in labels {
            let button = chip("helikodu-label-symbolic", name, &[]);
            button.set_tooltip_text(Some(&tr("Show this label")));
            button.update_property(&[gtk::accessible::Property::Label(&fill(
                &tr("Label {}"),
                &[name],
            ))]);
            let key = *key;
            button.connect_clicked(move |button| {
                if let Some(window) = button.root().and_downcast::<MainWindow>() {
                    window.open_label(key);
                }
            });
            button.insert_before(&self.row, Some(&add));
        }
    }

    /// "Add label", made once and kept last.
    fn add_chip(self: &Rc<Self>) -> gtk::Button {
        if let Some(add) = self.add.borrow().clone() {
            return add;
        }
        let add = chip("list-add-symbolic", &tr("Add label"), &["add"]);
        let weak = Rc::downgrade(self);
        add.connect_clicked(move |button| {
            let (Some(chips), Some(window)) =
                (weak.upgrade(), button.root().and_downcast::<MainWindow>())
            else {
                return;
            };
            let Some((target, scope)) = chips.shown.borrow().clone() else {
                return;
            };
            window.open_labels_popover(target, scope, button.upcast_ref(), None);
        });
        self.row.append(&add);
        self.add.replace(Some(add.clone()));
        add
    }

    /// The names on the chips, "Add label" last (for tests).
    pub(crate) fn texts(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut child = self.row.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(name) = widget
                .downcast_ref::<gtk::Button>()
                .and_then(|b| b.child())
                .and_then(|line| line.last_child())
                .and_downcast::<gtk::Label>()
            {
                out.push(name.text().to_string());
            }
        }
        out
    }

    /// Clicks the chip with `text` (for tests).
    pub(crate) fn click(&self, text: &str) -> bool {
        let mut child = self.row.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            let Some(button) = widget.downcast_ref::<gtk::Button>() else {
                continue;
            };
            let name = button
                .child()
                .and_then(|line| line.last_child())
                .and_downcast::<gtk::Label>();
            if name.is_some_and(|n| n.text() == text) {
                button.emit_clicked();
                return true;
            }
        }
        false
    }
}
