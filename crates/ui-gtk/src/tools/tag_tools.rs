// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tag tools, opened from the tag editor for the files open there: fill tags
//! from file names, number the tracks, change capital letters, find and
//! replace, repair garbled text, create sort names.
//!
//! Each tool shows its options under the header and a table of the files with
//! the value each would get: in `t1` when it changes the tag, in `t3` when the
//! tag already holds it, the old value in `t3` beside a new one. The primary
//! button hands the values to the tag editor as edited fields; they are
//! written when the editor saves, and undone with it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use helikodu_service::library::tag_tools::{Case, LEGACY_CHARSETS, PATTERN_FIELDS};

use super::tag_preview::{
    Cell as PreviewCell, Options, Preview, TEXT_FIELDS, Tool, ToolTrack, ToolValues, field_name,
    preview, sort_fields,
};
use super::tool_window::{Frame, GAP, footer, frame, primary_button};
use crate::backend::services::TagField;
use crate::dialogs::common::{compact_button, dropdown, find_open, note};
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "tag-tools";

/// The width of the file name column.
const FILE: i32 = 300;

/// What the window hands its values to.
type Apply = Box<dyn Fn(ToolValues)>;

struct State {
    window: gtk::Window,
    tracks: Vec<ToolTrack>,
    tool: Cell<Tool>,
    options: RefCell<Options>,
    title: gtk::Label,
    choices: gtk::Box,
    table: gtk::Grid,
    summary: gtk::Label,
    primary: gtk::Button,
    shown: RefCell<Preview>,
    apply: Apply,
    /// The pattern entry, to insert fields at its cursor.
    pattern: RefCell<Option<gtk::Entry>>,
    /// The charset dropdown of Repair garbled text, refilled after a preview.
    charsets: RefCell<Option<gtk::DropDown>>,
}

/// Opens Tag tools over the tag editor `parent` for `tracks`, showing `tool`.
/// `apply` gets the values the user keeps.
pub(crate) fn open(
    parent: &gtk::Window,
    tracks: Vec<ToolTrack>,
    tool: Tool,
    apply: impl Fn(ToolValues) + 'static,
) -> gtk::Window {
    if let Some(window) = find_open(parent, NAME) {
        window.close();
    }
    let Frame {
        window,
        list,
        side_note,
        title,
        summary: header_summary,
        main: column,
    } = frame(parent, NAME, &tr("Tag tools"), &tr("Tools"));
    let n = tracks.len();
    header_summary.set_text(&fill(
        &ntr(
            "{} file from the tag editor",
            "{} files from the tag editor",
            n,
        ),
        &[&n.to_string()],
    ));
    side_note.set_text(&tr(
        "Nothing is written until you choose Fill. The change can be undone with Ctrl+Z.",
    ));
    for tool in Tool::ALL {
        let row = gtk::ListBoxRow::new();
        let name = label(&tool.title(), &["hk-side-name"]);
        row.set_child(Some(&name));
        row.set_widget_name(tool.id());
        row.update_property(&[gtk::accessible::Property::Label(&tool.title())]);
        list.append(&row);
    }

    let choices = gtk::Box::new(gtk::Orientation::Vertical, 10);
    choices.add_css_class("hk-tagtool-options");
    column.append(&choices);
    // One grid for the header and the rows, so that the columns line up.
    let table = gtk::Grid::new();
    table.add_css_class("hk-tagtool-table");
    table.set_column_spacing(u32::try_from(GAP).unwrap_or(14));
    table.set_valign(gtk::Align::Start);
    table.update_property(&[gtk::accessible::Property::Label(&tr("Files"))]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&table));
    column.append(&scroller);
    let (bar, summary) = footer();
    let cancel = compact_button(&tr("Cancel"));
    bar.append(&cancel);
    let primary = primary_button(&Tool::FromNames.action(0));
    primary.set_widget_name("apply");
    bar.append(&primary);
    column.append(&bar);

    let state = Rc::new(State {
        window: window.clone(),
        tracks,
        tool: Cell::new(tool),
        options: RefCell::new(Options::default()),
        title,
        choices,
        table,
        summary,
        primary: primary.clone(),
        shown: RefCell::new(Preview::default()),
        apply: Box::new(apply),
        pattern: RefCell::new(None),
        charsets: RefCell::new(None),
    });

    let weak = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    let weak = Rc::downgrade(&state);
    primary.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            let values = state.shown.borrow().values.clone();
            (state.apply)(values);
            state.window.close();
        }
    });
    let weak = Rc::downgrade(&state);
    list.connect_row_selected(move |_, row| {
        let (Some(state), Some(row)) = (weak.upgrade(), row) else {
            return;
        };
        if let Some(tool) = Tool::from_id(&row.widget_name()) {
            state.show_tool(tool);
        }
    });
    let index = Tool::ALL.iter().position(|t| *t == tool).unwrap_or(0);
    list.select_row(
        list.row_at_index(i32::try_from(index).unwrap_or(0))
            .as_ref(),
    );
    state.show_tool(tool);
    window.set_default_widget(Some(&primary));
    let keep = RefCell::new(Some(state));
    window.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    window.present();
    window
}

impl State {
    fn show_tool(self: &Rc<Self>, tool: Tool) {
        self.tool.set(tool);
        self.title.set_text(&tool.title());
        while let Some(child) = self.choices.first_child() {
            self.choices.remove(&child);
        }
        self.pattern.replace(None);
        self.charsets.replace(None);
        match tool {
            Tool::FromNames => self.from_names_choices(),
            Tool::Number => self.number_choices(),
            Tool::Case => self.case_choices(),
            Tool::Replace => self.replace_choices(),
            Tool::Repair => self.repair_choices(),
            Tool::SortNames => self.sort_choices(),
        }
        self.update();
    }

    /// Works the preview out again and shows it.
    fn update(self: &Rc<Self>) {
        let found = preview(self.tool.get(), &self.tracks, &self.options.borrow());
        fill_table(&self.table, &found);
        self.summary.set_text(&found.summary);
        let files = found.files();
        self.primary.set_label(&self.tool.get().action(files));
        self.primary
            .set_sensitive(files > 0 && found.problem.is_none());
        if let Some(charsets) = self.charsets.borrow().as_ref()
            && let Some(model) = charsets.model().and_downcast::<gtk::StringList>()
        {
            let detected = match found.detected {
                Some(name) => fill(&tr("Detected: {}"), &[name]),
                None => tr("Detected: none"),
            };
            if model.string(0).as_deref() != Some(detected.as_str()) {
                model.splice(0, 1, &[&detected]);
            }
        }
        self.shown.replace(found);
    }

    /// A row of the option area: a label of 72 px and the controls.
    fn option_row(&self, name: &str) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let title = label(name, &["hk-t2"]);
        title.set_size_request(72, -1);
        title.set_valign(gtk::Align::Center);
        row.append(&title);
        self.choices.append(&row);
        row
    }

    /// A check box that sets an option and works the preview out again.
    fn check(
        self: &Rc<Self>,
        text: &str,
        active: bool,
        set: impl Fn(&mut Options, bool) + 'static,
    ) -> gtk::CheckButton {
        let check = gtk::CheckButton::with_label(text);
        check.set_active(active);
        let weak = Rc::downgrade(self);
        check.connect_toggled(move |check| {
            if let Some(state) = weak.upgrade() {
                set(&mut state.options.borrow_mut(), check.is_active());
                state.update();
            }
        });
        check
    }

    /// Check boxes for the text fields `pick` reads and changes.
    fn field_checks(
        self: &Rc<Self>,
        row: &gtk::Box,
        fields: &[TagField],
        pick: fn(&mut Options) -> &mut std::collections::BTreeSet<TagField>,
    ) {
        for field in fields {
            let active = pick(&mut self.options.borrow_mut()).contains(field);
            let field = field.clone();
            row.append(&self.check(&field_name(&field), active, move |o, on| {
                let set = pick(o);
                if on {
                    set.insert(field.clone());
                } else {
                    set.remove(&field);
                }
            }));
        }
    }

    fn from_names_choices(self: &Rc<Self>) {
        let row = self.option_row(&tr("Pattern"));
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.add_css_class("hk-query");
        entry.set_hexpand(true);
        entry.set_text(&self.options.borrow().pattern);
        entry.update_property(&[gtk::accessible::Property::Label(&tr("Pattern"))]);
        let weak = Rc::downgrade(self);
        entry.connect_changed(move |entry| {
            if let Some(state) = weak.upgrade() {
                state.options.borrow_mut().pattern = entry.text().to_string();
                state.update();
                state.fill_checks();
            }
        });
        row.append(&entry);
        let insert = gtk::MenuButton::new();
        insert.set_label(&tr("Insert field"));
        insert.add_css_class("hk-sort");
        insert.set_valign(gtk::Align::Center);
        let menu = gio::Menu::new();
        let group = gio::SimpleActionGroup::new();
        let action = gio::SimpleAction::new("insert", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        action.connect_activate(move |_, parameter| {
            let (Some(state), Some(name)) = (
                weak.upgrade(),
                parameter.and_then(glib::Variant::get::<String>),
            ) else {
                return;
            };
            if let Some(entry) = state.pattern.borrow().as_ref() {
                let mut position = entry.position();
                entry.insert_text(&format!("{{{name}}}"), &mut position);
                entry.set_position(position);
                entry.grab_focus();
            }
        });
        group.add_action(&action);
        self.window.insert_action_group("tagtools", Some(&group));
        for name in PATTERN_FIELDS {
            let item = gio::MenuItem::new(Some(&format!("{{{name}}}")), None);
            item.set_action_and_target_value(Some("tagtools.insert"), Some(&name.to_variant()));
            menu.append_item(&item);
        }
        crate::widgets::menu_names::set_menu_later(&insert, &menu);
        row.append(&insert);
        self.pattern.replace(Some(entry));
        let hint = note(&tr(
            "The pattern is compared with the file name without its extension. Use {skip} for a part you do not need.",
        ));
        hint.add_css_class("hk-t3");
        hint.set_margin_start(84);
        self.choices.append(&hint);
        let fill_row = self.option_row(&tr("Fill"));
        fill_row.set_widget_name("fill-row");
        self.fill_checks();
    }

    /// The check boxes of the fields in the pattern, and "Only where the tag
    /// is empty".
    fn fill_checks(self: &Rc<Self>) {
        let Some(row) = self
            .choices
            .last_child()
            .and_downcast::<gtk::Box>()
            .filter(|b| b.widget_name() == "fill-row")
        else {
            return;
        };
        // Keep the label; the checks follow the pattern.
        while let Some(child) = row.last_child() {
            if row.first_child().as_ref() == Some(&child) {
                break;
            }
            row.remove(&child);
        }
        let pattern = self.options.borrow().pattern.clone();
        let names: Vec<String> = helikodu_service::library::tag_tools::NamePattern::parse(&pattern)
            .map(|p| p.fields().into_iter().map(str::to_owned).collect())
            .unwrap_or_default();
        for name in names {
            let Some(field) = super::tag_preview::pattern_field(&name) else {
                continue;
            };
            let active = !self.options.borrow().unfilled.contains(&name);
            let key = name.clone();
            row.append(&self.check(&field_name(&field), active, move |o, on| {
                if on {
                    o.unfilled.remove(&key);
                } else {
                    o.unfilled.insert(key.clone());
                }
            }));
        }
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        row.append(&spacer);
        let only_empty = self.options.borrow().only_empty;
        row.append(
            &self.check(&tr("Only where the tag is empty"), only_empty, |o, on| {
                o.only_empty = on
            }),
        );
    }

    fn number_choices(self: &Rc<Self>) {
        let row = self.option_row(&tr("Start at"));
        let first = gtk::SpinButton::with_range(0.0, 9999.0, 1.0);
        first.set_value(f64::from(self.options.borrow().first));
        first.set_valign(gtk::Align::Center);
        first.update_property(&[gtk::accessible::Property::Label(&tr("The first number"))]);
        let weak = Rc::downgrade(self);
        first.connect_value_changed(move |spin| {
            if let Some(state) = weak.upgrade() {
                state.options.borrow_mut().first = u32::try_from(spin.value_as_int()).unwrap_or(1);
                state.update();
            }
        });
        row.append(&first);
        let (per_disc, totals) = {
            let o = self.options.borrow();
            (o.per_disc, o.totals)
        };
        row.append(
            &self.check(&tr("Start again for each disc"), per_disc, |o, on| {
                o.per_disc = on
            }),
        );
        row.append(&self.check(&tr("Write the total"), totals, |o, on| {
            o.totals = on;
        }));
        let hint = note(&tr(
            "The files are numbered in the order of the tag editor.",
        ));
        hint.add_css_class("hk-t3");
        hint.set_margin_start(84);
        self.choices.append(&hint);
    }

    fn case_choices(self: &Rc<Self>) {
        let row = self.option_row(&tr("Change to"));
        let cases = [Case::Title, Case::Sentence, Case::Lower, Case::Upper];
        let names: Vec<String> = vec![
            tr("Title Case"),
            tr("Sentence case"),
            tr("lower case"),
            tr("UPPER CASE"),
        ];
        let now = cases
            .iter()
            .position(|c| *c == self.options.borrow().case)
            .unwrap_or(0);
        let pick = dropdown(&names, now, 180);
        pick.update_property(&[gtk::accessible::Property::Label(&tr("Capital letters"))]);
        let weak = Rc::downgrade(self);
        pick.connect_selected_notify(move |pick| {
            if let Some(state) = weak.upgrade() {
                let index = usize::try_from(pick.selected()).unwrap_or(0);
                state.options.borrow_mut().case = cases.get(index).copied().unwrap_or(Case::Title);
                state.update();
            }
        });
        row.append(&pick);
        let fields = self.option_row(&tr("Fields"));
        self.field_checks(&fields, &TEXT_FIELDS, |o| &mut o.case_fields);
    }

    fn replace_choices(self: &Rc<Self>) {
        let row = self.option_row(&tr("Find"));
        let (find_text, replace_text, regular, match_case) = {
            let o = self.options.borrow();
            (o.find.clone(), o.replace.clone(), o.regular, o.match_case)
        };
        let find = gtk::Entry::new();
        find.add_css_class("hk-input");
        find.set_hexpand(true);
        find.set_text(&find_text);
        find.update_property(&[gtk::accessible::Property::Label(&tr("Find"))]);
        let weak = Rc::downgrade(self);
        find.connect_changed(move |entry| {
            if let Some(state) = weak.upgrade() {
                state.options.borrow_mut().find = entry.text().to_string();
                state.update();
            }
        });
        row.append(&find);
        let replace = gtk::Entry::new();
        replace.add_css_class("hk-input");
        replace.set_hexpand(true);
        replace.set_text(&replace_text);
        replace.set_placeholder_text(Some(&tr("Replace with")));
        replace.update_property(&[gtk::accessible::Property::Label(&tr("Replace with"))]);
        let weak = Rc::downgrade(self);
        replace.connect_changed(move |entry| {
            if let Some(state) = weak.upgrade() {
                state.options.borrow_mut().replace = entry.text().to_string();
                state.update();
            }
        });
        row.append(&replace);
        let flags = self.option_row("");
        flags.append(&self.check(&tr("Regular expression"), regular, |o, on| {
            o.regular = on;
        }));
        flags.append(
            &self.check(&tr("Match capital letters"), match_case, |o, on| {
                o.match_case = on;
            }),
        );
        let fields = self.option_row(&tr("Fields"));
        self.field_checks(&fields, &TEXT_FIELDS, |o| &mut o.replace_fields);
    }

    fn repair_choices(self: &Rc<Self>) {
        let row = self.option_row(&tr("Text is in"));
        let mut names = vec![tr("Detected: none")];
        names.extend(LEGACY_CHARSETS.iter().map(|c| (*c).to_owned()));
        let chosen = self.options.borrow().charset.clone();
        let now = chosen
            .as_deref()
            .and_then(|c| LEGACY_CHARSETS.iter().position(|l| *l == c))
            .map_or(0, |i| i + 1);
        let pick = dropdown(&names, now, 220);
        pick.update_property(&[gtk::accessible::Property::Label(&tr(
            "The charset the text really has",
        ))]);
        let weak = Rc::downgrade(self);
        pick.connect_selected_notify(move |pick| {
            if let Some(state) = weak.upgrade() {
                let index = usize::try_from(pick.selected()).unwrap_or(0);
                state.options.borrow_mut().charset = index
                    .checked_sub(1)
                    .and_then(|i| LEGACY_CHARSETS.get(i))
                    .map(|c| (*c).to_owned());
                state.update();
            }
        });
        row.append(&pick);
        self.charsets.replace(Some(pick));
        let hint = note(&tr(
            "Old taggers stored text in the code page of their system. Only values that change are listed.",
        ));
        hint.add_css_class("hk-t3");
        hint.set_margin_start(84);
        self.choices.append(&hint);
    }

    fn sort_choices(self: &Rc<Self>) {
        let row = self.option_row(&tr("Forms"));
        let forms = self.options.borrow().forms;
        row.append(
            &self.check(&tr("“Beatles, The”"), forms.article_last, |o, on| {
                o.forms.article_last = on
            }),
        );
        row.append(
            &self.check(&tr("“Surname, Name”"), forms.surname_first, |o, on| {
                o.forms.surname_first = on
            }),
        );
        let fields = self.option_row(&tr("For"));
        let sources: Vec<TagField> = sort_fields().into_iter().map(|(f, _)| f).collect();
        self.field_checks(&fields, &sources, |o| &mut o.sort_fields);
        let hint = note(&tr("Only where the sort tag is empty."));
        hint.add_css_class("hk-t3");
        hint.set_margin_start(84);
        self.choices.append(&hint);
    }
}

/// Fills `table` with the header and a row per file of `preview`. Each row
/// has a band behind it (spanning every column) that carries its height and
/// its bottom line; the cells sit on it.
fn fill_table(table: &gtk::Grid, preview: &Preview) {
    while let Some(child) = table.first_child() {
        table.remove(&child);
    }
    // The file name takes the spare room when no other column does.
    let file_width = if preview.columns.iter().any(|(_, w, _)| *w < 0) {
        FILE
    } else {
        -1
    };
    let mut columns = vec![(tr("File name"), file_width, false)];
    columns.extend(preview.columns.iter().cloned());
    let span = i32::try_from(columns.len()).unwrap_or(1);
    let last = span - 1;
    // The header.
    let band = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    band.add_css_class("hk-tagtool-head");
    table.attach(&band, 0, 0, span, 1);
    for (i, (title, width, right)) in columns.iter().enumerate() {
        let i = i32::try_from(i).unwrap_or(0);
        let cell = label(title, &["hk-section-label"]);
        place(table, &cell, i, 0, last, *width, *right);
    }
    for (r, row) in preview.rows.iter().enumerate() {
        let y = i32::try_from(r).unwrap_or(0) + 1;
        let band = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        band.add_css_class("hk-tagtool-row");
        table.attach(&band, 0, y, span, 1);
        let file = label(&row.file, &["hk-mono", "hk-t2", "hk-tagtool-file"]);
        file.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        file.set_max_width_chars(1);
        place(table, &file, 0, y, last, file_width, false);
        for (c, (cell, (_, width, right))) in row.cells.iter().zip(&preview.columns).enumerate() {
            let x = i32::try_from(c).unwrap_or(0) + 1;
            let shown = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            match cell {
                PreviewCell::New { new, old } => {
                    if let Some(old) = old.as_ref().filter(|o| !o.is_empty()) {
                        shown.append(&label(old, &["hk-t3"]));
                        shown.append(&label("→", &["hk-t3"]));
                    }
                    shown.append(&label(new, &["hk-medium"]));
                }
                PreviewCell::Kept(value) => shown.append(&label(value, &["hk-t3"])),
                PreviewCell::Blank => {}
            }
            if row.note.is_some() && c + 1 == preview.columns.len() {
                // In the last column, as the drawing shows it.
                shown.append(&label(row.note.as_deref().unwrap_or_default(), &["hk-t3"]));
            }
            place(table, &shown, x, y, last, *width, *right);
        }
    }
}

/// Puts `cell` at column `x` of row `y`: `width` wide, or sharing the rest
/// for -1; the first and last columns keep the table's side padding.
fn place(
    table: &gtk::Grid,
    cell: &impl IsA<gtk::Widget>,
    x: i32,
    y: i32,
    last: i32,
    width: i32,
    right: bool,
) {
    let cell = cell.as_ref();
    // A right-aligned value keeps to the right edge; the header's label holds
    // the column's width.
    let is_label = cell.is::<gtk::Label>();
    cell.set_size_request(if right && !is_label { -1 } else { width.max(0) }, -1);
    cell.set_hexpand(width < 0);
    cell.set_valign(gtk::Align::Center);
    cell.set_halign(if right {
        gtk::Align::End
    } else {
        gtk::Align::Fill
    });
    if let Some(text) = cell.downcast_ref::<gtk::Label>() {
        text.set_xalign(if right { 1.0 } else { 0.0 });
    }
    if x == 0 {
        cell.set_margin_start(24);
    }
    if x == last {
        cell.set_margin_end(24);
    }
    table.attach(cell, x, y, 1, 1);
}
