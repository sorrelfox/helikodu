// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tag editor: one track or several. The file list at the left, then the
//! tabs Tags, Cover, Lyrics and File info, and a footer with Cancel and
//! "Save N files".
//!
//! - **Several tracks.** A field whose files disagree is empty with the
//!   placeholder `<keep>` and "N different values" beside its label; leaving
//!   it untouched keeps each file's value.
//! - **Edited fields** get an accent border and "Edited" with a dot beside
//!   the label, so the change does not rely on colour.
//! - **Rating** in whole stars, kept in the library.
//! - **Saving** writes through the library, file by file, off the main
//!   thread. Files that fail are marked in the list with the reason; a toast
//!   in the main window says how it went.
//! - Files whose tags cannot be written show their fields insensitive, with
//!   a notice that says why.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::common::{bytes, compact_button, dialog_window, note, tilde};
use crate::backend::services::{CoverEdit, EditTrack, TagEdit, TagField, WriteResult};
use crate::backend::{Backend, ThumbnailState, TrackKey};
use crate::i18n::{fill, ntr, tr, trc};
use crate::tools::tag_preview::{Tool, ToolTrack, ToolValues};
use crate::tools::tag_tools;
use crate::widgets::{CoverArt, RatingStars, icon_button, label};
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "tag-editor";

/// The label of a form field.
fn field_title(field: &TagField) -> String {
    match field {
        TagField::Title => tr("Title"),
        TagField::Artist => tr("Artist"),
        TagField::AlbumArtist => tr("Album artist"),
        TagField::Album => tr("Album"),
        // Translators: the field of the track number, "Track [3] of [12]".
        TagField::Track | TagField::TrackTotal => trc("track number", "Track"),
        TagField::Disc | TagField::DiscTotal => tr("Disc"),
        TagField::Year => tr("Year"),
        TagField::Genre => tr("Genre"),
        TagField::Composer => tr("Composer"),
        TagField::Comment => tr("Comment"),
        TagField::Custom(name) => name.clone(),
    }
}

/// Fields with several values are shown joined by this.
const SEPARATOR: &str = "; ";

/// What a selection holds in one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Shared {
    /// Every file has these values.
    Same(Vec<String>),
    /// The files differ: this many different values.
    Mixed(usize),
}

/// What `tracks` hold in `field`.
pub(crate) fn shared(tracks: &[EditTrack], field: &TagField) -> Shared {
    let mut seen: Vec<&[String]> = Vec::new();
    for track in tracks {
        let values = track.values(field);
        if !seen.contains(&values) {
            seen.push(values);
        }
    }
    match seen.as_slice() {
        [] => Shared::Same(Vec::new()),
        [one] => Shared::Same(one.to_vec()),
        many => Shared::Mixed(many.len()),
    }
}

/// The values typed into a field.
fn parse(field: &TagField, text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    match field {
        TagField::Artist | TagField::AlbumArtist | TagField::Composer => text
            .split(';')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => vec![text.to_owned()],
    }
}

/// A date as its tag means it: the year, the year and month, or the full
/// date, without the time and zone some formats add (MP4 `©day` is often
/// `2004-07-02T12:00:00Z`). Anything else is shown as it is.
pub(crate) fn shown_date(raw: &str) -> String {
    let raw = raw.trim();
    let date_end = raw.find(['T', ' ']).unwrap_or(raw.len());
    let (date, rest) = raw.split_at(date_end);
    if valid_date(date) && (rest.is_empty() || rest.len() > 1) {
        date.to_owned()
    } else {
        raw.to_owned()
    }
}

/// Whether `text` is a date every tag format takes: `YYYY`, `YYYY-MM` or
/// `YYYY-MM-DD`.
pub(crate) fn valid_date(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    let number = |part: &str, len: usize| {
        (part.len() == len && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    let year = parts.first().and_then(|p| number(p, 4));
    let month = parts.get(1).map(|p| number(p, 2));
    let day = parts.get(2).map(|p| number(p, 2));
    match (year, month, day, parts.len()) {
        (Some(y), None, None, 1) => y > 0,
        (Some(y), Some(Some(m)), None, 2) => y > 0 && (1..=12).contains(&m),
        (Some(y), Some(Some(m)), Some(Some(d)), 3) => {
            let days = match m {
                2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
                2 => 28,
                4 | 6 | 9 | 11 => 30,
                _ => 31,
            };
            y > 0 && (1..=12).contains(&m) && (1..=days).contains(&d)
        }
        _ => false,
    }
}

/// The text a field shows for `values`.
fn shown(field: &TagField, values: &[String]) -> String {
    if *field == TagField::Year {
        let dates: Vec<String> = values.iter().map(|v| shown_date(v)).collect();
        dates.join(SEPARATOR)
    } else {
        values.join(SEPARATOR)
    }
}

/// Whether `now`, typed into `field`, is what it started with: the stored
/// `values`, or the same values as shown.
fn unchanged(field: &TagField, values: &[String], now: &[String]) -> bool {
    values == now || parse(field, &shown(field, values)) == now
}

/// One field of the form: its entry (or genre chips), its label row and the
/// value it started with.
struct FieldInput {
    field: TagField,
    entry: Option<gtk::Entry>,
    chips: Option<Rc<Chips>>,
    start: Shared,
    edited_mark: gtk::Box,
    frame: gtk::Widget,
    /// How many different values the tag tools gave this field, file by
    /// file; 0 when they gave none.
    pending: Cell<usize>,
}

impl FieldInput {
    /// The new values, or `None` when the field is untouched.
    fn change(&self) -> Option<Vec<String>> {
        let now = match (&self.entry, &self.chips) {
            (Some(entry), _) => {
                let text = entry.text();
                if (matches!(self.start, Shared::Mixed(_)) || self.pending.get() > 0)
                    && text.is_empty()
                {
                    return None;
                }
                parse(&self.field, &text)
            }
            (None, Some(chips)) => {
                if !chips.touched.get() {
                    return None;
                }
                chips.values.borrow().clone()
            }
            (None, None) => return None,
        };
        match &self.start {
            // A value shown shorter than it is stored (a date without its
            // time) and left as shown keeps what the file holds.
            Shared::Same(values) if unchanged(&self.field, values, &now) => None,
            _ => Some(now),
        }
    }

    /// Whether the field holds a value its tag cannot take.
    fn invalid(&self) -> bool {
        self.field == TagField::Year
            && self
                .change()
                .is_some_and(|values| values.iter().any(|v| !valid_date(v)))
    }

    fn refresh(&self) {
        if let Some(entry) = &self.entry {
            let invalid = self.invalid();
            if invalid {
                entry.add_css_class("error");
                entry.set_tooltip_text(Some(&tr("A year, or a date as YYYY-MM-DD")));
            } else {
                entry.remove_css_class("error");
                entry.set_tooltip_text(None);
            }
            entry.update_state(&[gtk::accessible::State::Invalid(if invalid {
                gtk::AccessibleInvalidState::True
            } else {
                gtk::AccessibleInvalidState::False
            })]);
        }
        let pending = self.pending.get();
        if let Some(entry) = &self.entry {
            if pending > 0 {
                entry.set_placeholder_text(Some(&fill(
                    &ntr("<{} new value>", "<{} new values>", pending),
                    &[&pending.to_string()],
                )));
                entry.add_css_class("hk-keep");
            } else if matches!(self.start, Shared::Mixed(_)) {
                // Translators: shown in an empty field whose files hold
                // different values, which are kept unless something is typed;
                // translate the word, the angle brackets may stay.
                entry.set_placeholder_text(Some(&tr("<keep>")));
            }
        }
        let edited = self.change().is_some() || pending > 0;
        self.edited_mark.set_visible(edited);
        if edited {
            self.frame.add_css_class("edited");
        } else {
            self.frame.remove_css_class("edited");
        }
    }
}

/// The genre field: a chip for each value and an entry to add one.
struct Chips {
    container: gtk::Box,
    entry: gtk::Entry,
    values: RefCell<Vec<String>>,
    touched: Cell<bool>,
    on_change: RefCell<Option<Box<dyn Fn()>>>,
}

impl Chips {
    fn new(values: Vec<String>, placeholder: Option<&str>) -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        container.add_css_class("hk-input");
        container.add_css_class("hk-chips");
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-chip-entry");
        entry.set_hexpand(true);
        entry.set_has_frame(false);
        entry.set_placeholder_text(Some(placeholder.unwrap_or(&tr("Add genre"))));
        entry.update_property(&[gtk::accessible::Property::Label(&tr("Add genre"))]);
        container.append(&entry);
        let chips = Rc::new(Self {
            container,
            entry,
            values: RefCell::new(values),
            touched: Cell::new(false),
            on_change: RefCell::new(None),
        });
        let weak = Rc::downgrade(&chips);
        chips.entry.connect_activate(move |entry| {
            let Some(chips) = weak.upgrade() else { return };
            let text = entry.text().trim().to_owned();
            if text.is_empty() {
                return;
            }
            if !chips.values.borrow().contains(&text) {
                chips.values.borrow_mut().push(text);
            }
            entry.set_text("");
            chips.changed();
        });
        chips.draw();
        chips
    }

    fn changed(self: &Rc<Self>) {
        self.touched.set(true);
        self.draw();
        if let Some(on_change) = self.on_change.borrow().as_ref() {
            on_change();
        }
    }

    fn draw(self: &Rc<Self>) {
        while let Some(child) = self.container.first_child() {
            if child == self.entry.clone().upcast::<gtk::Widget>() {
                break;
            }
            self.container.remove(&child);
        }
        for value in self.values.borrow().iter().rev() {
            let chip = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            chip.add_css_class("hk-chip");
            chip.append(&label(value, &[]));
            let remove = gtk::Button::from_icon_name("window-close-symbolic");
            remove.add_css_class("hk-chip-remove");
            let name = fill(&tr("Remove {}"), &[value]);
            remove.set_tooltip_text(Some(&name));
            remove.update_property(&[gtk::accessible::Property::Label(&name)]);
            let weak = Rc::downgrade(self);
            let value = value.clone();
            remove.connect_clicked(move |_| {
                if let Some(chips) = weak.upgrade() {
                    chips.values.borrow_mut().retain(|v| *v != value);
                    chips.changed();
                }
            });
            chip.append(&remove);
            self.container.prepend(&chip);
        }
    }
}

/// The editor's state.
struct Editor {
    window: gtk::Window,
    main: glib::WeakRef<MainWindow>,
    backend: Backend,
    tracks: Vec<EditTrack>,
    inputs: RefCell<Vec<FieldInput>>,
    others: RefCell<Vec<(String, String, gtk::Entry)>>,
    rating: Cell<Option<Option<u8>>>,
    cover: RefCell<Option<CoverEdit>>,
    lyrics: RefCell<Option<gtk::TextView>>,
    lyrics_start: String,
    rows: RefCell<Vec<gtk::ListBoxRow>>,
    save: gtk::Button,
    problems: gtk::Revealer,
    problems_text: gtk::Label,
    info: gtk::Box,
    cover_tab: gtk::Box,
    tab_buttons: RefCell<Vec<gtk::ToggleButton>>,
    /// Values from the tag tools that differ file by file.
    pending: RefCell<ToolValues>,
    /// "Keep the cover": inside each file, and as a file in the folder.
    cover_inside: Cell<bool>,
    cover_in_folder: Cell<bool>,
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("tag_editor::Editor")
            .field("tracks", &self.tracks.len())
            .finish_non_exhaustive()
    }
}

impl Editor {
    /// Why nothing can be changed: no file of the selection can be written.
    fn read_only(&self) -> Option<&str> {
        if self.tracks.iter().all(|t| t.read_only.is_some()) {
            self.tracks.iter().find_map(|t| t.read_only.as_deref())
        } else {
            None
        }
    }

    /// The files of a selection that cannot be written, when others can.
    fn some_read_only(&self) -> Vec<&EditTrack> {
        if self.read_only().is_some() {
            return Vec::new();
        }
        self.tracks
            .iter()
            .filter(|t| t.read_only.is_some())
            .collect()
    }

    /// The changes made so far.
    fn edit(&self) -> TagEdit {
        let mut fields = BTreeMap::new();
        for input in self.inputs.borrow().iter() {
            if let Some(values) = input.change() {
                fields.insert(input.field.clone(), values);
            }
        }
        for (name, start, entry) in self.others.borrow().iter() {
            let text = entry.text().trim().to_owned();
            if text != *start {
                fields.insert(
                    TagField::Custom(name.clone()),
                    if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![text]
                    },
                );
            }
        }
        let lyrics = self.lyrics.borrow().as_ref().and_then(|view| {
            let buffer = view.buffer();
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            (text != self.lyrics_start).then_some(text)
        });
        TagEdit {
            tracks: self.tracks.iter().map(|t| t.key).collect(),
            fields,
            cover: self.cover.borrow().clone(),
            lyrics,
            rating: self.rating.get(),
            per_track: self.pending.borrow().clone(),
            cover_not_inside: !self.cover_inside.get(),
            cover_in_folder: self.cover_in_folder.get(),
        }
    }

    /// The files as the tag tools see them: their values with the edits made
    /// so far.
    fn tool_tracks(&self) -> Vec<ToolTrack> {
        let edit = self.edit();
        self.tracks
            .iter()
            .map(|track| {
                let mut values = track.fields.clone();
                for (name, value) in &track.other {
                    let values_of: Vec<String> = value
                        .split(';')
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned)
                        .collect();
                    values.insert(TagField::Custom(name.clone()), values_of);
                }
                for (field, now) in edit.fields_for(track.key) {
                    values.insert(field, now);
                }
                ToolTrack {
                    key: track.key,
                    path: track.path.clone(),
                    values,
                    raw: track.raw.clone(),
                }
            })
            .collect()
    }

    /// Opens Tag tools at `tool` for the files of the editor.
    fn open_tools(self: &Rc<Self>, tool: Tool) {
        let weak = Rc::downgrade(self);
        tag_tools::open(&self.window, self.tool_tracks(), tool, move |values| {
            if let Some(editor) = weak.upgrade() {
                editor.apply_tool(values);
            }
        });
    }

    /// Takes the values the tag tools give: a field that gets one value for
    /// every file is filled in the form; others are kept file by file and
    /// shown as "N new values".
    fn apply_tool(&self, values: ToolValues) {
        let mut fields: Vec<TagField> = Vec::new();
        {
            let mut pending = self.pending.borrow_mut();
            for (track, changes) in values {
                for (field, new) in changes {
                    if !fields.contains(&field) {
                        fields.push(field.clone());
                    }
                    pending.entry(track).or_default().insert(field, new);
                }
            }
        }
        for field in fields {
            let mut seen: Vec<Vec<String>> = Vec::new();
            let mut everyone = true;
            for track in &self.tracks {
                match self
                    .pending
                    .borrow()
                    .get(&track.key)
                    .and_then(|f| f.get(&field))
                {
                    Some(values) => {
                        if !seen.contains(values) {
                            seen.push(values.clone());
                        }
                    }
                    None => everyone = false,
                }
            }
            let inputs = self.inputs.borrow();
            let Some(input) = inputs.iter().find(|i| i.field == field) else {
                continue;
            };
            if everyone && seen.len() == 1 {
                // One value for all: an ordinary edit of the form.
                for changes in self.pending.borrow_mut().values_mut() {
                    changes.remove(&field);
                }
                input.pending.set(0);
                if let Some(entry) = &input.entry {
                    entry.set_text(&shown(&field, &seen[0]));
                } else if let Some(chips) = &input.chips {
                    chips.values.replace(seen[0].clone());
                    chips.changed();
                }
            } else {
                input.pending.set(seen.len());
                if let Some(entry) = &input.entry {
                    entry.set_text("");
                }
            }
        }
        self.pending.borrow_mut().retain(|_, f| !f.is_empty());
        self.refresh();
    }

    fn refresh(&self) {
        for input in self.inputs.borrow().iter() {
            input.refresh();
        }
        let edit = self.edit();
        let invalid = self.inputs.borrow().iter().any(FieldInput::invalid);
        self.save.set_sensitive(!edit.is_empty() && !invalid);
        let marks = [
            !edit.fields.is_empty() || edit.rating.is_some() || !edit.per_track.is_empty(),
            edit.cover.is_some() && !edit.is_empty(),
            edit.lyrics.is_some(),
            false,
        ];
        for (button, edited) in self.tab_buttons.borrow().iter().zip(marks) {
            if edited {
                button.add_css_class("edited");
            } else {
                button.remove_css_class("edited");
            }
        }
    }

    fn save(self: &Rc<Self>) {
        let edit = self.edit();
        if edit.is_empty() {
            self.window.close();
            return;
        }
        self.save.set_sensitive(false);
        let services = self.backend.services();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let job = edit.clone();
            let results = gio::spawn_blocking(move || services.write_tags(&job))
                .await
                .unwrap_or_default();
            if let Some(editor) = weak.upgrade() {
                editor.saved(&edit, &results);
            }
        });
    }

    fn saved(&self, edit: &TagEdit, results: &[WriteResult]) {
        let failed: Vec<&WriteResult> = results.iter().filter(|r| r.error.is_some()).collect();
        let main = self.main.upgrade();
        for (row, track) in self.rows.borrow().iter().zip(&self.tracks) {
            let problem = results
                .iter()
                .find(|r| r.track == track.key)
                .and_then(|r| r.error.clone());
            if let Some(mark) = row
                .child()
                .and_then(|c| c.last_child())
                .and_downcast::<gtk::Image>()
            {
                mark.set_visible(problem.is_some());
                mark.set_tooltip_text(problem.as_deref());
            }
        }
        if failed.is_empty() {
            if let Some(main) = &main {
                let n = edit.tracks.len();
                let text = fill(
                    &ntr("Saved {} file", "Saved {} files", n),
                    &[&n.to_string()],
                );
                if edit.touches_files() {
                    main.toast_tags_undo(&text);
                } else {
                    main.toast(&text, false);
                }
                if let Some(stars) = edit.rating {
                    main.ratings_changed(&edit.tracks, stars);
                }
            }
            self.window.close();
            return;
        }
        let lines: Vec<String> = failed
            .iter()
            .map(|r| format!("{}: {}", r.file, r.error.as_deref().unwrap_or_default()))
            .collect();
        self.problems_text.set_text(&lines.join("\n"));
        self.problems.set_reveal_child(true);
        self.save.set_sensitive(true);
        if let Some(main) = &main {
            main.toast(
                &fill(
                    &ntr(
                        // Translators: {0} is the number of files that failed,
                        // which chooses the form, {1} the number of all files.
                        "{} of {} files could not be saved",
                        "{} of {} files could not be saved",
                        failed.len(),
                    ),
                    &[&failed.len().to_string(), &results.len().to_string()],
                ),
                true,
            );
        }
    }
}

/// A form field: its label row over the entry.
fn form_field(
    editor: &Rc<Editor>,
    field: TagField,
    total: Option<TagField>,
    sensitive: bool,
) -> (gtk::Box, Vec<FieldInput>) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.append(&label(&field_title(&field), &["hk-field-label"]));
    let start = shared(&editor.tracks, &field);
    let mut mixed = match &start {
        Shared::Mixed(n) => Some(*n),
        Shared::Same(_) => None,
    };
    if let Some(total) = &total
        && let Shared::Mixed(n) = shared(&editor.tracks, total)
    {
        mixed = Some(mixed.unwrap_or(0).max(n));
    }
    if let Some(n) = mixed {
        head.append(&label(
            &fill(
                &ntr("{} different value", "{} different values", n),
                &[&n.to_string()],
            ),
            &["hk-t3", "hk-secondary"],
        ));
    }
    let edited = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("hk-edited-dot");
    dot.set_valign(gtk::Align::Center);
    edited.append(&dot);
    edited.append(&label(&tr("Edited"), &["hk-accent-text", "hk-secondary"]));
    edited.set_visible(false);
    head.append(&edited);
    column.append(&head);

    let make_entry = |field: &TagField, start: &Shared| {
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.set_hexpand(true);
        entry.set_sensitive(sensitive);
        entry.update_property(&[gtk::accessible::Property::Label(&field_title(field))]);
        match start {
            Shared::Same(values) => entry.set_text(&shown(field, values)),
            Shared::Mixed(_) => {
                entry.set_placeholder_text(Some(&tr("<keep>")));
                entry.add_css_class("hk-keep");
            }
        }
        entry
    };

    let mut inputs = Vec::new();
    if field == TagField::Genre {
        let values = match &start {
            Shared::Same(values) => values.clone(),
            Shared::Mixed(_) => Vec::new(),
        };
        let keep = tr("<keep>");
        let chips = Chips::new(
            values,
            matches!(start, Shared::Mixed(_)).then_some(keep.as_str()),
        );
        chips.container.set_sensitive(sensitive);
        let weak = Rc::downgrade(editor);
        chips.on_change.replace(Some(Box::new(move || {
            if let Some(editor) = weak.upgrade() {
                editor.refresh();
            }
        })));
        column.append(&chips.container);
        inputs.push(FieldInput {
            field: field.clone(),
            entry: None,
            frame: chips.container.clone().upcast(),
            chips: Some(chips),
            start,
            edited_mark: edited,
            pending: Cell::new(0),
        });
    } else if let Some(total) = total {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let entry = make_entry(&field, &start);
        line.append(&entry);
        line.append(&label(&tr("of"), &["hk-t2"]));
        let total_start = shared(&editor.tracks, &total);
        let total_entry = make_entry(&total, &total_start);
        total_entry.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{}, total"),
            &[&field_title(&field)],
        ))]);
        line.append(&total_entry);
        column.append(&line);
        for (field, entry, start) in [(field, entry, start), (total, total_entry, total_start)] {
            inputs.push(FieldInput {
                field,
                frame: entry.clone().upcast(),
                entry: Some(entry),
                chips: None,
                start,
                edited_mark: edited.clone(),
                pending: Cell::new(0),
            });
        }
    } else {
        let entry = make_entry(&field, &start);
        column.append(&entry);
        inputs.push(FieldInput {
            field,
            frame: entry.clone().upcast(),
            entry: Some(entry),
            chips: None,
            start,
            edited_mark: edited,
            pending: Cell::new(0),
        });
    }
    for input in &inputs {
        if let Some(entry) = &input.entry {
            let weak = Rc::downgrade(editor);
            entry.connect_changed(move |_| {
                if let Some(editor) = weak.upgrade() {
                    editor.refresh();
                }
            });
        }
    }
    (column, inputs)
}

/// Whole stars that a click or the arrow keys set; clicking the current
/// value clears it.
fn rating_editor(editor: &Rc<Editor>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.append(&label(&tr("Rating"), &["hk-field-label"]));
    let start = {
        let mut seen: Vec<Option<u8>> = Vec::new();
        for track in &editor.tracks {
            if !seen.contains(&track.rating) {
                seen.push(track.rating);
            }
        }
        if seen.len() > 1 {
            head.append(&label(
                &fill(
                    &ntr("{} different value", "{} different values", seen.len()),
                    &[&seen.len().to_string()],
                ),
                &["hk-t3", "hk-secondary"],
            ));
            None
        } else {
            seen.first().copied()
        }
    };
    column.append(&head);
    let frame = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    frame.add_css_class("hk-rating-field");
    frame.set_focusable(true);
    frame.set_halign(gtk::Align::Start);
    frame.set_accessible_role(gtk::AccessibleRole::Slider);
    let stars = RatingStars::default();
    stars.set_rating(start.flatten());
    stars.set_valign(gtk::Align::Center);
    frame.append(&stars);
    let text = label("", &["hk-t3", "hk-secondary"]);
    frame.append(&text);
    let show = {
        let stars = stars.clone();
        let frame = frame.clone();
        move |value: Option<u8>| {
            stars.set_rating(value);
            let words = match value {
                Some(n) => fill(
                    &ntr("{} of 5 stars", "{} of 5 stars", usize::from(n)),
                    &[&n.to_string()],
                ),
                None => tr("Not rated"),
            };
            text.set_text(&words);
            frame.update_property(&[
                gtk::accessible::Property::Label(&tr("Rating")),
                gtk::accessible::Property::ValueMin(0.0),
                gtk::accessible::Property::ValueMax(5.0),
                gtk::accessible::Property::ValueNow(f64::from(value.unwrap_or(0))),
                gtk::accessible::Property::ValueText(&words),
            ]);
        }
    };
    show(start.flatten());
    let show = Rc::new(show);
    let set = {
        let weak = Rc::downgrade(editor);
        let show = show.clone();
        move |value: Option<u8>| {
            let Some(editor) = weak.upgrade() else { return };
            show(value);
            editor.rating.set((start != Some(value)).then_some(value));
            editor.refresh();
        }
    };
    let set = Rc::new(set);
    let click = gtk::GestureClick::new();
    let current = Rc::new(Cell::new(start.flatten()));
    let (c, s) = (current.clone(), set.clone());
    let weak_stars = stars.downgrade();
    click.connect_pressed(move |_, _, x, _| {
        let Some(stars) = weak_stars.upgrade() else {
            return;
        };
        if x > f64::from(stars.width()) {
            return;
        }
        let star = ((x / 13.0).floor() as u8 + 1).min(5);
        let value = if c.get() == Some(star) {
            None
        } else {
            Some(star)
        };
        c.set(value);
        s(value);
    });
    stars.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    let (c, s) = (current, set);
    keys.connect_key_pressed(move |_, key, _, _| {
        let now = c.get().unwrap_or(0);
        let next = match key {
            gdk::Key::Right | gdk::Key::Up => (now + 1).min(5),
            gdk::Key::Left | gdk::Key::Down => now.saturating_sub(1),
            gdk::Key::Home | gdk::Key::_0 | gdk::Key::BackSpace | gdk::Key::Delete => 0,
            gdk::Key::End => 5,
            gdk::Key::_1 => 1,
            gdk::Key::_2 => 2,
            gdk::Key::_3 => 3,
            gdk::Key::_4 => 4,
            gdk::Key::_5 => 5,
            _ => return glib::Propagation::Proceed,
        };
        let value = (next > 0).then_some(next);
        c.set(value);
        s(value);
        glib::Propagation::Stop
    });
    frame.add_controller(keys);
    column.append(&frame);
    column
}

fn tags_tab(editor: &Rc<Editor>) -> gtk::Widget {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 14);
    body.add_css_class("hk-tag-form");
    let sensitive = editor.read_only().is_none();
    if let Some(reason) = editor.read_only() {
        let notice = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        notice.add_css_class("hk-notice");
        notice.append(&gtk::Image::from_icon_name("dialog-information-symbolic"));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.append(&label(
            &tr("The tags of these files cannot be changed here"),
            &["hk-medium"],
        ));
        let why = note(reason);
        text.append(&why);
        notice.append(&text);
        body.append(&notice);
    }
    let fixed = editor.some_read_only();
    if let Some(first) = fixed.first() {
        let notice = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        notice.add_css_class("hk-notice");
        notice.append(&gtk::Image::from_icon_name("dialog-information-symbolic"));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.append(&label(
            &fill(
                &ntr(
                    "{} file keeps its tags",
                    "{} files keep their tags",
                    fixed.len(),
                ),
                &[&fixed.len().to_string()],
            ),
            &["hk-medium"],
        ));
        text.append(&note(&format!(
            "{}: {}",
            first.file_name(),
            first.read_only.as_deref().unwrap_or_default()
        )));
        notice.append(&text);
        body.append(&notice);
    }
    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(14);
    grid.set_column_homogeneous(true);
    let layout: [(TagField, Option<TagField>, i32, i32, i32); 10] = [
        (TagField::Title, None, 0, 0, 6),
        (TagField::Artist, None, 0, 1, 3),
        (TagField::AlbumArtist, None, 3, 1, 3),
        (TagField::Album, None, 0, 2, 6),
        (TagField::Track, Some(TagField::TrackTotal), 0, 3, 2),
        (TagField::Disc, Some(TagField::DiscTotal), 2, 3, 2),
        (TagField::Year, None, 4, 3, 2),
        (TagField::Genre, None, 0, 4, 6),
        (TagField::Composer, None, 0, 5, 3),
        (TagField::Comment, None, 3, 5, 3),
    ];
    let mut inputs = Vec::new();
    for (field, total, column, row, span) in layout {
        let (cell, mut made) = form_field(editor, field, total, sensitive);
        grid.attach(&cell, column, row, span, 1);
        inputs.append(&mut made);
    }
    grid.attach(&rating_editor(editor), 0, 6, 2, 1);
    editor.inputs.replace(inputs);
    body.append(&grid);

    // Other fields, when the files agree on them.
    let first = editor.tracks.first();
    let others: Vec<(String, String)> = first.map(|t| t.other.clone()).unwrap_or_default();
    let others: Vec<(String, String)> = others
        .into_iter()
        .filter(|(name, value)| {
            editor
                .tracks
                .iter()
                .all(|t| t.other.iter().any(|(n, v)| n == name && v == value))
        })
        .collect();
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("hk-others-head");
    let title = label(&tr("Other fields"), &["hk-section-label"]);
    title.set_hexpand(true);
    head.append(&title);
    let add = gtk::Button::new();
    add.add_css_class("hk-btn");
    add.add_css_class("text");
    let add_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    add_line.append(&gtk::Image::from_icon_name("list-add-symbolic"));
    add_line.append(&gtk::Label::new(Some(&tr("Add field"))));
    add.set_child(Some(&add_line));
    // Adding fields comes with the tag tools.
    add.set_sensitive(false);
    head.append(&add);
    body.append(&head);
    let mut entries = Vec::new();
    for (name, value) in others {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.add_css_class("hk-other-row");
        let key = label(&name, &["hk-mono-meta", "hk-t2", "hk-field-key"]);
        key.set_size_request(220, -1);
        line.append(&key);
        let entry = gtk::Entry::new();
        entry.set_has_frame(false);
        entry.add_css_class("hk-plain-entry");
        entry.set_text(&value);
        entry.set_hexpand(true);
        entry.set_sensitive(sensitive);
        entry.update_property(&[gtk::accessible::Property::Label(&name)]);
        let weak = Rc::downgrade(editor);
        entry.connect_changed(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.refresh();
            }
        });
        line.append(&entry);
        body.append(&line);
        entries.push((name, value, entry));
    }
    editor.others.replace(entries);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&body));
    scroller.upcast()
}

/// The texture of `track`'s cover, if its thumbnail is at hand.
fn cover_texture(backend: &Backend, track: &EditTrack) -> Option<gdk::Texture> {
    let cover = track.cover?;
    match backend.library().thumbnail(&cover, 384) {
        ThumbnailState::Ready(path) => gdk::Texture::from_filename(path).ok(),
        _ => None,
    }
}

fn fill_cover_tab(editor: &Rc<Editor>) {
    let tab = &editor.cover_tab;
    while let Some(child) = tab.first_child() {
        tab.remove(&child);
    }
    let art_column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let pending = editor.cover.borrow().clone();
    let first = editor.tracks.first();
    let picture: gtk::Widget = match &pending {
        Some(CoverEdit::Set(path)) => {
            let picture = gtk::Picture::for_filename(path);
            picture.set_size_request(240, 240);
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.add_css_class("hk-cover");
            picture.add_css_class("hero");
            picture.upcast()
        }
        _ => {
            let art = CoverArt::new(240, 8.0);
            art.add_css_class("hero");
            let texture = if pending.is_some() {
                None
            } else {
                first.and_then(|t| cover_texture(&editor.backend, t))
            };
            art.show(first.map_or(0, |t| t.cover_seed), texture);
            art.upcast()
        }
    };
    art_column.append(&picture);
    let caption = match &pending {
        Some(CoverEdit::Set(path)) => std::fs::metadata(path)
            .map(|m| format!("{} · {}", tilde(path), bytes(m.len())))
            .unwrap_or_default(),
        Some(CoverEdit::Remove) => tr("No cover after saving"),
        None if first.is_some_and(|t| t.cover.is_some()) => String::new(),
        None => tr("No cover: a generated picture stands in"),
    };
    art_column.append(&label(&caption, &["hk-mono-meta", "hk-t3"]));
    tab.append(&art_column);

    let side = gtk::Box::new(gtk::Orientation::Vertical, 10);
    side.set_hexpand(true);
    side.append(&label(&tr("Front cover"), &["hk-section-label"]));
    let with = editor.tracks.iter().filter(|t| t.cover.is_some()).count();
    let total = editor.tracks.len();
    let same = editor
        .tracks
        .iter()
        .all(|t| t.cover == editor.tracks[0].cover);
    let sentence = match &pending {
        Some(CoverEdit::Set(_)) => fill(
            &ntr(
                "The new picture goes into {} file when you save.",
                "The new picture goes into all {} files when you save.",
                total,
            ),
            &[&total.to_string()],
        ),
        Some(CoverEdit::Remove) => tr("The front cover is removed when you save."),
        None if with == 0 => tr("These files have no front cover."),
        None if same && total > 1 => fill(
            &tr("The same picture is inside all {} files"),
            &[&total.to_string()],
        ),
        None if same => tr("The file has a front cover."),
        None => fill(
            &tr("{} files have a front cover, {} have none or another"),
            &[&with.to_string(), &(total - with).to_string()],
        ),
    };
    let text = note(&sentence);
    text.remove_css_class("hk-note");
    text.add_css_class("hk-t2");
    side.append(&text);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let choose = compact_button(&tr("Choose image…"));
    let save_as = compact_button(&tr("Save as a file…"));
    let remove = compact_button(&tr("Remove"));
    let sensitive = editor.read_only().is_none();
    choose.set_sensitive(sensitive);
    remove.set_sensitive(sensitive && (with > 0 || matches!(pending, Some(CoverEdit::Set(_)))));
    let weak = Rc::downgrade(editor);
    choose.connect_clicked(move |_| {
        let Some(editor) = weak.upgrade() else { return };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&tr("JPEG and PNG images")));
        filter.add_mime_type("image/jpeg");
        filter.add_mime_type("image/png");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(tr("Choose a cover image"))
            .filters(&filters)
            .modal(true)
            .build();
        let weak = Rc::downgrade(&editor);
        dialog.open(
            Some(&editor.window),
            gio::Cancellable::NONE,
            move |result| {
                if let (Some(editor), Ok(Some(path))) = (weak.upgrade(), result.map(|f| f.path())) {
                    editor.cover.replace(Some(CoverEdit::Set(path)));
                    fill_cover_tab(&editor);
                    editor.refresh();
                }
            },
        );
    });
    buttons.append(&choose);
    save_as.set_sensitive(pending.is_none() && first.is_some_and(|t| t.cover.is_some()));
    let weak = Rc::downgrade(editor);
    save_as.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            save_cover_as(&editor);
        }
    });
    buttons.append(&save_as);
    let weak = Rc::downgrade(editor);
    remove.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            let had = editor.tracks.iter().any(|t| t.cover.is_some());
            editor
                .cover
                .replace(if had { Some(CoverEdit::Remove) } else { None });
            fill_cover_tab(&editor);
            editor.refresh();
        }
    });
    buttons.append(&remove);
    if pending.is_some() {
        let keep = compact_button(&tr("Keep the cover"));
        let weak = Rc::downgrade(editor);
        keep.connect_clicked(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.cover.replace(None);
                fill_cover_tab(&editor);
                editor.refresh();
            }
        });
        buttons.append(&keep);
    }
    side.append(&buttons);
    side.append(&drop_area(editor, sensitive));
    side.append(&keep_cover(editor, sensitive));
    tab.append(&side);
}

/// The area a picture can be dropped on to make it the front cover.
fn drop_area(editor: &Rc<Editor>, sensitive: bool) -> gtk::Box {
    let area = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    area.add_css_class("hk-cover-drop");
    let text = label(&tr("or drop a picture here"), &[]);
    text.set_hexpand(true);
    area.append(&text);
    area.set_sensitive(sensitive);
    let target = gtk::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
    let weak = Rc::downgrade(editor);
    target.connect_drop(move |_, value, _, _| {
        let (Some(editor), Ok(file)) = (weak.upgrade(), value.get::<gio::File>()) else {
            return false;
        };
        let Some(path) = file.path() else {
            return false;
        };
        editor.cover.replace(Some(CoverEdit::Set(path)));
        fill_cover_tab(&editor);
        editor.refresh();
        true
    });
    let weak = area.downgrade();
    target.connect_enter(move |_, _, _| {
        if let Some(area) = weak.upgrade() {
            area.add_css_class("hk-drop-hover");
        }
        gdk::DragAction::COPY
    });
    let weak = area.downgrade();
    target.connect_leave(move |_| {
        if let Some(area) = weak.upgrade() {
            area.remove_css_class("hk-drop-hover");
        }
    });
    area.add_controller(target);
    area
}

/// "Keep the cover": inside each file, and as `cover.jpg` in the album
/// folder; each with a line on what it means.
fn keep_cover(editor: &Rc<Editor>, sensitive: bool) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.add_css_class("hk-keep-cover");
    column.append(&label(
        // Translators: the heading of the section that saves the cover.
        &trc("heading", "Keep the cover"),
        &["hk-section-label"],
    ));
    type Choice = (String, String, bool, fn(&Editor, bool));
    let choices: [Choice; 2] = [
        (
            tr("Inside each file"),
            tr("It travels with the file."),
            editor.cover_inside.get(),
            |e, on| e.cover_inside.set(on),
        ),
        (
            tr("As cover.jpg in the album folder"),
            tr("One picture for the whole folder. Other players look for it there."),
            editor.cover_in_folder.get(),
            |e, on| e.cover_in_folder.set(on),
        ),
    ];
    for (title, line, active, set) in choices {
        let check = gtk::CheckButton::new();
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.append(&label(&title, &[]));
        text.append(&label(&line, &["hk-t3", "hk-secondary"]));
        check.set_child(Some(&text));
        check.set_active(active);
        // They decide where a new picture goes.
        check.set_sensitive(sensitive && matches!(*editor.cover.borrow(), Some(CoverEdit::Set(_))));
        check.update_property(&[gtk::accessible::Property::Label(&title)]);
        let weak = Rc::downgrade(editor);
        check.connect_toggled(move |check| {
            if let Some(editor) = weak.upgrade() {
                set(&editor, check.is_active());
                editor.refresh();
            }
        });
        column.append(&check);
    }
    column
}

/// Saves the front cover of the first file as a picture file the user names.
fn save_cover_as(editor: &Rc<Editor>) {
    let Some(track) = editor.tracks.first().map(|t| t.key) else {
        return;
    };
    let services = editor.backend.services();
    let weak = Rc::downgrade(editor);
    glib::spawn_future_local(async move {
        let Ok(Some(bytes)) = gio::spawn_blocking(move || services.cover_picture(track)).await
        else {
            return;
        };
        let Some(editor) = weak.upgrade() else { return };
        let name = if bytes.starts_with(b"\x89PNG") {
            "cover.png"
        } else {
            "cover.jpg"
        };
        let dialog = gtk::FileDialog::builder()
            .title(tr("Save the cover as a file"))
            .initial_name(name)
            .modal(true)
            .build();
        let window = editor.window.clone();
        let main = editor.main.clone();
        dialog.save(Some(&window), gio::Cancellable::NONE, move |result| {
            let Ok(Some(path)) = result.map(|f| f.path()) else {
                return;
            };
            if let Err(e) = std::fs::write(&path, &bytes)
                && let Some(main) = main.upgrade()
            {
                main.toast(
                    &fill(&tr("The cover could not be saved: {}"), &[&e.to_string()]),
                    true,
                );
            }
        });
    });
}

fn lyrics_tab(editor: &Rc<Editor>) -> gtk::Widget {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.add_css_class("hk-tag-form");
    if editor.tracks.len() != 1 {
        body.append(&note(&tr(
            "Lyrics belong to one file. Open the tag editor for one track to change them.",
        )));
        return body.upcast();
    }
    let view = gtk::TextView::new();
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.add_css_class("hk-text");
    view.set_editable(editor.read_only().is_none());
    view.buffer().set_text(&editor.lyrics_start);
    view.update_property(&[gtk::accessible::Property::Label(&tr("Lyrics"))]);
    let weak = Rc::downgrade(editor);
    view.buffer().connect_changed(move |_| {
        if let Some(editor) = weak.upgrade() {
            editor.refresh();
        }
    });
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_vexpand(true);
    scroller.set_child(Some(&view));
    scroller.add_css_class("hk-text-frame");
    body.append(&scroller);
    let under = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let hint = note(&tr(
        "Lines that start with a time, such as [01:23.45], are shown in step with the music.",
    ));
    hint.set_hexpand(true);
    under.append(&hint);
    let clear = compact_button(&tr("Clear"));
    clear.set_sensitive(editor.read_only().is_none());
    let weak = view.downgrade();
    clear.connect_clicked(move |_| {
        if let Some(view) = weak.upgrade() {
            view.buffer().set_text("");
        }
    });
    under.append(&clear);
    body.append(&under);
    editor.lyrics.replace(Some(view));
    body.upcast()
}

fn info_pair(grid: &gtk::Grid, row: &mut i32, name: &str, value: &str, mono: bool) {
    if value.is_empty() {
        return;
    }
    let key = label(name, &["hk-t3"]);
    key.set_size_request(96, -1);
    key.set_valign(gtk::Align::Start);
    grid.attach(&key, 0, *row, 1, 1);
    let text = gtk::Label::new(Some(value));
    text.set_xalign(0.0);
    text.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    text.set_tooltip_text(Some(value));
    text.set_hexpand(true);
    text.add_css_class(if mono { "hk-mono-small" } else { "hk-medium" });
    grid.attach(&text, 1, *row, 1, 1);
    *row += 1;
}

fn info_group(title: &str, pairs: &[(String, String, bool)]) -> gtk::Box {
    let group = gtk::Box::new(gtk::Orientation::Vertical, 8);
    group.append(&label(title, &["hk-section-label"]));
    let grid = gtk::Grid::new();
    grid.set_row_spacing(7);
    grid.set_column_spacing(12);
    let mut row = 0;
    for (name, value, mono) in pairs {
        info_pair(&grid, &mut row, name, value, *mono);
    }
    group.append(&grid);
    group
}

fn gain(value: Option<f64>) -> String {
    value.map_or_else(String::new, |v| {
        let text = if v < 0.0 {
            format!("−{}", crate::format::decimal(-v, 1))
        } else {
            crate::format::decimal(v, 1)
        };
        fill(&tr("{} dB"), &[&text])
    })
}

fn fill_info(editor: &Rc<Editor>, index: usize) {
    let info = &editor.info;
    while let Some(child) = info.first_child() {
        info.remove(&child);
    }
    let Some(track) = editor.tracks.get(index) else {
        return;
    };
    let facts = &track.facts;
    let format = &facts.format;
    let left = gtk::Box::new(gtk::Orientation::Vertical, 22);
    left.set_hexpand(true);
    let folder = track.path.parent().map(tilde).unwrap_or_default();
    // The tag formats and pictures are the last row of the File group, which
    // leaves room for the Analysis band under the columns.
    let mut tags = facts.tag_formats.join(", ");
    if facts.pictures > 0 {
        let pictures = fill(
            &ntr("{} picture", "{} pictures", facts.pictures),
            &[&facts.pictures.to_string()],
        );
        tags = if tags.is_empty() {
            pictures
        } else {
            format!("{tags} · {pictures}")
        };
    }
    let file_group = info_group(
        &tr("File"),
        &[
            (tr("Name"), track.file_name(), true),
            (tr("Folder"), folder, true),
            (tr("Size"), facts.size.map(bytes).unwrap_or_default(), false),
            (
                tr("Changed"),
                facts.modified.clone().unwrap_or_default(),
                false,
            ),
            (tr("Tags"), tags, false),
        ],
    );
    let open = compact_button(&tr("Open folder"));
    open.set_halign(gtk::Align::Start);
    let path: PathBuf = track.path.clone();
    let weak = editor.main.clone();
    open.connect_clicked(move |_| {
        let (sender, receiver) = async_channel::bounded(1);
        let started =
            helikodu_desktop::open_folder::show_in_folder_later(path.clone(), None, move |r| {
                let _ = sender.send_blocking(r.err().map(|e| e.to_string()));
            });
        let weak = weak.clone();
        glib::spawn_future_local(async move {
            let problem = match started {
                Ok(()) => receiver.recv().await.ok().flatten(),
                Err(e) => Some(e.to_string()),
            };
            if let (Some(problem), Some(main)) = (problem, weak.upgrade()) {
                main.toast(&problem, true);
            }
        });
    });
    file_group.append(&open);
    left.append(&file_group);
    let channels = match format.channels {
        Some(1) => tr("Mono"),
        Some(2) => tr("Stereo"),
        Some(n) => fill(&tr("{} channels"), &[&n.to_string()]),
        None => String::new(),
    };
    left.append(&info_group(
        &tr("Audio"),
        &[
            (
                tr("Format"),
                format.codec.clone().unwrap_or_default(),
                false,
            ),
            (
                tr("Sample rate"),
                format
                    .sample_rate
                    .map(|r| fill(&tr("{} kHz"), &[&crate::format::khz(r)]))
                    .unwrap_or_default(),
                false,
            ),
            (
                tr("Bit depth"),
                format
                    .bits
                    .map(|b| fill(&tr("{}-bit"), &[&b.to_string()]))
                    .unwrap_or_default(),
                false,
            ),
            (tr("Channels"), channels, false),
            (
                tr("Bitrate"),
                format
                    .bitrate_kbps
                    .map(|b| fill(&tr("{} kbps"), &[&b.to_string()]))
                    .unwrap_or_default(),
                false,
            ),
            (
                tr("Length"),
                facts
                    .duration
                    .map(crate::format::duration)
                    .unwrap_or_default(),
                false,
            ),
            (
                tr("Encoder"),
                facts.encoder.clone().unwrap_or_default(),
                true,
            ),
        ],
    ));
    let columns = gtk::Box::new(gtk::Orientation::Horizontal, 32);
    columns.append(&left);
    let right = gtk::Box::new(gtk::Orientation::Vertical, 22);
    right.set_hexpand(true);
    let peak = |v: Option<f64>| v.map(|p| crate::format::decimal(p, 2)).unwrap_or_default();
    right.append(&info_group(
        &tr("Loudness"),
        &[
            (tr("Track gain"), gain(facts.track_gain_db), false),
            (tr("Track peak"), peak(facts.track_peak), false),
            (tr("Album gain"), gain(facts.album_gain_db), false),
            (tr("Album peak"), peak(facts.album_peak), false),
        ],
    ));
    right.append(&info_group(
        &tr("In the library"),
        &[
            (tr("Added"), facts.added.clone().unwrap_or_default(), false),
            (
                tr("Last played"),
                facts.last_played.clone().unwrap_or_else(|| {
                    if facts.plays.is_some() {
                        tr("Never")
                    } else {
                        String::new()
                    }
                }),
                false,
            ),
            (
                tr("Plays"),
                facts.plays.map(|p| p.to_string()).unwrap_or_default(),
                false,
            ),
            (
                tr("Skips"),
                facts.skips.map(|p| p.to_string()).unwrap_or_default(),
                false,
            ),
        ],
    ));
    // Kept in the library and applied at once, not part of "Save".
    if let Some(group) = super::track_flags::playback_group(&editor.backend, track.key) {
        right.append(&group);
    }
    columns.append(&right);
    info.append(&columns);
    // What the track analysis found, across both columns.
    info.append(&super::analysis_band::band(&facts.analysis));
}

fn file_list(editor: &Rc<Editor>) -> gtk::Box {
    let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side.add_css_class("hk-tag-files");
    side.set_size_request(264, -1);
    side.append(&label(
        &tr("Editing"),
        &["hk-section-label", "hk-list-section", "first"],
    ));
    let list = gtk::ListBox::new();
    list.add_css_class("hk-list");
    list.add_css_class("hk-file-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.update_property(&[gtk::accessible::Property::Label(&tr("Files"))]);
    let mut rows = Vec::new();
    for track in &editor.tracks {
        let row = gtk::ListBoxRow::new();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let number = track
            .values(&TagField::Track)
            .first()
            .cloned()
            .unwrap_or_default();
        let lead = label(&number, &["hk-t3", "hk-numeric"]);
        lead.set_size_request(18, -1);
        // The number sits at the middle of the two lines, and the two lines
        // at the middle of the row.
        lead.set_valign(gtk::Align::Center);
        lead.add_css_class("hk-file-number");
        line.append(&lead);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let title = track
            .values(&TagField::Title)
            .first()
            .cloned()
            .unwrap_or_else(|| track.file_name());
        text.append(&label(&title, &["hk-medium"]));
        text.append(&label(&track.file_name(), &["hk-mono-meta", "hk-t3"]));
        line.append(&text);
        let problem = gtk::Image::from_icon_name("dialog-warning-symbolic");
        problem.set_visible(false);
        line.append(&problem);
        row.set_child(Some(&line));
        let file = track.file_name();
        row.update_property(&[gtk::accessible::Property::Label(&if number.is_empty() {
            fill(&tr("{}, {}"), &[&title, &file])
        } else {
            fill(&tr("{}. {}, {}"), &[&number, &title, &file])
        })]);
        list.append(&row);
        rows.push(row);
    }
    editor.rows.replace(rows);
    let weak = Rc::downgrade(editor);
    list.connect_row_selected(move |_, row| {
        if let (Some(editor), Some(row)) = (weak.upgrade(), row) {
            fill_info(&editor, usize::try_from(row.index()).unwrap_or(0));
        }
    });
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&list));
    side.append(&scroller);
    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }

    if let Some(first) = editor.tracks.first() {
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        foot.add_css_class("hk-tag-files-foot");
        let art = CoverArt::new(44, 4.0);
        art.add_css_class("small");
        art.show(first.cover_seed, cover_texture(&editor.backend, first));
        foot.append(&art);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_valign(gtk::Align::Center);
        let album = match shared(&editor.tracks, &TagField::Album) {
            Shared::Same(values) => values.join(SEPARATOR),
            Shared::Mixed(_) => tr("Several albums"),
        };
        text.append(&label(&album, &["hk-medium"]));
        let mut kind: Vec<String> = Vec::new();
        if let Some(codec) = &first.facts.format.codec {
            kind.push(codec.clone());
        }
        if let Some(tags) = first.facts.tag_formats.first() {
            kind.push(tags.clone());
        }
        text.append(&label(&kind.join(" · "), &["hk-mono-meta", "hk-t3"]));
        foot.append(&text);
        side.append(&foot);
    }
    side
}

fn build(main: &MainWindow, backend: &Backend, tracks: Vec<EditTrack>) -> gtk::Window {
    let parent = main.upcast_ref::<gtk::Window>();
    let window = dialog_window(Some(parent), NAME, &tr("Edit tags"), (960, 720), false);
    let n = tracks.len();
    let save = gtk::Button::with_label(&fill(
        &ntr("Save {} file", "Save {} files", n),
        &[&n.to_string()],
    ));
    save.add_css_class("hk-btn");
    save.add_css_class("primary");
    save.set_sensitive(false);
    let problems_text = label("", &[]);
    problems_text.set_wrap(true);
    problems_text.set_hexpand(true);
    let problems = gtk::Revealer::new();
    crate::style::follow_reduced_motion(&problems, 250);
    let problem_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    problem_box.add_css_class("hk-notice");
    problem_box.append(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
    problem_box.append(&problems_text);
    problems.set_child(Some(&problem_box));
    let lyrics_start = if n == 1 {
        tracks[0].lyrics.clone().unwrap_or_default()
    } else {
        String::new()
    };
    let info = gtk::Box::new(gtk::Orientation::Vertical, 18);
    info.add_css_class("hk-tag-form");
    let cover_tab = gtk::Box::new(gtk::Orientation::Horizontal, 28);
    cover_tab.add_css_class("hk-tag-form");
    let editor = Rc::new(Editor {
        window: window.clone(),
        main: main.downgrade(),
        backend: backend.clone(),
        tracks,
        inputs: RefCell::new(Vec::new()),
        others: RefCell::new(Vec::new()),
        rating: Cell::new(None),
        cover: RefCell::new(None),
        lyrics: RefCell::new(None),
        lyrics_start,
        rows: RefCell::new(Vec::new()),
        save: save.clone(),
        problems,
        problems_text,
        info,
        cover_tab,
        tab_buttons: RefCell::new(Vec::new()),
        pending: RefCell::new(BTreeMap::new()),
        cover_inside: Cell::new(true),
        cover_in_folder: Cell::new(false),
    });

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    strip.add_css_class("hk-strip");
    strip.append(&label(&tr("Edit tags"), &["hk-title"]));
    let badge = label(
        &fill(&ntr("{} track", "{} tracks", n), &[&n.to_string()]),
        &["hk-badge", "hk-badge-plain"],
    );
    // Short, and cut off it says nothing.
    badge.set_ellipsize(gtk::pango::EllipsizeMode::None);
    badge.set_valign(gtk::Align::Center);
    badge.set_hexpand(true);
    badge.set_halign(gtk::Align::Start);
    strip.append(&badge);
    let close = icon_button("window-close-symbolic", &tr("Close"));
    let weak = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    strip.append(&close);
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&strip));
    root.append(&handle);

    let middle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    middle.set_vexpand(true);
    middle.append(&file_list(&editor));
    let main_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main_column.set_hexpand(true);
    main_column.add_css_class("hk-tag-main");
    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 22);
    tabs.add_css_class("hk-tabs");
    tabs.add_css_class("hk-tag-tabs");
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&tags_tab(&editor), Some("tags"));
    fill_cover_tab(&editor);
    stack.add_named(&editor.cover_tab, Some("cover"));
    stack.add_named(&lyrics_tab(&editor), Some("lyrics"));
    let info_scroller = gtk::ScrolledWindow::new();
    info_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    info_scroller.set_child(Some(&editor.info));
    stack.add_named(&info_scroller, Some("info"));
    let mut buttons: Vec<gtk::ToggleButton> = Vec::new();
    for (name, title) in [
        ("tags", tr("Tags")),
        ("cover", tr("Cover")),
        ("lyrics", tr("Lyrics")),
        ("info", tr("File info")),
    ] {
        let button = gtk::ToggleButton::with_label(&title);
        if let Some(first) = buttons.first() {
            button.set_group(Some(first));
        }
        let weak = stack.downgrade();
        button.connect_toggled(move |button| {
            if button.is_active()
                && let Some(stack) = weak.upgrade()
            {
                stack.set_visible_child_name(name);
            }
        });
        tabs.append(&button);
        buttons.push(button);
    }
    buttons[0].set_active(true);
    editor.tab_buttons.replace(buttons);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    tabs.append(&spacer);
    tabs.append(&tools_button(&editor));
    main_column.append(&tabs);
    main_column.append(&editor.problems);
    main_column.append(&stack);
    middle.append(&main_column);
    root.append(&middle);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("hk-dialog-footer");
    let how = label(
        &tr("Written to a copy, verified, then swapped in. Undo with Ctrl Z."),
        &["hk-t3", "hk-secondary"],
    );
    how.set_hexpand(true);
    footer.append(&how);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("hk-btn");
    cancel.set_valign(gtk::Align::Center);
    save.set_valign(gtk::Align::Center);
    let weak = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    footer.append(&cancel);
    let weak = Rc::downgrade(&editor);
    save.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            editor.save();
        }
    });
    footer.append(&save);
    root.append(&footer);
    window.set_child(Some(&root));
    window.set_default_widget(Some(&save));
    let keep = RefCell::new(Some(editor));
    window.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    window.present();
    window
}

/// The compact "Tag tools" button at the end of the tab strip: a menu of the
/// tools, each opening the Tag tools window there.
fn tools_button(editor: &Rc<Editor>) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_label(&tr("Tag tools"));
    button.add_css_class("hk-sort");
    button.set_valign(gtk::Align::Center);
    button.set_widget_name("tag-tools");
    button.set_sensitive(editor.read_only().is_none());
    let group = gio::SimpleActionGroup::new();
    let action = gio::SimpleAction::new("tool", Some(glib::VariantTy::STRING));
    let weak = Rc::downgrade(editor);
    action.connect_activate(move |_, parameter| {
        let (Some(editor), Some(tool)) = (
            weak.upgrade(),
            parameter
                .and_then(glib::Variant::get::<String>)
                .and_then(|id| Tool::from_id(&id)),
        ) else {
            return;
        };
        editor.open_tools(tool);
    });
    group.add_action(&action);
    editor.window.insert_action_group("tageditor", Some(&group));
    let menu = gio::Menu::new();
    for tool in Tool::ALL {
        let item = gio::MenuItem::new(Some(&tool.title()), None);
        item.set_action_and_target_value(Some("tageditor.tool"), Some(&tool.id().to_variant()));
        menu.append_item(&item);
    }
    crate::widgets::menu_names::set_menu_later(&button, &menu);
    button
}

/// Opens the tag editor over `main` for `tracks`, once their tags are read.
pub(crate) fn open(main: &MainWindow, tracks: &[TrackKey]) {
    let Some(backend) = main.backend() else {
        return;
    };
    if tracks.is_empty() {
        return;
    }
    let services = backend.services();
    let keys = tracks.to_vec();
    let weak = main.downgrade();
    glib::spawn_future_local(async move {
        let loaded = gio::spawn_blocking(move || services.edit_tracks(&keys))
            .await
            .unwrap_or_else(|_| Ok(Vec::new()));
        let Some(main) = weak.upgrade() else { return };
        match loaded {
            Ok(tracks) if !tracks.is_empty() => {
                build(&main, &backend, tracks);
            }
            Ok(_) => main.toast(&tr("These tracks are no longer in the library."), true),
            Err(error) => main.toast(&fill(&tr("The tags cannot be read: {}"), &[&error.0]), true),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(key: u64, title: &str, album: &str) -> EditTrack {
        let mut fields = BTreeMap::new();
        fields.insert(TagField::Title, vec![title.to_owned()]);
        fields.insert(TagField::Album, vec![album.to_owned()]);
        EditTrack {
            key: TrackKey(key),
            path: PathBuf::from(format!("/music/{key}.flac")),
            fields,
            other: Vec::new(),
            raw: BTreeMap::new(),
            rating: None,
            cover: None,
            cover_seed: 0,
            lyrics: None,
            read_only: None,
            facts: Default::default(),
        }
    }

    #[test]
    fn mixed_and_shared_values() {
        let tracks = [track(1, "A", "X"), track(2, "B", "X"), track(3, "C", "X")];
        assert_eq!(shared(&tracks, &TagField::Title), Shared::Mixed(3));
        assert_eq!(
            shared(&tracks, &TagField::Album),
            Shared::Same(vec!["X".into()])
        );
        assert_eq!(shared(&tracks, &TagField::Genre), Shared::Same(Vec::new()));
    }

    #[test]
    fn people_split_on_semicolons() {
        assert_eq!(
            parse(&TagField::Artist, "Ada Lind; Brann Orme"),
            vec!["Ada Lind".to_owned(), "Brann Orme".to_owned()]
        );
        assert_eq!(parse(&TagField::Title, "  "), Vec::<String>::new());
    }

    #[test]
    fn dates_show_without_time_and_zone() {
        assert_eq!(shown_date("2004-07-02T12:00:00Z"), "2004-07-02");
        assert_eq!(shown_date("2004-07-02 12:00"), "2004-07-02");
        assert_eq!(shown_date("2004T10"), "2004");
        assert_eq!(shown_date("2004"), "2004");
        assert_eq!(shown_date("2004-07"), "2004-07");
        assert_eq!(shown_date("  1999-04-01 "), "1999-04-01");
        assert_eq!(shown_date("summer 69"), "summer 69");
        assert_eq!(shown_date("2004-13-02T12:00:00Z"), "2004-13-02T12:00:00Z");
        assert!(valid_date("2004") && valid_date("2004-07") && valid_date("2004-02-29"));
        assert!(!valid_date("2003-02-29") && !valid_date("04") && !valid_date("2004-7-2"));
        assert!(!valid_date("2004-07-02T12:00:00Z") && !valid_date("0000") && !valid_date(""));
        // A stored date left as shown is no change; another date is.
        let stored = vec!["2004-07-02T12:00:00Z".to_owned()];
        let year = |text: &str| unchanged(&TagField::Year, &stored, &parse(&TagField::Year, text));
        assert!(year("2004-07-02"));
        assert!(!year("2004"));
        assert_eq!(parse(&TagField::Title, "A; B"), vec!["A; B".to_owned()]);
    }
}
