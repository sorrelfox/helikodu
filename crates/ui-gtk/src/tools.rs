// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tool windows: separate, non-modal windows in the layout of Supported
//! formats (a list at the left, options, a table, the action in the footer),
//! and the per-track copies window built from the same table. Nothing changes
//! before the footer's button is used, and files only ever go to the desktop
//! trash after a confirmation.

pub(crate) mod converter;
pub(crate) mod copies;
pub(crate) mod cover_manager;
pub(crate) mod duplicates;
pub(crate) mod file_tools;
pub(crate) mod jobs;
pub(crate) mod missing_files;
pub(crate) mod organizer;
pub(crate) mod rename;
pub(crate) mod sync_export;
pub(crate) mod tag_preview;
pub(crate) mod tag_tools;
pub(crate) mod tool_window;
pub(crate) mod trash;
pub(crate) mod verify_files;

use crate::backend::export::{PatternError, PatternProblem};
use crate::i18n::{fill, tr};

/// Why a naming pattern cannot be read, in a sentence with its place: the same
/// words in the organizer, the converter and folder sync.
pub(crate) fn naming_problem(error: &PatternError) -> String {
    let problem = match &error.problem {
        PatternProblem::Empty => tr("The pattern is empty."),
        PatternProblem::UnclosedField => tr("A field is not closed with }."),
        PatternProblem::EmptyField => fill(
            // Translators: {} stands for an empty pair of braces typed in the
            // pattern; it is shown as "{}".
            &tr("{} names no field."),
            &["{}"],
        ),
        PatternProblem::UnknownField(name) => fill(&tr("There is no field called {}."), &[name]),
        PatternProblem::UnclosedBracket => tr("A [ is not closed with ]."),
        PatternProblem::UnexpectedClose(c) => fill(&tr("{} closes nothing."), &[&c.to_string()]),
        PatternProblem::NestedField => tr("A field name cannot hold {."),
        PatternProblem::EmptyFolder => tr("A folder name would be empty."),
        PatternProblem::TooDeep => tr("Square brackets are nested too deeply."),
        PatternProblem::TooComplex => tr("The expression is nested too deeply."),
        PatternProblem::UnclosedQuote => tr("A quote is not closed."),
        PatternProblem::UnclosedParenthesis => tr("A ( is not closed with )."),
        PatternProblem::UnknownFunction(name) => {
            fill(&tr("There is no function called {}."), &[name])
        }
        PatternProblem::Arguments { function, min, max } => match max {
            Some(max) if max == min => fill(
                // Translators: {} are a function's name and a number.
                &tr("{} takes {} values."),
                &[*function, &min.to_string()],
            ),
            Some(max) => fill(
                // Translators: {} are a function's name and two numbers.
                &tr("{} takes {} to {} values."),
                &[*function, &min.to_string(), &max.to_string()],
            ),
            None => fill(
                // Translators: {} are a function's name and a number.
                &tr("{} takes at least {} values."),
                &[*function, &min.to_string()],
            ),
        },
        PatternProblem::MissingValue => tr("A value is missing."),
        PatternProblem::Unexpected(text) => fill(&tr("{} does not belong here."), &[text]),
        PatternProblem::NameExpected => tr("let needs a name first."),
        PatternProblem::FieldExpected => tr("raw needs a field."),
    };
    fill(
        &tr("{} At character {}."),
        &[&problem, &(error.start + 1).to_string()],
    )
}

/// Rename and delete on the rows of the list at the left of a tool window, as
/// the saved searches have them: a right click or the Menu key opens a menu
/// with "Rename…" and "Delete…", F2 renames and Delete deletes. Only rows that
/// `editable` accepts take part (not "New pattern", not a built-in preset).
pub(crate) fn edit_rows(
    list: &gtk::ListBox,
    editable: impl Fn(&gtk::ListBoxRow) -> bool + 'static,
    rename: impl Fn(&gtk::ListBoxRow) + 'static,
    delete: impl Fn(&gtk::ListBoxRow) + 'static,
) {
    use gtk::prelude::*;
    use gtk::{gdk, gio, glib};
    use std::rc::Rc;

    type RowFn = Rc<dyn Fn(&gtk::ListBoxRow)>;
    let editable: Rc<dyn Fn(&gtk::ListBoxRow) -> bool> = Rc::new(editable);
    let rename: RowFn = Rc::new(rename);
    let delete: RowFn = Rc::new(delete);
    let menu = {
        let (rename, delete) = (rename.clone(), delete.clone());
        Rc::new(
            move |list: &gtk::ListBox, row: &gtk::ListBoxRow, at: Option<(f64, f64)>| {
                let group = gio::SimpleActionGroup::new();
                for (name, act) in [("rename", rename.clone()), ("delete", delete.clone())] {
                    let action = gio::SimpleAction::new(name, None);
                    let row = row.downgrade();
                    action.connect_activate(move |_, _| {
                        if let Some(row) = row.upgrade() {
                            act(&row);
                        }
                    });
                    group.add_action(&action);
                }
                list.insert_action_group("row", Some(&group));
                let model = gio::Menu::new();
                model.append(Some(&tr("Rename…")), Some("row.rename"));
                model.append(Some(&tr("Delete…")), Some("row.delete"));
                let popover = gtk::PopoverMenu::from_model(Some(&model));
                crate::widgets::menu_names::name_menus(&popover);
                popover.set_has_arrow(false);
                popover.set_parent(list);
                let (x, y) = at.unwrap_or_else(|| {
                    row.compute_bounds(list).map_or((0.0, 0.0), |b| {
                        (f64::from(b.x() + 24.0), f64::from(b.y() + b.height()))
                    })
                });
                popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                popover.connect_closed(|p| {
                    let p = p.clone();
                    glib::idle_add_local_once(move || p.unparent());
                });
                popover.popup();
            },
        )
    };
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    {
        let (editable, menu) = (editable.clone(), menu.clone());
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(list) = gesture.widget().and_downcast::<gtk::ListBox>() else {
                return;
            };
            let Some(row) = list.row_at_y(y as i32) else {
                return;
            };
            if !editable(&row) {
                return;
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
            list.select_row(Some(&row));
            menu(&list, &row, Some((x, y)));
        });
    }
    list.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |controller, key, _, state| {
        let Some(list) = controller.widget().and_downcast::<gtk::ListBox>() else {
            return glib::Propagation::Proceed;
        };
        let Some(row) = list.selected_row().filter(|row| editable(row)) else {
            return glib::Propagation::Proceed;
        };
        if crate::views::actions::is_menu_key(key, state) {
            menu(&list, &row, None);
        } else if key == gdk::Key::F2 {
            rename(&row);
        } else if key == gdk::Key::Delete {
            delete(&row);
        } else {
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Stop
    });
    list.add_controller(keys);
}

/// Asks whether to delete: `message` as the question, `detail` under it;
/// `done` runs on "Delete".
pub(crate) fn confirm_delete(
    near: &impl gtk::prelude::IsA<gtk::Widget>,
    message: &str,
    detail: &str,
    done: impl FnOnce() + 'static,
) {
    use gtk::prelude::*;
    let dialog = gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .buttons([tr("Cancel"), tr("Delete")])
        .cancel_button(0)
        .default_button(0)
        .modal(true)
        .build();
    let window = near.root().and_downcast::<gtk::Window>();
    dialog.choose(
        window.as_ref(),
        gtk::gio::Cancellable::NONE,
        move |answer| {
            if answer == Ok(1) {
                done();
            }
        },
    );
}
