// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Your fields: the fields the user defines, in a tool window opened from the
//! Library menu. The left pane lists them, with "New field"; the editor holds
//! the name, the kind (computed from other fields, or a tag of the files), the
//! expression with the Insert popover or the tag name, and a table of example
//! values for the tracks selected in the main window, or for the first tracks
//! of a typed search, that follows the expression as it is typed. A name or an
//! expression that cannot be used is underlined and explained under its entry,
//! and nothing is saved while there is a problem. A saved change is in use at
//! once: searches, naming patterns and the Insert popover know it, and the
//! lists fetch their tracks again.

mod draft;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

use self::draft::{Check, Draft, Kind, Notice};
use super::tool_window::{Frame, footer, frame, primary_button};
use crate::backend::fields::{FieldServices, UserField};
use crate::backend::{Backend, TrackKey, TrackQuery, TrackRow};
use crate::dialogs::common::{compact_button, find_open, segmented};
use crate::i18n::{fill, ntr, tr};
use crate::widgets::insert_popover::{InsertTarget, insert_button};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "user-fields";

/// How long the example values wait for typing to pause.
const TYPING: Duration = Duration::from_millis(200);

/// How many tracks of a search, or of the library, give example values.
const FIRST_TRACKS: usize = 20;

/// The most selected tracks that give example values.
const SELECTED_TRACKS: usize = 200;

/// The notice under an entry: a warning icon, what is wrong, and a line under
/// it (the rule for names, or an example).
struct NoticeBox {
    root: gtk::Box,
    message: gtk::Label,
    detail: gtk::Label,
}

impl NoticeBox {
    fn new(name: &str) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.add_css_class("hk-notice");
        root.add_css_class("hk-fields-notice");
        root.set_widget_name(name);
        root.set_accessible_role(gtk::AccessibleRole::Alert);
        let icon = gtk::Image::from_icon_name("helikodu-warning-symbolic");
        icon.set_valign(gtk::Align::Start);
        root.append(&icon);
        let lines = gtk::Box::new(gtk::Orientation::Vertical, 1);
        lines.set_hexpand(true);
        let message = crate::widgets::wrapped_label("", &["hk-medium"]);
        lines.append(&message);
        let detail = crate::widgets::wrapped_label("", &["hk-t3", "hk-secondary"]);
        lines.append(&detail);
        root.append(&lines);
        root.set_visible(false);
        Self {
            root,
            message,
            detail,
        }
    }

    fn show(&self, notice: Option<&Notice>) {
        let Some(notice) = notice else {
            self.root.set_visible(false);
            return;
        };
        self.message.set_text(&notice.message);
        match (&notice.detail, &notice.example) {
            (Some(detail), _) => {
                self.detail.set_text(detail);
                self.detail.set_visible(true);
            }
            (None, Some(example)) => {
                // The example itself in mono.
                let mono = format!(
                    "<span font_family=\"monospace\">{}</span>",
                    glib::markup_escape_text(example)
                );
                let words = glib::markup_escape_text(&tr("For example {}"));
                self.detail.set_markup(&fill(&words, &[&mono]));
                self.detail.add_css_class("hk-fields-example");
                self.detail.set_visible(true);
            }
            (None, None) => self.detail.set_visible(false),
        }
        if notice.detail.is_some() {
            self.detail.remove_css_class("hk-fields-example");
        }
        self.root.set_visible(true);
    }
}

/// A row of the editor: the 92 px label column and its content.
fn editor_row(title: &str, content: &impl IsA<gtk::Widget>, top: bool) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let name = label(title, &["hk-medium", "hk-fields-label"]);
    name.set_size_request(92, -1);
    name.set_ellipsize(gtk::pango::EllipsizeMode::None);
    name.set_wrap(true);
    name.set_single_line_mode(false);
    if top {
        name.set_valign(gtk::Align::Start);
        name.add_css_class("top");
    } else {
        name.set_valign(gtk::Align::Center);
    }
    row.append(&name);
    content.as_ref().set_hexpand(true);
    row.append(content);
    (row, name)
}

/// Your fields. See the module documentation.
struct Fields {
    window: gtk::Window,
    main: glib::WeakRef<MainWindow>,
    backend: Backend,
    services: Arc<dyn FieldServices>,
    /// The saved fields, in their order.
    stored: RefCell<Vec<UserField>>,
    /// The field being edited: its index in `stored`, or `None` for a new one.
    chosen: Cell<Option<usize>>,
    draft: RefCell<Draft>,
    check: RefCell<Check>,
    /// Set while the widgets are filled from a field.
    filling: Cell<bool>,
    /// Set while a question is asked.
    asking: Cell<bool>,
    /// Set once closing was agreed to.
    closing: Cell<bool>,
    list: gtk::ListBox,
    new_row: gtk::ListBoxRow,
    title: gtk::Label,
    summary: gtk::Label,
    name: gtk::Entry,
    name_notice: NoticeBox,
    kinds: Vec<gtk::ToggleButton>,
    expression_row: gtk::Box,
    expression: gtk::TextView,
    expression_notice: NoticeBox,
    use_help: gtk::Label,
    tag_row: gtk::Box,
    tag: gtk::Entry,
    tag_help: gtk::Label,
    value_head: gtk::Label,
    search: gtk::SearchEntry,
    values: gtk::Box,
    values_stack: gtk::Stack,
    footer_line: gtk::Label,
    remove: gtk::Button,
    save: gtk::Button,
    /// The tracks whose values are shown.
    tracks: RefCell<Vec<TrackRow>>,
    /// The values shown, by track.
    shown: RefCell<Vec<(TrackKey, String)>>,
    typing: Cell<u32>,
    searching: Cell<u32>,
    generation: Cell<u32>,
    error_tag: gtk::TextTag,
}

/// Opens Your fields over `main`, or brings it to the front.
pub(crate) fn open(main: &MainWindow) -> Option<gtk::Window> {
    let parent = main.upcast_ref::<gtk::Window>();
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        return Some(window);
    }
    let backend = main.backend()?;
    let services = backend.services().user_fields();
    let Frame {
        window,
        list,
        side_note,
        title,
        summary,
        main: column,
    } = frame(parent, NAME, &tr("Your fields"), &tr("Fields"));
    side_note.set_text(&tr("Fields are kept in the library, not in the files."));
    let new_row = new_field_row();

    // The editor.
    let editor = gtk::Box::new(gtk::Orientation::Vertical, 12);
    editor.add_css_class("hk-fields-editor");
    let name_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let name = gtk::Entry::new();
    name.add_css_class("hk-input");
    name.add_css_class("hk-mono");
    name.set_widget_name("fields-name");
    name.set_size_request(260, -1);
    name.set_halign(gtk::Align::Start);
    name_column.append(&name);
    let name_notice = NoticeBox::new("fields-name-notice");
    name_column.append(&name_notice.root);
    let (name_row, name_label) = editor_row(&tr("Name"), &name_column, true);
    name.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        name_label.upcast_ref()
    ])]);
    editor.append(&name_row);

    let (kind_group, kinds) =
        segmented(&[tr("Computed from other fields"), tr("A tag of the files")]);
    kinds[0].set_widget_name("fields-kind-computed");
    kinds[1].set_widget_name("fields-kind-tag");
    kinds[0].set_active(true);
    kind_group.update_property(&[gtk::accessible::Property::Label(&tr("Kind of field"))]);
    let (kind_row, _) = editor_row(&tr("Kind"), &kind_group, false);
    kind_group.set_hexpand(false);
    editor.append(&kind_row);

    let expression_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let expression_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let expression = gtk::TextView::new();
    expression.add_css_class("hk-mono");
    expression.add_css_class("hk-fields-expression");
    expression.set_widget_name("fields-expression");
    expression.set_wrap_mode(gtk::WrapMode::WordChar);
    expression.set_accepts_tab(false);
    expression.set_top_margin(7);
    expression.set_bottom_margin(7);
    expression.set_left_margin(10);
    expression.set_right_margin(10);
    let expression_scroller = gtk::ScrolledWindow::new();
    expression_scroller.add_css_class("hk-fields-expression-box");
    expression_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    expression_scroller.set_min_content_height(60);
    expression_scroller.set_max_content_height(120);
    expression_scroller.set_propagate_natural_height(true);
    expression_scroller.set_hexpand(true);
    expression_scroller.set_child(Some(&expression));
    expression_line.append(&expression_scroller);
    let insert = insert_button(InsertTarget::TextView(expression.clone()));
    insert.set_widget_name("fields-insert");
    expression_line.append(&insert);
    expression_column.append(&expression_line);
    let expression_notice = NoticeBox::new("fields-expression-notice");
    expression_column.append(&expression_notice.root);
    let use_help = crate::widgets::wrapped_label("", &["hk-t3", "hk-secondary", "hk-fields-help"]);
    expression_column.append(&use_help);
    let (expression_row, expression_label) =
        editor_row(&tr("Expression"), &expression_column, true);
    expression.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        expression_label.upcast_ref()
    ])]);
    editor.append(&expression_row);

    let tag_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let tag = gtk::Entry::new();
    tag.add_css_class("hk-input");
    tag.add_css_class("hk-mono");
    tag.set_widget_name("fields-tag");
    tag.set_size_request(260, -1);
    tag.set_halign(gtk::Align::Start);
    tag_column.append(&tag);
    let tag_help = crate::widgets::wrapped_label("", &["hk-t3", "hk-secondary", "hk-fields-help"]);
    tag_column.append(&tag_help);
    let (tag_row, tag_label) = editor_row(&tr("Tag name"), &tag_column, true);
    tag.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        tag_label.upcast_ref()
    ])]);
    tag_row.set_visible(false);
    editor.append(&tag_row);
    column.append(&editor);

    // The example values.
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    head.add_css_class("hk-table-header");
    head.add_css_class("hk-tool-head");
    head.add_css_class("hk-fields-head");
    let examples = label(&tr("Example values"), &["hk-section-label"]);
    examples.set_max_width_chars(1);
    let value_head = label("", &["hk-section-label", "hk-fields-value-head"]);
    value_head.set_max_width_chars(1);
    head.append(&columns(&examples, &value_head));
    let search = gtk::SearchEntry::new();
    search.add_css_class("hk-input");
    search.add_css_class("hk-fields-search");
    search.set_widget_name("fields-search");
    search.set_placeholder_text(Some(&tr("Or the tracks of a search")));
    search.update_property(&[gtk::accessible::Property::Label(&tr(
        "Show example values for the tracks of a search",
    ))]);
    search.set_size_request(250, -1);
    search.set_valign(gtk::Align::Center);
    head.append(&search);
    column.append(&head);
    let values = gtk::Box::new(gtk::Orientation::Vertical, 0);
    values.set_widget_name("fields-values");
    values.set_valign(gtk::Align::Start);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_child(Some(&values));
    let values_empty = crate::widgets::wrapped_label(
        &tr("The example values come back when the name and the expression can be read."),
        &["hk-t3", "hk-fields-empty"],
    );
    values_empty.set_widget_name("fields-values-empty");
    values_empty.set_valign(gtk::Align::Start);
    let values_stack = gtk::Stack::new();
    values_stack.set_vexpand(true);
    values_stack.add_named(&scroller, Some("values"));
    values_stack.add_named(&values_empty, Some("empty"));
    column.append(&values_stack);

    // The footer.
    let (bar, footer_line) = footer();
    footer_line.set_widget_name("fields-footer");
    let remove = compact_button(&tr("Remove"));
    remove.set_widget_name("fields-remove");
    bar.append(&remove);
    let save = primary_button(&tr("Save"));
    save.set_widget_name("fields-save");
    bar.append(&save);
    column.append(&bar);

    let error_tag = gtk::TextTag::new(Some("error"));
    error_tag.set_underline(gtk::pango::Underline::Error);
    expression.buffer().tag_table().add(&error_tag);

    let fields = Rc::new(Fields {
        window: window.clone(),
        main: main.downgrade(),
        backend,
        services,
        stored: RefCell::new(Vec::new()),
        chosen: Cell::new(None),
        draft: RefCell::new(Draft::default()),
        check: RefCell::new(Check::default()),
        filling: Cell::new(false),
        asking: Cell::new(false),
        closing: Cell::new(false),
        list,
        new_row,
        title,
        summary,
        name,
        name_notice,
        kinds,
        expression_row,
        expression,
        expression_notice,
        use_help,
        tag_row,
        tag,
        tag_help,
        value_head,
        search,
        values,
        values_stack,
        footer_line,
        remove,
        save,
        tracks: RefCell::new(Vec::new()),
        shown: RefCell::new(Vec::new()),
        typing: Cell::new(0),
        searching: Cell::new(0),
        generation: Cell::new(0),
        error_tag,
    });
    fields.connect();
    fields.reload(None);
    fields.load_tracks();
    // The window keeps its state for as long as it is open.
    let keep_alive = RefCell::new(Some(fields));
    window.connect_destroy(move |_| {
        keep_alive.borrow_mut().take();
    });
    window.present();
    Some(window)
}

/// The "New field" row: a plus and the words.
fn new_field_row() -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_widget_name("new-field");
    row.add_css_class("hk-dim");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.append(&gtk::Image::from_icon_name("list-add-symbolic"));
    content.append(&label(&tr("New field"), &[]));
    row.set_child(Some(&content));
    row.update_property(&[gtk::accessible::Property::Label(&tr("New field"))]);
    row
}

/// A row of the left pane: the name in mono over what the field stands for.
fn field_row(field: &UserField) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("two-line");
    row.set_widget_name(&format!("field-{}", field.name));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_valign(gtk::Align::Center);
    text.append(&label(
        &field.name,
        &["hk-side-name", "hk-mono", "hk-fields-side-name"],
    ));
    let line = match &field.definition {
        crate::backend::fields::FieldDefinition::Computed { expression } => label(
            expression,
            &["hk-t3", "hk-side-line", "hk-mono", "hk-side-mono"],
        ),
        crate::backend::fields::FieldDefinition::Tag { key } => label(
            // Translators: {} is the name of a tag, such as MOOD.
            &fill(&tr("Tag {}"), &[key]),
            &["hk-t3", "hk-side-line"],
        ),
    };
    text.append(&line);
    row.set_child(Some(&text));
    row.update_property(&[gtk::accessible::Property::Label(&field.name)]);
    row
}

/// What to do with unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Discard,
    Cancel,
    Save,
}

impl Fields {
    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_selected(move |_, row| {
            let (Some(this), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            if this.filling.get() {
                return;
            }
            let wanted = this.row_choice(row);
            if wanted == this.chosen.get() {
                return;
            }
            if this.is_dirty() {
                // Stay on the field being edited until the question is answered.
                this.select_row_of(this.chosen.get());
                let weak = Rc::downgrade(&this);
                this.ask(move |answer| {
                    let Some(this) = weak.upgrade() else { return };
                    match answer {
                        Answer::Discard => this.choose(wanted),
                        Answer::Save => this.save_then(move |this| this.choose(wanted)),
                        Answer::Cancel => {}
                    }
                });
                return;
            }
            this.choose(wanted);
        });
        let weak = Rc::downgrade(self);
        self.name.connect_changed(move |entry| {
            if let Some(this) = weak.upgrade()
                && !this.filling.get()
            {
                this.draft.borrow_mut().name = entry.text().to_string();
                this.changed();
            }
        });
        let weak = Rc::downgrade(self);
        self.expression.buffer().connect_changed(move |buffer| {
            if let Some(this) = weak.upgrade()
                && !this.filling.get()
            {
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                this.draft.borrow_mut().expression = text.to_string();
                this.changed();
            }
        });
        let weak = Rc::downgrade(self);
        self.tag.connect_changed(move |entry| {
            if let Some(this) = weak.upgrade()
                && !this.filling.get()
            {
                this.draft.borrow_mut().tag = entry.text().to_string();
                this.changed();
            }
        });
        for (index, button) in self.kinds.iter().enumerate() {
            let weak = Rc::downgrade(self);
            button.connect_toggled(move |button| {
                if !button.is_active() {
                    return;
                }
                let Some(this) = weak.upgrade() else { return };
                let kind = if index == 0 {
                    Kind::Computed
                } else {
                    Kind::Tag
                };
                this.expression_row.set_visible(kind == Kind::Computed);
                this.tag_row.set_visible(kind == Kind::Tag);
                if !this.filling.get() {
                    this.draft.borrow_mut().kind = kind;
                    this.changed();
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let tick = this.searching.get().wrapping_add(1);
            this.searching.set(tick);
            let weak = Rc::downgrade(&this);
            glib::timeout_add_local_once(TYPING, move || {
                if let Some(this) = weak.upgrade()
                    && this.searching.get() == tick
                {
                    this.load_tracks();
                }
            });
        });
        let weak = Rc::downgrade(self);
        self.save.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.save_then(|_| {});
            }
        });
        let weak = Rc::downgrade(self);
        self.remove.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.confirm_remove();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |window| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if this.closing.get() || !this.is_dirty() {
                return glib::Propagation::Proceed;
            }
            let weak_window = window.downgrade();
            let close = move |this: &Rc<Fields>| {
                this.closing.set(true);
                if let Some(window) = weak_window.upgrade() {
                    window.close();
                }
            };
            let weak = Rc::downgrade(&this);
            this.ask(move |answer| {
                let Some(this) = weak.upgrade() else { return };
                match answer {
                    Answer::Discard => close(&this),
                    Answer::Save => this.save_then(close),
                    Answer::Cancel => {}
                }
            });
            glib::Propagation::Stop
        });
        // Coming back to the window takes the main window's selection again.
        let weak = Rc::downgrade(self);
        self.window.connect_is_active_notify(move |window| {
            if let Some(this) = weak.upgrade()
                && window.is_active()
                && this.search.text().trim().is_empty()
            {
                this.load_tracks();
            }
        });
    }

    /// The field a row of the left pane stands for: its index, or `None` for
    /// "New field".
    fn row_choice(&self, row: &gtk::ListBoxRow) -> Option<usize> {
        let index = usize::try_from(row.index()).ok()?;
        (index < self.stored.borrow().len()).then_some(index)
    }

    /// Selects the row of `choice` without acting on it.
    fn select_row_of(&self, choice: Option<usize>) {
        self.filling.set(true);
        let row = match choice {
            Some(index) => self.list.row_at_index(i32::try_from(index).unwrap_or(0)),
            None => Some(self.new_row.clone()),
        };
        self.list.select_row(row.as_ref());
        self.filling.set(false);
    }

    /// Reads the saved fields and shows them, choosing the field `name` (the
    /// first one, or "New field" when there is none).
    fn reload(self: &Rc<Self>, name: Option<String>) {
        let services = self.services.clone();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let fields = backend
                .work(move || services.user_fields())
                .await
                .and_then(Result::ok)
                .unwrap_or_default();
            let Some(this) = weak.upgrade() else { return };
            this.show_list(fields, name.as_deref());
        });
    }

    fn show_list(self: &Rc<Self>, fields: Vec<UserField>, name: Option<&str>) {
        self.filling.set(true);
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        for field in &fields {
            self.list.append(&field_row(field));
        }
        self.list.append(&self.new_row);
        let choice = match name {
            Some(name) => fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(name)),
            None => (!fields.is_empty()).then_some(0),
        };
        self.stored.replace(fields);
        self.filling.set(false);
        self.select_row_of(choice);
        self.choose(choice);
    }

    /// Shows the field `choice` (`None`: a new one) in the editor.
    fn choose(self: &Rc<Self>, choice: Option<usize>) {
        let draft = choice
            .and_then(|i| self.stored.borrow().get(i).map(Draft::of))
            .unwrap_or_default();
        self.chosen.set(choice);
        self.select_row_of(choice);
        self.filling.set(true);
        self.name.set_text(&draft.name);
        self.expression.buffer().set_text(&draft.expression);
        self.tag.set_text(&draft.tag);
        let index = usize::from(draft.kind == Kind::Tag);
        self.kinds[index].set_active(true);
        self.draft.replace(draft);
        self.filling.set(false);
        self.shown.borrow_mut().clear();
        self.changed();
    }

    /// The saved field being edited.
    fn saved(&self) -> Option<UserField> {
        self.chosen
            .get()
            .and_then(|i| self.stored.borrow().get(i).cloned())
    }

    /// Whether the editor holds changes that are not saved.
    fn is_dirty(&self) -> bool {
        self.draft.borrow().changed_from(self.saved().as_ref())
    }

    /// Checks the draft and shows what follows from it.
    fn changed(self: &Rc<Self>) {
        let check = draft::check(&self.stored.borrow(), &self.draft.borrow());
        self.show_check(&check);
        self.check.replace(check);
        let tick = self.typing.get().wrapping_add(1);
        self.typing.set(tick);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(TYPING, move || {
            if let Some(this) = weak.upgrade()
                && this.typing.get() == tick
            {
                this.evaluate();
            }
        });
    }

    fn show_check(&self, check: &Check) {
        let draft = self.draft.borrow();
        let saved = self.saved();
        let name = draft.name.trim().to_ascii_lowercase();
        // The header.
        match &saved {
            Some(field) => {
                self.title.set_text(&field.name);
                self.title.add_css_class("hk-mono");
            }
            None => {
                self.title.set_text(&tr("New field"));
                self.title.remove_css_class("hk-mono");
            }
        }
        let users = saved
            .as_ref()
            .map(|f| draft::users_of(&self.stored.borrow(), &f.name))
            .unwrap_or_default();
        let summary = match (&saved, draft.kind) {
            (None, _) => tr("Not saved yet"),
            (Some(_), Kind::Tag) => tr("A tag of the files"),
            (Some(_), Kind::Computed) if users.is_empty() => tr("Computed"),
            (Some(_), Kind::Computed) => fill(
                &ntr(
                    "Computed · used by the field {}",
                    "Computed · used by the fields {}",
                    users.len(),
                ),
                &[&users.join(", ")],
            ),
        };
        self.summary.set_text(&summary);
        // The notices and their underlines.
        self.name_notice.show(check.name.as_ref());
        crate::widgets::pattern_entry::underline(&self.name, check.name.as_ref().map(|n| n.span));
        self.expression_notice.show(
            check
                .definition
                .as_ref()
                .filter(|_| draft.kind == Kind::Computed),
        );
        let buffer = self.expression.buffer();
        buffer.remove_tag(&self.error_tag, &buffer.start_iter(), &buffer.end_iter());
        if draft.kind == Kind::Computed
            && let Some(notice) = &check.definition
        {
            let count = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
            let (start, end) = notice.span;
            let last = buffer.char_count();
            let start = count(start).min((last - 1).max(0));
            let end = count(end.max(notice.span.0 + 1)).min(last).max(start);
            let (a, b) = (buffer.iter_at_offset(start), buffer.iter_at_offset(end));
            buffer.apply_tag(&self.error_tag, &a, &b);
        }
        // A tag that cannot be used is said under the tag's entry.
        if draft.kind == Kind::Tag {
            match &check.definition {
                Some(notice) => {
                    self.tag_help.set_text(&notice.message);
                    crate::widgets::pattern_entry::underline(&self.tag, Some(notice.span));
                }
                None => crate::widgets::pattern_entry::underline(&self.tag, None),
            }
        }
        self.use_help
            .set_visible(check.definition.is_none() || draft.kind == Kind::Tag);
        self.show_help(&name, &draft, check.definition.is_some());
        self.value_head.set_text(&name);
        // The footer.
        let problem = check.name.is_some() || check.definition.is_some();
        let line = if problem {
            tr("Nothing is saved while there is a problem.")
        } else if let (Some(field), Some(user)) = (&saved, users.first()) {
            fill(
                &tr("The field {} uses {}, so it cannot be removed."),
                &[user, &field.name],
            )
        } else {
            tr("Fields are kept in the library and apply at once.")
        };
        self.footer_line.set_text(&line);
        self.remove.set_visible(saved.is_some());
        self.remove.set_sensitive(users.is_empty());
        self.save.set_label(&if saved.is_some() {
            tr("Save")
        } else {
            tr("Add field")
        });
        self.save.set_sensitive(check.can_save());
        if problem || check.values.is_none() {
            self.values_stack.set_visible_child_name("empty");
        }
    }

    /// The lines under the expression and the tag on how to use the field.
    fn show_help(&self, name: &str, draft: &Draft, problem: bool) {
        let name = if name.is_empty() { "name" } else { name };
        let value = self
            .shown
            .borrow()
            .iter()
            .map(|(_, v)| v.trim().to_owned())
            .find(|v| !v.is_empty() && !v.contains(char::is_whitespace))
            .unwrap_or_else(|| "…".to_owned());
        let mono = |text: &str| {
            format!(
                "<span font_family=\"monospace\">{}</span>",
                glib::markup_escape_text(text)
            )
        };
        let plain = |text: String| glib::markup_escape_text(&text).to_string();
        if draft.kind == Kind::Computed {
            self.use_help.set_text(&fill(
                &tr(
                    "Write {} in names, or {} in searches. A search works the field out for every track first, which takes longer on a very large library.",
                ),
                &[&format!("{{{name}}}"), &format!("{name}={value}")],
            ));
            return;
        }
        if problem && !draft.tag.trim().is_empty() {
            return;
        }
        let key = helikodu_model::user_fields::tag_key(&draft.tag).unwrap_or_else(|| "MOOD".into());
        // The sentence is translated as a whole; its parts in mono.
        let pattern = plain(tr(
            "As in Vorbis comments, for example MOOD. {} in MP3 and {} in MP4 are read as the same tag. The tag editor writes this tag like any other field. Write {} in names, or {} in searches.",
        ));
        let markup = crate::i18n::fill(
            &pattern,
            &[
                &mono(&format!("TXXX:{key}")),
                &mono(&format!("----:com.apple.iTunes:{key}")),
                &mono(&format!("{{{name}}}")),
                &mono(&format!("{name}:{value}")),
            ],
        );
        self.tag_help.set_markup(&markup);
    }

    /// Reads the tracks of the example values: those of the typed search, or
    /// those selected in the main window, or the first of the library.
    fn load_tracks(self: &Rc<Self>) {
        let text = self.search.text().trim().to_owned();
        let backend = self.backend.clone();
        let target = if text.is_empty() {
            self.main.upgrade().and_then(|main| main.current_target())
        } else {
            None
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let mut keys = match target {
                Some(target) => target.keys(&backend).await,
                None => Vec::new(),
            };
            keys.truncate(SELECTED_TRACKS);
            if keys.is_empty() {
                let query = TrackQuery {
                    text,
                    ..TrackQuery::default()
                };
                keys = backend
                    .query(move |l| l.track_keys(&query))
                    .await
                    .unwrap_or_default();
                keys.truncate(FIRST_TRACKS);
            }
            let rows = backend
                .query(move |l| l.track_rows(&keys))
                .await
                .unwrap_or_default();
            let Some(this) = weak.upgrade() else { return };
            let same = {
                let tracks = this.tracks.borrow();
                tracks.len() == rows.len() && tracks.iter().zip(&rows).all(|(a, b)| a.key == b.key)
            };
            if same && !this.shown.borrow().is_empty() {
                return;
            }
            this.tracks.replace(rows);
            this.evaluate();
        });
    }

    /// Works out the example values of the draft for the tracks.
    fn evaluate(self: &Rc<Self>) {
        let Some(template) = self.check.borrow().values.clone() else {
            self.values_stack.set_visible_child_name("empty");
            return;
        };
        let keys: Vec<TrackKey> = self.tracks.borrow().iter().map(|r| r.key).collect();
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let services = self.services.clone();
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let values = backend
                .work(move || services.evaluate(&template, &keys))
                .await
                .and_then(Result::ok)
                .unwrap_or_default();
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            this.show_values(values);
        });
    }

    fn show_values(&self, values: Vec<(TrackKey, String)>) {
        while let Some(row) = self.values.first_child() {
            self.values.remove(&row);
        }
        let tracks = self.tracks.borrow();
        for (key, value) in &values {
            let Some(track) = tracks.iter().find(|t| t.key == *key) else {
                continue;
            };
            self.values.append(&value_row(track, value));
        }
        self.shown.replace(values);
        let can = self.check.borrow().values.is_some();
        self.values_stack
            .set_visible_child_name(if can { "values" } else { "empty" });
        let draft = self.draft.borrow().clone();
        let problem = self.check.borrow().definition.is_some();
        self.show_help(&draft.name.trim().to_ascii_lowercase(), &draft, problem);
    }

    /// Asks what to do with the unsaved changes, and hands the answer to
    /// `then`. Without a field that can be saved it offers only Discard and
    /// Cancel.
    fn ask(self: &Rc<Self>, then: impl FnOnce(Answer) + 'static) {
        if self.asking.replace(true) {
            return;
        }
        let draft = self.draft.borrow().clone();
        let name = draft.name.trim();
        let message = if name.is_empty() {
            tr("Save the new field?")
        } else {
            fill(&tr("Save the changes to {}?"), &[name])
        };
        let savable = self.check.borrow().can_save();
        let mut buttons = vec![tr("Discard"), tr("Cancel")];
        if savable {
            buttons.push(tr("Save"));
        }
        let detail = if savable {
            tr("The field is changed but not saved.")
        } else {
            tr("The field has a problem and cannot be saved as it is.")
        };
        let dialog = gtk::AlertDialog::builder()
            .message(message)
            .detail(detail)
            .buttons(buttons)
            .cancel_button(1)
            .default_button(if savable { 2 } else { 1 })
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |answer| {
            if let Some(this) = weak.upgrade() {
                this.asking.set(false);
            }
            then(match answer {
                Ok(0) => Answer::Discard,
                Ok(2) => Answer::Save,
                _ => Answer::Cancel,
            });
        });
    }

    /// Saves the draft, then calls `then`; on a refusal says why and does not.
    fn save_then(self: &Rc<Self>, then: impl FnOnce(&Rc<Self>) + 'static) {
        if !self.check.borrow().can_save() {
            return;
        }
        let draft = self.draft.borrow().clone();
        let field = draft.field();
        let old = draft.saved.clone();
        let services = self.services.clone();
        let backend = self.backend.clone();
        self.save.set_sensitive(false);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let saved = backend
                .work(move || services.save_field(old.as_deref(), field))
                .await;
            let Some(this) = weak.upgrade() else { return };
            match saved {
                Some(Ok(stored)) => {
                    let services = this.services.clone();
                    let fields = this
                        .backend
                        .work(move || services.user_fields())
                        .await
                        .and_then(Result::ok)
                        .unwrap_or_default();
                    this.show_list(fields, Some(&stored.name));
                    then(&this);
                }
                Some(Err(error)) => {
                    this.footer_line.set_text(&error.0);
                    this.save.set_sensitive(true);
                }
                None => this.save.set_sensitive(true),
            }
        });
    }

    /// Asks before removing the saved field being edited, then removes it.
    fn confirm_remove(self: &Rc<Self>) {
        let Some(field) = self.saved() else { return };
        let dialog = gtk::AlertDialog::builder()
            .message(fill(&tr("Remove the field {}?"), &[&field.name]))
            .detail(tr(
                "Searches, smart playlists and naming patterns that name it no longer find it.",
            ))
            .buttons([tr("Cancel"), tr("Remove")])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |answer| {
            if answer != Ok(1) {
                return;
            }
            let Some(this) = weak.upgrade() else { return };
            let services = this.services.clone();
            let backend = this.backend.clone();
            let name = field.name.clone();
            glib::spawn_future_local(async move {
                let removed = backend.work(move || services.remove_field(&name)).await;
                let Some(this) = weak.upgrade() else { return };
                match removed {
                    Some(Ok(())) => this.reload(None),
                    Some(Err(error)) => this.footer_line.set_text(&error.0),
                    None => {}
                }
            });
        });
    }
}

/// The two columns of the example values, of one width each, so that the
/// values stand under their header.
fn columns(first: &impl IsA<gtk::Widget>, second: &impl IsA<gtk::Widget>) -> gtk::Box {
    let pair = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    pair.set_homogeneous(true);
    pair.set_hexpand(true);
    pair.append(first);
    pair.append(second);
    pair
}

/// A row of the example values: "Title · Artist" and the value in mono, or
/// "no value".
fn value_row(track: &TrackRow, value: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    row.add_css_class("hk-fields-value-row");
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let name = label(&track.title, &[]);
    title.append(&name);
    if !track.artist.is_empty() {
        let artist = label(&format!("  ·  {}", track.artist), &["hk-t3"]);
        artist.set_max_width_chars(1);
        artist.set_hexpand(true);
        title.append(&artist);
    }
    let shown = if value.trim().is_empty() {
        label(&tr("no value"), &["hk-mono", "hk-t3", "hk-fields-value"])
    } else {
        label(value, &["hk-mono", "hk-fields-value"])
    };
    shown.set_max_width_chars(1);
    row.append(&columns(&title, &shown));
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_size_request(238, -1);
    row.append(&spacer);
    row.update_property(&[gtk::accessible::Property::Label(&format!(
        "{} · {}: {}",
        track.title,
        track.artist,
        if value.trim().is_empty() {
            tr("no value")
        } else {
            value.to_owned()
        }
    ))]);
    row
}
