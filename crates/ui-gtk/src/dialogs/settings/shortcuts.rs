// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Shortcuts: every action with its keys, in two columns of
//! groups. Activating a row records the next key combination for it (Esc
//! cancels, Backspace removes the shortcut); keys another action has are
//! named in a notice under the row, which can take them over. A changed
//! shortcut has a dot before its keys and a reset button after them. The
//! filter finds actions by name, or by keys pressed in it.
//!
//! Under the columns, where the desktop offers it, the shortcuts that work in
//! other applications: the desktop chooses their keys and shows them here.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};

use super::Context;
use crate::dialogs::common::{compact_button, key_caps};
use crate::dialogs::shortcuts::caps;
use crate::i18n::{fill, tr};
use crate::shortcuts::{self, Binding, Group};
use crate::widgets::label;

/// A conflict waiting for an answer: `action` wants `keys`, which `owner`
/// has.
#[derive(Debug, Clone)]
struct Conflict {
    action: &'static str,
    keys: String,
    owner: Binding,
}

/// The page and what it is doing.
struct Page {
    context: Weak<Context>,
    root: glib::WeakRef<gtk::Box>,
    columns: gtk::Box,
    global: gtk::Box,
    filter: gtk::SearchEntry,
    /// Keys pressed in the filter: the rows that have them.
    filter_keys: RefCell<Option<String>>,
    recording: RefCell<Option<&'static str>>,
    conflict: RefCell<Option<Conflict>>,
}

/// The keys of `binding` as caps text, such as `Ctrl+Shift+→`.
fn keys_text(accelerator: &str) -> String {
    caps(accelerator).join("+")
}

impl Page {
    fn context(&self) -> Option<Rc<Context>> {
        self.context.upgrade()
    }

    fn settings(&self) -> helikodu_model::settings::ShortcutSettings {
        self.context()
            .map(|c| c.settings().shortcuts.clone())
            .unwrap_or_default()
    }

    /// Gives `action` the keys `keys`, and takes `taken` from `from`.
    fn set_keys(self: &Rc<Self>, action: &str, keys: Vec<String>, taken: Option<(&str, &str)>) {
        let Some(context) = self.context() else {
            return;
        };
        let defaults = |action: &str| -> Vec<String> {
            shortcuts::bindings()
                .into_iter()
                .find(|b| b.action == action)
                .map(|b| {
                    b.defaults
                        .iter()
                        .copied()
                        .map(shortcuts::normalize)
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut changes = vec![(action.to_owned(), keys)];
        if let Some((from, key)) = taken {
            let current = self.settings();
            let binding = shortcuts::bindings().into_iter().find(|b| b.action == from);
            if let Some(binding) = binding {
                let rest: Vec<String> = shortcuts::keys(&binding, &current)
                    .into_iter()
                    .filter(|k| *k != shortcuts::normalize(key))
                    .collect();
                changes.push((from.to_owned(), rest));
            }
        }
        let saved = context.update(|s| {
            for (action, keys) in &changes {
                if *keys == defaults(action) {
                    s.shortcuts.keys.remove(action);
                } else {
                    s.shortcuts.keys.insert(action.clone(), keys.clone());
                }
            }
        });
        if saved && let Some(application) = context.window.application() {
            shortcuts::apply(&application, &context.settings().shortcuts);
        }
        self.render();
    }

    fn reset_all(self: &Rc<Self>) {
        let Some(context) = self.context() else {
            return;
        };
        if context.update(|s| s.shortcuts.keys.clear())
            && let Some(application) = context.window.application()
        {
            shortcuts::apply(&application, &context.settings().shortcuts);
        }
        self.recording.replace(None);
        self.conflict.replace(None);
        self.render();
    }

    fn ask_reset_all(self: &Rc<Self>) {
        let Some(context) = self.context() else {
            return;
        };
        let dialog = gtk::AlertDialog::builder()
            .message(tr("Reset all shortcuts?"))
            .detail(tr("Every action gets its original keys back."))
            .buttons([tr("Cancel"), tr("Reset all")])
            .cancel_button(0)
            .default_button(1)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.choose(
            Some(&context.window),
            gtk::gio::Cancellable::NONE,
            move |result| {
                if result == Ok(1)
                    && let Some(page) = weak.upgrade()
                {
                    page.reset_all();
                }
            },
        );
    }

    /// Starts or stops recording keys for `action`.
    fn record(self: &Rc<Self>, action: Option<&'static str>) {
        self.conflict.replace(None);
        self.recording.replace(action);
        self.render();
    }

    /// A key pressed while recording; whether it was taken.
    fn recorded(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> bool {
        let Some(action) = *self.recording.borrow() else {
            return false;
        };
        let plain = (state
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK
                | gdk::ModifierType::SHIFT_MASK))
            .is_empty();
        if plain && key == gdk::Key::Escape {
            self.record(None);
            return true;
        }
        if plain && key == gdk::Key::BackSpace {
            self.recording.replace(None);
            self.set_keys(action, Vec::new(), None);
            return true;
        }
        let Some(accelerator) = shortcuts::from_press(key, state) else {
            // A modifier on its own: wait for the key.
            return true;
        };
        let settings = self.settings();
        match shortcuts::owner(&accelerator, &settings, Some(action)) {
            Some(owner) => {
                self.recording.replace(None);
                self.conflict.replace(Some(Conflict {
                    action,
                    keys: accelerator,
                    owner,
                }));
                self.render();
            }
            None => {
                self.recording.replace(None);
                self.set_keys(action, vec![accelerator], None);
            }
        }
        true
    }

    /// Fills the columns from the settings.
    fn render(self: &Rc<Self>) {
        while let Some(child) = self.columns.first_child() {
            self.columns.remove(&child);
        }
        let settings = self.settings();
        let text = self.filter.text().trim().to_lowercase();
        let filter_keys = self.filter_keys.borrow().clone();
        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.set_hexpand(true);
        right.set_hexpand(true);
        self.columns.append(&left);
        self.columns.append(&right);
        let all = shortcuts::bindings();
        for (group, title) in Group::all() {
            let column = if group == Group::Playback {
                &left
            } else {
                &right
            };
            let rows: Vec<&Binding> = all
                .iter()
                .filter(|b| b.group == group)
                .filter(|b| {
                    let keys = shortcuts::keys(b, &settings);
                    match &filter_keys {
                        Some(accelerator) => keys.iter().any(|k| k == accelerator),
                        None => {
                            let words = format!(
                                "{} {}",
                                b.name.to_lowercase(),
                                keys.iter()
                                    .map(|k| keys_text(k).to_lowercase())
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            );
                            text.split_whitespace().all(|w| words.contains(w))
                        }
                    }
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            let head = label(&title, &["hk-section-label", "hk-list-section"]);
            if column.first_child().is_some() {
                head.set_margin_top(14);
            }
            column.append(&head);
            for binding in rows {
                column.append(&self.row(binding, &settings));
                if let Some(conflict) = self
                    .conflict
                    .borrow()
                    .as_ref()
                    .filter(|c| c.action == binding.action)
                {
                    column.append(&self.conflict_notice(conflict));
                }
            }
        }
    }

    fn row(
        self: &Rc<Self>,
        binding: &Binding,
        settings: &helikodu_model::settings::ShortcutSettings,
    ) -> gtk::Box {
        // A button to screen readers: Enter or a click records new keys.
        let row = glib::Object::builder::<gtk::Box>()
            .property("orientation", gtk::Orientation::Horizontal)
            .property("spacing", 3)
            .property("accessible-role", gtk::AccessibleRole::Button)
            .build();
        row.add_css_class("hk-shortcut-edit");
        row.set_focusable(true);
        row.set_widget_name(binding.action);
        let name = label(&binding.name, &[]);
        name.set_hexpand(true);
        row.append(&name);
        let keys = shortcuts::keys(binding, settings);
        let recording = *self.recording.borrow() == Some(binding.action);
        let changed = shortcuts::changed(binding, settings);
        if changed {
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("hk-changed-dot");
            dot.set_valign(gtk::Align::Center);
            dot.set_margin_end(6);
            dot.update_property(&[gtk::accessible::Property::Label(&tr("Changed"))]);
            row.append(&dot);
        }
        let caps_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        caps_box.set_valign(gtk::Align::Center);
        for accelerator in &keys {
            let caps = caps(accelerator);
            let refs: Vec<&str> = caps.iter().map(String::as_str).collect();
            let shown = key_caps(&refs);
            shown.update_property(&[gtk::accessible::Property::Label(&caps.join("+"))]);
            caps_box.append(&shown);
        }
        let accessible_keys = if keys.is_empty() {
            tr("Not set")
        } else {
            keys.iter()
                .map(|k| keys_text(k))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if recording {
            row.add_css_class("recording");
            caps_box.add_css_class("hk-keys-recording");
            let caret = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            caret.add_css_class("hk-caret");
            caret.set_valign(gtk::Align::Center);
            caps_box.append(&caret);
            row.append(&caps_box);
        } else if keys.is_empty() {
            row.append(&label(&tr("Not set"), &["hk-t3"]));
        } else {
            row.append(&caps_box);
        }
        if changed {
            let reset = crate::widgets::icon_button("edit-undo-symbolic", &tr("Reset"));
            reset.add_css_class("hk-reset");
            reset.set_valign(gtk::Align::Center);
            reset.set_margin_start(4);
            let weak = Rc::downgrade(self);
            let action = binding.action;
            let defaults: Vec<String> = binding
                .defaults
                .iter()
                .copied()
                .map(shortcuts::normalize)
                .collect();
            reset.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.set_keys(action, defaults.clone(), None);
                }
            });
            row.append(&reset);
        }
        row.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{}: {}"),
            &[&binding.name, &accessible_keys],
        ))]);
        let action = binding.action;
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_released(move |gesture, _, x, y| {
            let Some(page) = weak.upgrade() else { return };
            // The reset button handles its own clicks.
            if let Some(row) = gesture.widget()
                && let Some(picked) = row.pick(x, y, gtk::PickFlags::DEFAULT)
                && picked.ancestor(gtk::Button::static_type()).is_some()
            {
                return;
            }
            page.record(Some(action));
        });
        row.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(page) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if page.recording.borrow().is_none()
                && state.is_empty()
                && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space)
            {
                page.record(Some(action));
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        row.add_controller(keys);
        if recording {
            let row = row.clone();
            glib::idle_add_local_once(move || {
                row.grab_focus();
            });
        }
        row
    }

    fn conflict_notice(self: &Rc<Self>, conflict: &Conflict) -> gtk::Box {
        let notice = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        notice.add_css_class("hk-conflict");
        let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
        icon.set_valign(gtk::Align::Center);
        notice.append(&icon);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        text.append(&label(
            &fill(&tr("{} is already in use"), &[&keys_text(&conflict.keys)]),
            &["hk-medium"],
        ));
        text.append(&label(&conflict.owner.name, &["hk-t3", "hk-secondary"]));
        notice.append(&text);
        let take = gtk::Button::with_label(&tr("Use it here"));
        take.add_css_class("hk-text-button");
        take.add_css_class("accent");
        take.set_valign(gtk::Align::Center);
        let cancel = gtk::Button::with_label(&tr("Cancel"));
        cancel.add_css_class("hk-text-button");
        cancel.set_valign(gtk::Align::Center);
        let weak = Rc::downgrade(self);
        let wanted = conflict.clone();
        take.connect_clicked(move |_| {
            let Some(page) = weak.upgrade() else { return };
            page.conflict.replace(None);
            page.set_keys(
                wanted.action,
                vec![wanted.keys.clone()],
                Some((wanted.owner.action, &wanted.keys)),
            );
        });
        let weak = Rc::downgrade(self);
        cancel.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.conflict.replace(None);
                page.render();
            }
        });
        notice.append(&take);
        notice.append(&cancel);
        notice.update_property(&[gtk::accessible::Property::Label(&tr("Problem"))]);
        notice
    }

    /// Fills the box of the shortcuts in other applications, or hides it.
    fn render_global(self: &Rc<Self>) {
        while let Some(child) = self.global.first_child() {
            self.global.remove(&child);
        }
        let main = self.context().and_then(|c| c.main.upgrade());
        let Some(state) = main.as_ref().and_then(|m| m.global_shortcut_state()) else {
            self.global.set_visible(false);
            return;
        };
        self.global.set_visible(true);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        head.add_css_class("hk-global-head");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        text.append(&label(&tr("In other applications"), &["hk-medium"]));
        let line = label(
            &tr(
                "These work while another window is in front. The desktop chooses the keys and shows them here.",
            ),
            &["hk-t3", "hk-secondary"],
        );
        line.set_wrap(true);
        text.append(&line);
        head.append(&text);
        let set_up = compact_button(&tr("Set up in the desktop…"));
        let weak = main.as_ref().map(|m| m.downgrade());
        set_up.connect_clicked(move |_| {
            if let Some(main) = weak.as_ref().and_then(|w| w.upgrade()) {
                main.set_up_global_shortcuts();
            }
        });
        head.append(&set_up);
        self.global.append(&head);
        let cells = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        cells.set_homogeneous(true);
        for spec in crate::window::desktop::global_shortcuts() {
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 3);
            cell.add_css_class("hk-global-cell");
            cell.append(&label(&spec.description, &["hk-t3", "hk-secondary"]));
            let trigger = state
                .bound
                .as_ref()
                .and_then(|bound| bound.iter().find(|b| b.id == spec.id))
                .map(|b| b.trigger.clone())
                .filter(|t| !t.is_empty());
            match trigger {
                Some(trigger) => cell.append(&label(&trigger, &["hk-mono", "hk-secondary"])),
                None => cell.append(&label(&tr("Not set"), &["hk-t3", "hk-secondary"])),
            }
            cells.append(&cell);
        }
        self.global.append(&cells);
    }
}

thread_local! {
    /// The page built last, for tests.
    static CURRENT: RefCell<Weak<Page>> = const { RefCell::new(Weak::new()) };
}

/// Records `accelerator` for `action` on the page built last, as if the row
/// had been activated and the keys pressed; `Escape` and `BackSpace` cancel
/// and remove (for tests).
pub(super) fn demo_record(action: &str, accelerator: &str) -> bool {
    let Some(page) = CURRENT.with(|c| c.borrow().upgrade()) else {
        return false;
    };
    let Some(binding) = shortcuts::bindings()
        .into_iter()
        .find(|b| b.action == action)
    else {
        return false;
    };
    page.record(Some(binding.action));
    let Some((key, mods)) = gtk::accelerator_parse(accelerator) else {
        return false;
    };
    page.recorded(key, mods)
}

/// "Reset all" without the question (for tests).
pub(super) fn demo_reset_all() {
    if let Some(page) = CURRENT.with(|c| c.borrow().upgrade()) {
        page.reset_all();
    }
}

/// The Shortcuts page.
pub(super) fn page(context: &Rc<Context>) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("hk-shortcuts-page");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let filter = gtk::SearchEntry::new();
    filter.add_css_class("hk-input");
    filter.set_hexpand(true);
    let hint = tr("Find an action, or press keys to see what they do");
    filter.set_placeholder_text(Some(&hint));
    filter.update_property(&[gtk::accessible::Property::Label(&hint)]);
    top.append(&filter);
    let reset_all = compact_button(&tr("Reset all…"));
    top.append(&reset_all);
    root.append(&top);
    let columns = gtk::Box::new(gtk::Orientation::Horizontal, 40);
    columns.set_homogeneous(true);
    columns.set_margin_top(12);
    root.append(&columns);
    let global = gtk::Box::new(gtk::Orientation::Vertical, 0);
    global.add_css_class("hk-global-box");
    root.append(&global);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&root));

    let page = Rc::new(Page {
        context: Rc::downgrade(context),
        root: root.downgrade(),
        columns,
        global,
        filter: filter.clone(),
        filter_keys: RefCell::new(None),
        recording: RefCell::new(None),
        conflict: RefCell::new(None),
    });
    page.render();
    page.render_global();
    CURRENT.with(|c| c.replace(Rc::downgrade(&page)));
    let weak = Rc::downgrade(&page);
    reset_all.connect_clicked(move |_| {
        if let Some(page) = weak.upgrade() {
            page.ask_reset_all();
        }
    });
    let weak = Rc::downgrade(&page);
    filter.connect_search_changed(move |entry| {
        let Some(page) = weak.upgrade() else { return };
        // Text typed after keys were pressed searches by name again.
        let keys = page.filter_keys.borrow().clone();
        if keys.is_some_and(|k| keys_text(&k) != entry.text().as_str()) {
            page.filter_keys.replace(None);
        }
        page.render();
    });
    // Keys with Ctrl, Alt or Super (or a function key) pressed in the filter
    // show the actions that have them.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&page);
    keys.connect_key_pressed(move |_, key, _, state| {
        let Some(page) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let modified = state.intersects(
            gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK,
        );
        let function = key
            .name()
            .is_some_and(|n| n.starts_with('F') && n[1..].parse::<u32>().is_ok());
        if !(modified || function) {
            return glib::Propagation::Proceed;
        }
        let Some(accelerator) = shortcuts::from_press(key, state) else {
            return glib::Propagation::Proceed;
        };
        page.filter_keys.replace(Some(accelerator.clone()));
        page.filter.set_text(&keys_text(&accelerator));
        page.filter.set_position(-1);
        page.render();
        glib::Propagation::Stop
    });
    filter.add_controller(keys);
    // While recording, the next keys go to the row, before the window's own
    // shortcuts (Esc closes the window otherwise).
    let record = gtk::EventControllerKey::new();
    record.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&page);
    record.connect_key_pressed(move |_, key, _, state| {
        let Some(page) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if page.root.upgrade().is_some_and(|r| r.is_mapped()) && page.recorded(key, state) {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    context.window.add_controller(record);
    if let Some(main) = context.main.upgrade() {
        let weak = Rc::downgrade(&page);
        main.connect_desktop_changed(&root, move |_| {
            if let Some(page) = weak.upgrade() {
                page.render_global();
            }
        });
    }
    // The page lives as long as its widgets.
    let keep = RefCell::new(Some(page));
    root.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    scroller.upcast()
}
