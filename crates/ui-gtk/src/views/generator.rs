// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A generator playlist: the hero (the steps tile, the name, a summary, Play,
//! Shuffle, Edit steps, Make again and more), then either the steps line and
//! the table of its tracks, or, while editing, the steps beside the tracks
//! they leave.
//!
//! The editor keeps the steps it edits to itself and tries them half a second
//! after the last change; each step row ends with the number of tracks left
//! after it. "Done editing" stores them (which makes the playlist again). A
//! step that cannot run (a playlist that is gone, a query that cannot be
//! read) gets a notice under its row and a warning on its chip; the tracks of
//! the last result stay. Steps move by dragging their grip, or with Alt+Up and
//! Alt+Down; Delete removes the focused step. The source is always first.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::generator_steps::{
    SORTS, StepKind, chips, compare, directions, made_words, sort_label, sort_value, sorted_by,
};
use super::list_table::{Column, ColumnKind, ListTable, Value, follow_playing};
use crate::backend::{
    Backend, GeneratorInfo, GeneratorRun, GeneratorSort, GeneratorSource, GeneratorStep,
    GeneratorSteps, GeneratorTrack, PlayerRequest, PlaylistKey, PlaylistType, StepProblem,
    TrackKey, TrackRow,
};
use crate::dialogs::common::dropdown;
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::playback::Playback;
use crate::popovers::query_completion::QueryCompletion;
use crate::views::actions::Target;
use crate::views::artists::button;
use crate::views::table::PlayingRows;
use crate::widgets::{WrapBox, label};

/// How long the editor waits after a change before it tries the steps.
const TRY_AFTER: Duration = Duration::from_millis(500);

/// The width of the step rows' kind dropdown.
const KIND_WIDTH: i32 = 108;

/// The page of a generator playlist.
pub(crate) struct GeneratorView {
    pub(crate) root: gtk::Box,
    name: gtk::Label,
    summary: gtk::Label,
    edit: gtk::Button,
    again: gtk::Button,
    again_icon: gtk::Stack,
    more: gtk::MenuButton,
    stack: gtk::Stack,
    chips: WrapBox,
    body: gtk::Stack,
    view: Rc<ListTable>,
    status_left: gtk::Label,
    steps_box: gtk::Box,
    on_open: gtk::Switch,
    results_count: gtk::Label,
    results_time: gtk::Label,
    results_note: gtk::Label,
    results: Rc<ListTable>,
    backend: Backend,
    key: Cell<Option<PlaylistKey>>,
    info: RefCell<Option<GeneratorInfo>>,
    /// The tracks of the page: the last result or the new one.
    run: RefCell<GeneratorRun>,
    /// The steps being edited.
    draft: RefCell<GeneratorSteps>,
    /// What the edited steps leave.
    trial: RefCell<GeneratorRun>,
    /// The tracks in the order shown, in the page's table and the results.
    shown: RefCell<Vec<GeneratorTrack>>,
    shown_results: RefCell<Vec<GeneratorTrack>>,
    /// The column the page's table is sorted by for this visit, and whether
    /// descending.
    sorted: Cell<Option<(&'static str, bool)>>,
    editing: Cell<bool>,
    making: Cell<bool>,
    /// Set while the switch is set from the playlist, so that it saves nothing.
    filling: Cell<bool>,
    generation: Cell<u64>,
    trial_generation: Cell<u64>,
    pending: RefCell<Option<glib::SourceId>>,
    /// The playlists a step can name, by name.
    playlists: RefCell<Vec<String>>,
    /// The step row to focus once the rows are built again.
    focus_step: Cell<Option<usize>>,
    /// The query entries of the step rows and their completions, top to bottom.
    completions: RefCell<Vec<(glib::WeakRef<gtk::Entry>, std::rc::Weak<QueryCompletion>)>>,
    _playing: Rc<PlayingRows>,
}

impl std::fmt::Debug for GeneratorView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratorView")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

/// A button with an icon and text whose icon can give way to a spinner.
fn again_button() -> (gtk::Button, gtk::Stack) {
    let b = gtk::Button::new();
    b.add_css_class("hk-btn");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let icon = gtk::Stack::new();
    // The circular arrow of the design is drawn by another part of the
    // window; until it is bundled, the desktop's refresh icon stands in.
    icon.add_named(
        &gtk::Image::from_icon_name("view-refresh-symbolic"),
        Some("icon"),
    );
    let spinner = gtk::Spinner::new();
    icon.add_named(&spinner, Some("spinner"));
    line.append(&icon);
    line.append(&gtk::Label::new(Some(&tr("Make again"))));
    b.set_child(Some(&line));
    b.update_property(&[gtk::accessible::Property::Label(&tr("Make again"))]);
    (b, icon)
}

/// The tracks in words with their length: "50 tracks · 3 h 12 min".
fn count_and_length(tracks: &[GeneratorTrack]) -> String {
    let length: Duration = tracks.iter().filter_map(|t| t.row.duration).sum();
    format!(
        "{} · {}",
        fill(
            &ntr("{} track", "{} tracks", tracks.len()),
            &[&format::count(tracks.len())]
        ),
        format::minutes(length)
    )
}

/// The steps can be stored: their text form reads back as they are.
fn storable(steps: &GeneratorSteps) -> bool {
    steps.to_string().parse::<GeneratorSteps>().ok().as_ref() == Some(steps)
}

impl GeneratorView {
    /// The page over `backend`.
    pub(crate) fn new(
        backend: &Backend,
        covers: &Rc<CoverCache>,
        playback: &Rc<Playback>,
    ) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-generator");
        let hero = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        hero.add_css_class("hk-playlist-hero");
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
        tile.add_css_class("hk-smart-tile");
        tile.set_size_request(88, 88);
        tile.set_valign(gtk::Align::Center);
        tile.set_homogeneous(true);
        let steps_icon = gtk::Image::from_icon_name("helikodu-steps-symbolic");
        steps_icon.set_pixel_size(34);
        steps_icon.set_valign(gtk::Align::Center);
        steps_icon.set_halign(gtk::Align::Center);
        tile.append(&steps_icon);
        hero.append(&tile);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        titles.set_valign(gtk::Align::Center);
        titles.append(&label(&tr("Generator playlist"), &["hk-section-label"]));
        let name = label("", &["hk-hero-title", "small"]);
        name.set_accessible_role(gtk::AccessibleRole::Heading);
        name.set_max_width_chars(1);
        titles.append(&name);
        let summary = label("", &["hk-t3"]);
        summary.set_max_width_chars(1);
        titles.append(&summary);
        hero.append(&titles);
        let play = button(&tr("Play"), Some("media-playback-start-symbolic"), true);
        let shuffle = button(
            &tr("Shuffle"),
            Some("media-playlist-shuffle-symbolic"),
            false,
        );
        let edit = button(&tr("Edit steps"), Some("document-edit-symbolic"), false);
        let (again, again_icon) = again_button();
        let more = gtk::MenuButton::new();
        more.set_icon_name("view-more-symbolic");
        more.add_css_class("hk-more");
        more.set_tooltip_text(Some(&tr("More")));
        more.update_property(&[gtk::accessible::Property::Label(&tr("More"))]);
        let buttons = WrapBox::new(8);
        buttons.set_valign(gtk::Align::Center);
        for b in [
            play.upcast_ref::<gtk::Widget>(),
            shuffle.upcast_ref(),
            edit.upcast_ref(),
            again.upcast_ref(),
            more.upcast_ref(),
        ] {
            buttons.append(b);
        }
        hero.append(&buttons);
        root.append(&hero);

        let playing = Rc::new(PlayingRows::default());
        // Viewing: the steps line, the table or the empty sentence, the
        // status bar.
        let viewing = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.add_css_class("hk-rule-line");
        let chips_box = WrapBox::new(6);
        chips_box.set_hexpand(true);
        chips_box.update_property(&[gtk::accessible::Property::Label(&tr("Steps"))]);
        line.append(&chips_box);
        viewing.append(&line);
        let view = ListTable::new(
            &tr("Tracks of the generator playlist"),
            false,
            covers,
            &playing,
        );
        let table_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        table_box.append(&view.header);
        table_box.append(&view.scroller);
        let empty = gtk::Box::new(gtk::Orientation::Vertical, 14);
        empty.set_valign(gtk::Align::Center);
        empty.set_halign(gtk::Align::Center);
        empty.set_vexpand(true);
        let sentence = label(&tr("The steps leave no tracks."), &["hk-t2"]);
        sentence.set_halign(gtk::Align::Center);
        empty.append(&sentence);
        let edit_empty = button(&tr("Edit steps"), Some("document-edit-symbolic"), false);
        edit_empty.set_halign(gtk::Align::Center);
        empty.append(&edit_empty);
        let body = gtk::Stack::new();
        body.set_vexpand(true);
        body.add_named(&table_box, Some("table"));
        body.add_named(&empty, Some("empty"));
        viewing.append(&body);
        let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        status.add_css_class("hk-statusbar");
        let status_left = label("", &[]);
        status_left.set_hexpand(true);
        status_left.set_max_width_chars(1);
        status.append(&status_left);
        let status_right = label(
            &tr("The steps set the order. To reorder by hand, save a copy as a playlist."),
            &[],
        );
        status.append(&status_right);
        viewing.append(&status);

        // Editing: the steps pane beside the results pane.
        let editing = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        editing.set_layout_manager(Some(
            glib::Object::new::<super::smart_playlist::EditorLayout>(),
        ));
        editing.add_css_class("hk-rule-editor");
        let pane = gtk::Box::new(gtk::Orientation::Vertical, 8);
        pane.add_css_class("hk-rules-pane");
        pane.add_css_class("hk-steps-pane");
        let explain = label(
            &tr("Each step works on the tracks the step before it left."),
            &["hk-t2"],
        );
        explain.set_wrap(true);
        explain.set_margin_bottom(4);
        pane.append(&explain);
        let steps_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pane.append(&steps_box);
        pane.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let (switch_line, on_open) = crate::dialogs::common::switch_row(
            &tr("Make again when opened"),
            Some(&tr(
                "Off: the tracks stay as they are until you choose Make again.",
            )),
        );
        on_open.set_widget_name("generator-on-open");
        pane.append(&switch_line);
        let pane_scroller = gtk::ScrolledWindow::new();
        pane_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        pane_scroller.set_vexpand(true);
        pane_scroller.set_child(Some(&pane));
        pane_scroller.set_hexpand(false);
        pane_scroller.add_css_class("hk-rules-scroller");
        editing.append(&pane_scroller);
        let divider = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        divider.add_css_class("hk-rules-divider");
        editing.append(&divider);
        let results_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        results_box.set_hexpand(true);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.add_css_class("hk-results-head");
        let results_count = label("", &["hk-strong"]);
        head.append(&results_count);
        let results_time = label("", &["hk-t3"]);
        results_time.set_hexpand(true);
        head.append(&results_time);
        let results_note = label("", &["hk-t3", "hk-small"]);
        head.append(&results_note);
        results_box.append(&head);
        let results = ListTable::new(&tr("Tracks the steps leave"), true, covers, &playing);
        results_box.append(&results.header);
        results_box.append(&results.scroller);
        editing.append(&results_box);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.add_named(&viewing, Some("view"));
        stack.add_named(&editing, Some("edit"));
        root.append(&stack);

        let view = Rc::new(Self {
            root,
            name,
            summary,
            edit,
            again,
            again_icon,
            more,
            stack,
            chips: chips_box,
            body,
            view,
            status_left,
            steps_box,
            on_open,
            results_count,
            results_time,
            results_note,
            results,
            backend: backend.clone(),
            key: Cell::new(None),
            info: RefCell::new(None),
            run: RefCell::new(GeneratorRun::default()),
            draft: RefCell::new(GeneratorSteps::default()),
            trial: RefCell::new(GeneratorRun::default()),
            shown: RefCell::new(Vec::new()),
            shown_results: RefCell::new(Vec::new()),
            sorted: Cell::new(None),
            editing: Cell::new(false),
            making: Cell::new(false),
            filling: Cell::new(false),
            generation: Cell::new(0),
            trial_generation: Cell::new(0),
            pending: RefCell::new(None),
            playlists: RefCell::new(Vec::new()),
            focus_step: Cell::new(None),
            completions: RefCell::new(Vec::new()),
            _playing: playing.clone(),
        });
        follow_playing(&playing, playback);
        view.connect(&play, &shuffle, &edit_empty);
        view.set_columns();
        view
    }

    fn window(&self) -> Option<crate::window::MainWindow> {
        self.root.root().and_downcast::<crate::window::MainWindow>()
    }

    fn connect(
        self: &Rc<Self>,
        play: &gtk::Button,
        shuffle: &gtk::Button,
        edit_empty: &gtk::Button,
    ) {
        for (b, random) in [(play, false), (shuffle, true)] {
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.play_from(0, random);
                }
            });
        }
        for b in [&self.edit, edit_empty] {
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.set_editing(!view.editing.get());
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.again.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.make_again();
            }
        });
        let weak = Rc::downgrade(self);
        self.more.set_create_popup_func(move |more| {
            let Some(view) = weak.upgrade() else { return };
            let (Some(window), Some(key)) = (view.window(), view.key.get()) else {
                return;
            };
            more.set_menu_model(Some(&window.playlist_menu_model(key)));
            crate::widgets::menu_names::name_menus(more);
        });
        let weak = Rc::downgrade(self);
        self.view.connect(move |position| {
            if let Some(view) = weak.upgrade() {
                view.play_from(position, false);
            }
        });
        let weak = Rc::downgrade(self);
        self.results.connect(move |position| {
            if let Some(view) = weak.upgrade() {
                view.play_results_from(position);
            }
        });
        let weak = Rc::downgrade(self);
        self.view
            .selection
            .connect_selection_changed(move |_, _, _| {
                if let Some(view) = weak.upgrade() {
                    view.show_status();
                }
            });
        let weak = Rc::downgrade(self);
        self.on_open.connect_active_notify(move |switch| {
            let Some(view) = weak.upgrade() else { return };
            if view.filling.get() {
                return;
            }
            view.set_on_open(switch.is_active());
        });
    }

    /// The columns of both tables: the column the steps sort by joins them.
    fn set_columns(self: &Rc<Self>) {
        let steps = if self.editing.get() {
            self.draft.borrow().clone()
        } else {
            self.info
                .borrow()
                .as_ref()
                .map(|i| i.steps.clone())
                .unwrap_or_default()
        };
        let by = sorted_by(&steps);
        let visit = self.sorted.get();
        let arrow = |id: &str| -> Option<&'static str> {
            match visit {
                Some((c, descending)) if c == id => Some(if descending { " ↓" } else { " ↑" }),
                Some(_) => None,
                None => match by {
                    Some((b, descending)) if sort_id(b) == id => {
                        Some(if descending { " ↓" } else { " ↑" })
                    }
                    _ => None,
                },
            }
        };
        // The narrow window keeps the number, the title and the length.
        let mut columns = vec![
            Column::new("number", "#".into(), 22, ColumnKind::Number)
                .right()
                .kept_narrow(),
            Column::new("title", tr("Title"), 0, ColumnKind::TitleWithCover).kept_narrow(),
            Column::new("artist", tr("Artist"), 0, ColumnKind::Text),
            Column::new("album", tr("Album"), 0, ColumnKind::Text),
        ];
        let extra = by.map(|(b, _)| b).filter(|b| {
            !matches!(
                b,
                GeneratorSort::Title
                    | GeneratorSort::Artist
                    | GeneratorSort::Album
                    | GeneratorSort::Length
            )
        });
        if let Some(b) = extra {
            columns.push(Column::new(sort_id(b), sort_label(b), 104, ColumnKind::Text).right());
        }
        columns.push(
            Column::new("length", tr("Length"), 48, ColumnKind::Text)
                .right()
                .kept_narrow(),
        );
        for column in &mut columns {
            column.arrow = arrow(column.id);
        }
        let weak = Rc::downgrade(self);
        self.view.set_columns(
            columns,
            move |position, row, id| {
                let Some(view) = weak.upgrade() else {
                    return Value::Text(String::new());
                };
                let shown = view.shown.borrow();
                Value::Text(cell_text(shown.get(position as usize), row, id))
            },
            {
                let weak = Rc::downgrade(self);
                move |id| {
                    if let Some(view) = weak.upgrade() {
                        view.sort_by(id);
                    }
                }
            },
        );
        // The results beside the steps: no cover, no length, the steps' order.
        let mut columns = vec![
            Column::new("number", "#".into(), 20, ColumnKind::Number).right(),
            Column::new("title", tr("Title"), 0, ColumnKind::Title),
            Column::new("artist", tr("Artist"), 0, ColumnKind::Text),
            Column::new("album", tr("Album"), 0, ColumnKind::Text),
        ];
        if let Some(b) = extra {
            let mut column = Column::new(sort_id(b), sort_label(b), 96, ColumnKind::Text).right();
            // Its header is in t1, without an arrow.
            column.arrow = Some("");
            columns.push(column);
        }
        let weak = Rc::downgrade(self);
        self.results.set_columns(
            columns,
            move |position, row, id| {
                let Some(view) = weak.upgrade() else {
                    return Value::Text(String::new());
                };
                let shown = view.shown_results.borrow();
                Value::Text(cell_text(shown.get(position as usize), row, id))
            },
            |_| {},
        );
    }

    /// Shows the generator `key`, viewing or editing its steps.
    pub(crate) fn show(self: &Rc<Self>, key: PlaylistKey, editing: bool) {
        if self.key.get() != Some(key) {
            self.leave_editing();
            self.key.set(Some(key));
            self.sorted.set(None);
            self.info.replace(None);
            self.run.replace(GeneratorRun::default());
            self.view.store.remove_all();
            self.results.store.remove_all();
            self.name.set_text("");
            self.summary.set_text("");
            self.editing.set(false);
        }
        self.load(editing);
    }

    /// The generator shown.
    pub(crate) fn key(&self) -> Option<PlaylistKey> {
        self.key.get()
    }

    /// Whether the steps are being edited.
    pub(crate) fn is_editing(&self) -> bool {
        self.editing.get()
    }

    /// Reads the generator and its tracks, made again when it is made on
    /// open; then shows the page or the editor.
    fn load(self: &Rc<Self>, editing: bool) {
        let Some(key) = self.key.get() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| {
                    let info = l.generator(key)?;
                    let run = l.generator_tracks(key, false)?;
                    let names = choices(l, key)?;
                    Ok((info, run, names))
                })
                .await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation || view.key.get() != Some(key) {
                return;
            }
            match result {
                Ok((Some(info), run, names)) => {
                    view.playlists.replace(names);
                    view.info.replace(Some(info));
                    view.run.replace(run);
                    view.show_info();
                    view.set_editing(editing);
                    if run_is_fresh(&view.run.borrow()) {
                        view.announce_made();
                    }
                }
                Ok((None, ..)) => {}
                Err(error) => {
                    if let Some(window) = view.window() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    /// After the tracks were made again: the sidebar's count follows.
    fn announce_made(&self) {
        if let Some(window) = self.window() {
            window.playlists_changed(None);
        }
    }

    /// The hero, the chips, the table and the status bar from what was read.
    fn show_info(self: &Rc<Self>) {
        let Some(info) = self.info.borrow().clone() else {
            return;
        };
        self.name.set_text(&info.name);
        self.filling.set(true);
        self.on_open.set_active(info.on_open);
        self.filling.set(false);
        self.show_summary();
        self.show_chips();
        self.set_columns();
        self.show_rows();
    }

    fn show_summary(&self) {
        let run = self.run.borrow();
        if self.making.get() {
            self.summary.set_text(&tr("Making it again…"));
            return;
        }
        let mut parts = vec![count_and_length(&run.tracks)];
        if let Some(made) = run.made {
            parts.push(made_words(made));
        }
        if run.problem.is_some() {
            parts.push(tr("could not be made again"));
        }
        self.summary.set_text(&parts.join(" · "));
    }

    fn show_chips(&self) {
        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        let Some(info) = self.info.borrow().clone() else {
            return;
        };
        let run = self.run.borrow();
        for (i, chip) in chips(&info.steps, run.problem.as_ref())
            .into_iter()
            .enumerate()
        {
            if i > 0 {
                let chevron = gtk::Image::from_icon_name("go-next-symbolic");
                chevron.set_pixel_size(12);
                chevron.add_css_class("hk-t3");
                chevron.set_valign(gtk::Align::Center);
                self.chips.append(&chevron);
            }
            let b = gtk::Button::new();
            b.add_css_class("hk-rule-chip");
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            if chip.problem {
                let warning = gtk::Image::from_icon_name("helikodu-warning-symbolic");
                warning.set_pixel_size(12);
                line.append(&warning);
                b.add_css_class("problem");
            }
            let text = gtk::Label::new(Some(&chip.text));
            if chip.mono {
                text.add_css_class("hk-mono");
            }
            line.append(&text);
            b.set_child(Some(&line));
            b.update_property(&[gtk::accessible::Property::Label(&chip.text)]);
            b.set_tooltip_text(Some(&tr("Edit steps")));
            b.set_action_name(Some("generator-page.edit"));
            self.chips.append(&b);
        }
    }

    /// The tracks of the page in the order shown: the steps' order, or the
    /// column clicked for this visit.
    fn show_rows(&self) {
        let mut tracks = self.run.borrow().tracks.clone();
        if let Some((id, descending)) = self.sorted.get() {
            let by = match id {
                "number" => None,
                "title" => Some(GeneratorSort::Title),
                "artist" => Some(GeneratorSort::Artist),
                "album" => Some(GeneratorSort::Album),
                "length" => Some(GeneratorSort::Length),
                other => SORTS.into_iter().find(|b| sort_id(*b) == other),
            };
            if let Some(by) = by {
                tracks.sort_by(|a, b| {
                    let order = compare(by, a, b);
                    if descending { order.reverse() } else { order }
                });
            }
        }
        let rows: Vec<TrackRow> = tracks.iter().map(|t| t.row.clone()).collect();
        self.shown.replace(tracks);
        self.view.show(&rows);
        self.body
            .set_visible_child_name(if rows.is_empty() { "empty" } else { "table" });
        self.show_status();
    }

    fn show_status(&self) {
        let tracks = self.shown.borrow();
        let selected = crate::views::table::selected_positions(&self.view.selection);
        let first = if selected.is_empty() {
            count_and_length(&tracks)
        } else {
            fill(
                &ntr("{} selected", "{} selected", selected.len()),
                &[&format::count(selected.len())],
            )
        };
        let kept = if self.on_open.is_active() {
            tr("made again each time it is opened")
        } else {
            tr("kept until you choose Make again")
        };
        self.status_left.set_text(&format!("{first} · {kept}"));
    }

    fn sort_by(self: &Rc<Self>, id: &'static str) {
        let next = match self.sorted.get() {
            Some((c, false)) if c == id => Some((id, true)),
            Some((c, true)) if c == id => None,
            _ => Some((id, false)),
        };
        self.sorted.set(next);
        self.set_columns();
        self.show_rows();
    }

    fn title(&self) -> String {
        self.info
            .borrow()
            .as_ref()
            .map(|i| i.name.clone())
            .unwrap_or_default()
    }

    fn play_from(&self, start: usize, shuffle: bool) {
        let tracks: Vec<TrackKey> = self.shown.borrow().iter().map(|t| t.row.key).collect();
        self.play(tracks, start, shuffle);
    }

    fn play_results_from(&self, start: usize) {
        let tracks: Vec<TrackKey> = self
            .shown_results
            .borrow()
            .iter()
            .map(|t| t.row.key)
            .collect();
        self.play(tracks, start, false);
    }

    fn play(&self, tracks: Vec<TrackKey>, start: usize, shuffle: bool) {
        if tracks.is_empty() {
            return;
        }
        let start = start.min(tracks.len() - 1);
        self.backend.send(PlayerRequest::PlayTracks {
            tracks,
            start,
            title: self.title(),
            shuffle,
        });
    }

    /// Makes the playlist again; the tracks of the last result stay while it
    /// works, and a toast says when it is done if the page is no longer open.
    pub(crate) fn make_again(self: &Rc<Self>) {
        let Some(key) = self.key.get() else { return };
        if self.making.replace(true) {
            return;
        }
        self.again.set_sensitive(false);
        self.again_icon.set_visible_child_name("spinner");
        if let Some(spinner) = self
            .again_icon
            .child_by_name("spinner")
            .and_downcast::<gtk::Spinner>()
        {
            spinner.start();
        }
        self.show_summary();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend.query(move |l| l.generator_tracks(key, true)).await;
            let Some(view) = weak.upgrade() else { return };
            view.making.set(false);
            view.again.set_sensitive(true);
            view.again_icon.set_visible_child_name("icon");
            if let Some(spinner) = view
                .again_icon
                .child_by_name("spinner")
                .and_downcast::<gtk::Spinner>()
            {
                spinner.stop();
            }
            match result {
                Ok(run) => {
                    let count = run.tracks.len();
                    let fresh = run.problem.is_none();
                    let still_open = view.key.get() == Some(key) && view.root.is_mapped();
                    if view.key.get() == Some(key) {
                        view.run.replace(run);
                        view.show_info();
                    }
                    if fresh {
                        view.announce_made();
                    }
                    if fresh
                        && !still_open
                        && let Some(window) = view.window()
                    {
                        let name = window.playlist_row(key).map(|p| p.name).unwrap_or_default();
                        window.toast_marked(
                            &fill(
                                &ntr(
                                    "“{}” was made again: {} track",
                                    "“{}” was made again: {} tracks",
                                    count,
                                ),
                                &[&name, &format::count(count)],
                            ),
                            crate::window::ToastLead::Check,
                            None,
                            false,
                            || {},
                        );
                    }
                }
                Err(error) => {
                    view.show_summary();
                    if let Some(window) = view.window() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    fn set_on_open(self: &Rc<Self>, on: bool) {
        let Some(key) = self.key.get() else { return };
        if let Some(info) = self.info.borrow_mut().as_mut() {
            info.on_open = on;
        }
        self.show_status();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| l.set_generator(key, None, Some(on)))
                .await;
            if let (Some(view), Err(error)) = (weak.upgrade(), result)
                && let Some(window) = view.window()
            {
                window.toast(&error.0, true);
            }
        });
    }

    /// Switches between viewing and editing the steps.
    pub(crate) fn set_editing(self: &Rc<Self>, editing: bool) {
        let was = self.editing.replace(editing);
        if was && !editing {
            self.save_draft();
        }
        self.stack
            .set_visible_child_name(if editing { "edit" } else { "view" });
        let (text, icon) = if editing {
            (tr("Done editing"), "object-select-symbolic")
        } else {
            (tr("Edit steps"), "document-edit-symbolic")
        };
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.append(&gtk::Image::from_icon_name(icon));
        line.append(&gtk::Label::new(Some(&text)));
        self.edit.set_child(Some(&line));
        self.edit
            .update_property(&[gtk::accessible::Property::Label(&text)]);
        if editing {
            self.edit.add_css_class("hk-pressed");
        } else {
            self.edit.remove_css_class("hk-pressed");
        }
        // The results are always made from the steps as they are.
        self.again.set_visible(!editing);
        self.more.set_visible(!editing);
        if editing && !was {
            let steps = self
                .info
                .borrow()
                .as_ref()
                .map(|i| i.steps.clone())
                .unwrap_or_default();
            self.draft.replace(steps);
            // Until the steps are tried, the results are those of the page.
            let run = self.run.borrow().clone();
            self.trial.replace(run);
            self.build_steps();
            self.show_trial();
            self.try_now();
        }
        self.set_columns();
        if !editing {
            self.show_rows();
        }
    }

    /// Leaving the page or another generator while editing keeps the steps.
    fn leave_editing(self: &Rc<Self>) {
        if self.editing.get() {
            self.save_draft();
            self.editing.set(false);
        }
    }

    /// Stores the edited steps when they changed and can be stored; the page
    /// then shows what they leave.
    fn save_draft(self: &Rc<Self>) {
        if let Some(id) = self.pending.take() {
            id.remove();
        }
        let Some(key) = self.key.get() else { return };
        let draft = self.draft.borrow().clone();
        let unchanged = self
            .info
            .borrow()
            .as_ref()
            .is_some_and(|i| i.steps == draft);
        if unchanged || !storable(&draft) {
            return;
        }
        if let Some(info) = self.info.borrow_mut().as_mut() {
            info.steps = draft.clone();
        }
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| {
                    l.set_generator(key, Some(&draft), None)?;
                    l.generator_tracks(key, false)
                })
                .await;
            let Some(view) = weak.upgrade() else { return };
            match result {
                Ok(run) => {
                    if view.key.get() == Some(key) {
                        view.run.replace(run);
                        view.show_info();
                    }
                    view.announce_made();
                }
                Err(error) => {
                    if let Some(window) = view.window() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    /// Tries the edited steps half a second after the last change.
    fn steps_changed(self: &Rc<Self>, rebuild: bool) {
        if rebuild {
            self.build_steps();
        }
        self.set_columns();
        self.results_count.set_text(&tr("Making…"));
        if let Some(id) = self.pending.take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(TRY_AFTER, move || {
            if let Some(view) = weak.upgrade() {
                view.pending.replace(None);
                view.try_now();
            }
        });
        self.pending.replace(Some(id));
    }

    fn try_now(self: &Rc<Self>) {
        let Some(key) = self.key.get() else { return };
        let steps = self.draft.borrow().clone();
        let generation = self.trial_generation.get() + 1;
        self.trial_generation.set(generation);
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend.query(move |l| l.try_generator(key, &steps)).await;
            let Some(view) = weak.upgrade() else { return };
            if view.trial_generation.get() != generation || !view.editing.get() {
                return;
            }
            match result {
                Ok(trial) => {
                    let mut trial = trial;
                    // A playlist that is gone is no choice: its dropdown
                    // reads "Choose a playlist".
                    if let Some((_, StepProblem::MissingPlaylist(name))) = &trial.problem {
                        view.playlists
                            .borrow_mut()
                            .retain(|n| !n.eq_ignore_ascii_case(name));
                    }
                    if trial.problem.is_some() {
                        // The tracks of the last result stay.
                        let run = view.run.borrow();
                        trial.tracks = run.tracks.clone();
                        trial.made = run.made;
                    }
                    view.trial.replace(trial);
                    view.build_steps();
                    view.show_trial();
                }
                Err(error) => {
                    if let Some(window) = view.window() {
                        window.toast(&error.0, true);
                    }
                }
            }
        });
    }

    fn show_trial(&self) {
        let trial = self.trial.borrow();
        let tracks = &trial.tracks;
        let length: Duration = tracks.iter().filter_map(|t| t.row.duration).sum();
        self.results_count.set_text(&fill(
            &ntr("{} track", "{} tracks", tracks.len()),
            &[&format::count(tracks.len())],
        ));
        self.results_time.set_text(&format::minutes(length));
        self.results_note
            .set_text(&match (&trial.problem, trial.made) {
                (Some(_), Some(made)) => super::generator_steps::as_made_words(made),
                _ => tr("As the steps are now"),
            });
        let rows: Vec<TrackRow> = tracks.iter().map(|t| t.row.clone()).collect();
        self.shown_results.replace(tracks.clone());
        self.results.show(&rows);
    }

    /// The playlists a step can name, with `current` first when it is not
    /// among them (it reads "Choose a playlist").
    fn playlist_choices(&self, current: &str) -> (Vec<String>, usize, bool) {
        let names = self.playlists.borrow().clone();
        match names.iter().position(|n| n.eq_ignore_ascii_case(current)) {
            Some(at) => (names, at, false),
            None => {
                let mut with = vec![tr("Choose a playlist")];
                with.extend(names);
                (with, 0, true)
            }
        }
    }

    /// Builds the step rows from the edited steps.
    fn build_steps(self: &Rc<Self>) {
        self.completions.borrow_mut().clear();
        while let Some(child) = self.steps_box.first_child() {
            self.steps_box.remove(&child);
        }
        let steps = self.draft.borrow().clone();
        let trial = self.trial.borrow().clone();
        let count = |index: usize| trial.counts.get(index).copied();
        let problem = trial.problem.clone();
        let source_row = self.source_row(&steps.source, count(0));
        self.steps_box.append(&source_row);
        if let Some((0, p)) = &problem {
            self.steps_box.append(&self.notice(p));
        }
        for (i, step) in steps.steps.iter().enumerate() {
            let row = self.step_row(i, step, count(i + 1));
            self.steps_box.append(&row);
            if let Some((n, p)) = &problem
                && *n == i + 1
            {
                self.steps_box.append(&self.notice(p));
            }
            if self.focus_step.get() == Some(i) {
                self.focus_step.set(None);
                let row = row.clone();
                glib::idle_add_local_once(move || {
                    row.grab_focus();
                });
            }
        }
        let add = gtk::MenuButton::new();
        add.add_css_class("hk-btn");
        add.add_css_class("small");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.append(&gtk::Image::from_icon_name("list-add-symbolic"));
        line.append(&gtk::Label::new(Some(&tr("Add step"))));
        line.append(&gtk::Image::from_icon_name("pan-down-symbolic"));
        add.set_child(Some(&line));
        add.set_halign(gtk::Align::Start);
        add.set_margin_start(16 + 8 + 14 + 8);
        add.set_margin_top(6);
        add.set_margin_bottom(8);
        let menu = gio::Menu::new();
        for (i, kind) in StepKind::ALL.into_iter().enumerate() {
            let item = gio::MenuItem::new(Some(&kind.label()), None);
            item.set_action_and_target_value(
                Some("generator-page.add-step"),
                Some(&(i as u32).to_variant()),
            );
            menu.append_item(&item);
        }
        add.set_menu_model(Some(&menu));
        self.steps_box.append(&add);
    }

    /// The notice under a step that cannot run.
    fn notice(&self, problem: &StepProblem) -> gtk::Box {
        let notice = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        notice.add_css_class("hk-notice");
        notice.add_css_class("hk-step-notice");
        notice.set_margin_start(16 + 8 + 14 + 8);
        notice.set_margin_end(32);
        let icon = gtk::Image::from_icon_name("helikodu-warning-symbolic");
        icon.set_valign(gtk::Align::Center);
        notice.append(&icon);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let (message, detail) = match problem {
            StepProblem::MissingPlaylist(name) => (
                fill(&tr("The playlist “{}” no longer exists"), &[name]),
                tr(
                    "Choose another playlist or remove this step. Until then the tracks stay as they were.",
                ),
            ),
            StepProblem::Query(message) => (
                message.clone(),
                fill(&tr("For example: {}"), &["genre:jazz rating>=4"]),
            ),
            StepProblem::Other(message) => (
                message.clone(),
                tr("Until then the tracks stay as they were."),
            ),
        };
        let first = label(&message, &["message"]);
        first.set_wrap(true);
        first.set_widget_name("step-problem");
        text.append(&first);
        let second = label(&detail, &["detail"]);
        second.set_wrap(true);
        if matches!(problem, StepProblem::Query(_)) {
            second.add_css_class("hk-mono");
        }
        text.append(&second);
        notice.append(&text);
        notice
    }

    /// The parts every row has: the grip (or room for it), the number, the
    /// kind dropdown.
    fn row_start(&self, number: usize, movable: bool) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("hk-rule-row");
        row.add_css_class("hk-step");
        if movable {
            let grip = gtk::Image::from_icon_name("helikodu-drag-handle-symbolic");
            grip.add_css_class("hk-t3");
            grip.add_css_class("hk-grip");
            grip.set_size_request(16, -1);
            grip.set_tooltip_text(Some(&tr("Drag to move the step")));
            row.append(&grip);
        } else {
            let room = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            room.set_size_request(16, -1);
            row.append(&room);
        }
        let n = label(&number.to_string(), &["number"]);
        n.set_size_request(14, -1);
        n.set_xalign(1.0);
        row.append(&n);
        row
    }

    /// The end of a row: the tracks left after it, and the remove button.
    fn row_end(self: &Rc<Self>, row: &gtk::Box, left: Option<usize>, index: Option<usize>) {
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        row.append(&spacer);
        let count = label(
            &left.map(format::count).unwrap_or_default(),
            &["hk-t3", "hk-numeric", "hk-step-count"],
        );
        count.set_xalign(1.0);
        count.set_size_request(44, -1);
        if let Some(n) = left {
            let words = fill(
                &ntr("{} track left", "{} tracks left", n),
                &[&format::count(n)],
            );
            count.set_tooltip_text(Some(&words));
            count.update_property(&[gtk::accessible::Property::Label(&words)]);
        }
        row.append(&count);
        match index {
            Some(i) => {
                let remove = gtk::Button::from_icon_name("list-remove-symbolic");
                remove.add_css_class("hk-icon");
                remove.set_tooltip_text(Some(&tr("Remove")));
                remove.update_property(&[gtk::accessible::Property::Label(&tr("Remove"))]);
                let weak = Rc::downgrade(self);
                remove.connect_clicked(move |_| {
                    if let Some(view) = weak.upgrade() {
                        view.remove_step(i);
                    }
                });
                row.append(&remove);
            }
            None => {
                let room = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                room.set_size_request(24, -1);
                row.append(&room);
            }
        }
    }

    /// The row of the source: "From", then the library, a playlist or a
    /// search. It cannot be moved or removed.
    fn source_row(self: &Rc<Self>, source: &GeneratorSource, left: Option<usize>) -> gtk::Box {
        let row = self.row_start(1, false);
        let kind = dropdown(&[tr("From")], 0, KIND_WIDTH);
        kind.update_property(&[gtk::accessible::Property::Label(&tr("Kind of step"))]);
        row.append(&kind);
        let at = match source {
            GeneratorSource::Library => 0,
            GeneratorSource::Playlist(_) => 1,
            GeneratorSource::Query(_) => 2,
        };
        let what = dropdown(
            &[tr("The library"), tr("A playlist"), tr("A search")],
            at,
            160,
        );
        what.update_property(&[gtk::accessible::Property::Label(&tr("Source"))]);
        let weak = Rc::downgrade(self);
        what.connect_selected_notify(move |d| {
            let Some(view) = weak.upgrade() else { return };
            let first = view.playlists.borrow().first().cloned().unwrap_or_default();
            let source = match d.selected() {
                1 => GeneratorSource::Playlist(first),
                2 => GeneratorSource::Query(String::new()),
                _ => GeneratorSource::Library,
            };
            view.draft.borrow_mut().source = source;
            view.steps_changed(true);
        });
        row.append(&what);
        match source {
            GeneratorSource::Library => {}
            GeneratorSource::Playlist(name) => {
                let chooser = self.playlist_dropdown(name, |view, chosen| {
                    view.draft.borrow_mut().source = GeneratorSource::Playlist(chosen);
                });
                row.append(&chooser);
            }
            GeneratorSource::Query(text) => {
                let entry = self.query_entry(text, |view, text| {
                    view.draft.borrow_mut().source = GeneratorSource::Query(text);
                });
                row.append(&entry);
            }
        }
        self.row_end(&row, left, None);
        row
    }

    fn playlist_dropdown(
        self: &Rc<Self>,
        current: &str,
        set: impl Fn(&Rc<Self>, String) + 'static,
    ) -> gtk::DropDown {
        let (names, at, placeholder) = self.playlist_choices(current);
        let chooser = dropdown(&names, at, 0);
        chooser.set_hexpand(true);
        chooser.set_halign(gtk::Align::Fill);
        chooser.update_property(&[gtk::accessible::Property::Label(&tr("Playlist"))]);
        let weak = Rc::downgrade(self);
        chooser.connect_selected_notify(move |d| {
            let Some(view) = weak.upgrade() else { return };
            let index = d.selected() as usize;
            if placeholder && index == 0 {
                return;
            }
            let Some(name) = names.get(index).cloned() else {
                return;
            };
            set(&view, name);
            view.steps_changed(true);
        });
        chooser
    }

    /// A query entry that fills the row: the query font once it holds a
    /// field operator, and a wavy line under what cannot be read.
    fn query_entry(
        self: &Rc<Self>,
        text: &str,
        set: impl Fn(&Rc<Self>, String) + 'static,
    ) -> gtk::Entry {
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.add_css_class("hk-step-query");
        entry.set_hexpand(true);
        entry.set_text(text);
        entry.update_property(&[gtk::accessible::Property::Label(&tr("Query"))]);
        crate::query_text::mark(&entry, &crate::query_text::read(text));
        let weak = Rc::downgrade(self);
        entry.connect_changed(move |entry| {
            let Some(view) = weak.upgrade() else { return };
            let text = entry.text().to_string();
            crate::query_text::mark(entry, &crate::query_text::read(&text));
            set(&view, text);
            view.steps_changed(false);
        });
        let completion = QueryCompletion::attach(&entry, &self.backend);
        self.completions
            .borrow_mut()
            .push((entry.downgrade(), Rc::downgrade(&completion)));
        entry
    }

    /// Follows the narrow window: the page's table keeps the number, the title
    /// with the artist under it, and the length.
    pub(crate) fn set_narrow(&self, narrow: bool) {
        self.view.set_narrow(narrow);
    }

    /// Types `text` into the `nth` query entry of the steps and returns what
    /// its completion lists (for tests).
    pub(crate) fn demo_step_completion(&self, nth: usize, text: &str) -> Vec<String> {
        let found = self.completions.borrow().get(nth).cloned();
        let Some((entry, completion)) = found else {
            return Vec::new();
        };
        let (Some(entry), Some(completion)) = (entry.upgrade(), completion.upgrade()) else {
            return Vec::new();
        };
        entry.grab_focus();
        entry.set_text(text);
        entry.set_position(-1);
        completion.update();
        completion.names()
    }

    fn number_entry(
        self: &Rc<Self>,
        value: usize,
        name: &str,
        set: impl Fn(&Rc<Self>, usize) + 'static,
    ) -> gtk::Entry {
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.set_width_chars(3);
        entry.set_max_width_chars(5);
        entry.set_size_request(52, -1);
        gtk::prelude::EditableExt::set_alignment(&entry, 1.0);
        entry.set_input_purpose(gtk::InputPurpose::Digits);
        entry.set_text(&value.to_string());
        entry.update_property(&[gtk::accessible::Property::Label(name)]);
        let weak = Rc::downgrade(self);
        entry.connect_changed(move |entry| {
            let Some(view) = weak.upgrade() else { return };
            match entry.text().trim().parse::<usize>() {
                Ok(n) if n > 0 => {
                    entry.remove_css_class("error");
                    set(&view, n);
                    view.steps_changed(false);
                }
                _ => entry.add_css_class("error"),
            }
        });
        entry
    }

    /// The row of the step at `index` (of the steps after the source).
    fn step_row(
        self: &Rc<Self>,
        index: usize,
        step: &GeneratorStep,
        left: Option<usize>,
    ) -> gtk::Box {
        let row = self.row_start(index + 2, true);
        let kinds: Vec<String> = StepKind::ALL.iter().map(|k| k.label()).collect();
        let at = StepKind::ALL
            .iter()
            .position(|k| *k == StepKind::of(step))
            .unwrap_or(0);
        let kind = dropdown(&kinds, at, KIND_WIDTH);
        kind.update_property(&[gtk::accessible::Property::Label(&tr("Kind of step"))]);
        let weak = Rc::downgrade(self);
        kind.connect_selected_notify(move |d| {
            let Some(view) = weak.upgrade() else { return };
            let Some(chosen) = StepKind::ALL.get(d.selected() as usize).copied() else {
                return;
            };
            let first = view.playlists.borrow().first().cloned();
            if let Some(step) = view.draft.borrow_mut().steps.get_mut(index) {
                *step = chosen.new_step(first.as_deref());
            }
            view.steps_changed(true);
        });
        row.append(&kind);
        let change = move |view: &Rc<Self>, step: GeneratorStep| {
            if let Some(slot) = view.draft.borrow_mut().steps.get_mut(index) {
                *slot = step;
            }
        };
        match step {
            GeneratorStep::Where(text) => {
                row.append(&self.query_entry(text, move |view, text| {
                    change(view, GeneratorStep::Where(text));
                }));
            }
            GeneratorStep::Exclude(name) => {
                // Translators: in the Exclude step of a generator playlist,
                // before the dropdown of playlists.
                row.append(&label(&tr("the tracks of"), &["hk-t2"]));
                row.append(&self.playlist_dropdown(name, move |view, chosen| {
                    change(view, GeneratorStep::Exclude(chosen));
                }));
            }
            GeneratorStep::Sort { by, descending } => {
                let fields: Vec<String> = SORTS.iter().map(|s| sort_label(*s)).collect();
                let at = SORTS.iter().position(|s| s == by).unwrap_or(0);
                let field = dropdown(&fields, at, 150);
                field.update_property(&[gtk::accessible::Property::Label(&tr("Sort by"))]);
                let weak = Rc::downgrade(self);
                field.connect_selected_notify(move |d| {
                    let Some(view) = weak.upgrade() else { return };
                    let Some(by) = SORTS.get(d.selected() as usize).copied() else {
                        return;
                    };
                    let descending = directions(by).first().is_some_and(|(_, d)| *d);
                    change(&view, GeneratorStep::Sort { by, descending });
                    view.steps_changed(true);
                });
                row.append(&field);
                let ways = directions(*by);
                if !ways.is_empty() {
                    let names: Vec<String> = ways.iter().map(|(n, _)| n.clone()).collect();
                    let at = ways.iter().position(|(_, d)| d == descending).unwrap_or(0);
                    let direction = dropdown(&names, at, 160);
                    direction
                        .update_property(&[gtk::accessible::Property::Label(&tr("Direction"))]);
                    let by = *by;
                    let weak = Rc::downgrade(self);
                    direction.connect_selected_notify(move |d| {
                        let Some(view) = weak.upgrade() else { return };
                        let Some((_, descending)) = ways.get(d.selected() as usize) else {
                            return;
                        };
                        let descending = *descending;
                        if let Some(slot) = view.draft.borrow_mut().steps.get_mut(index) {
                            *slot = GeneratorStep::Sort { by, descending };
                        }
                        view.steps_changed(false);
                    });
                    row.append(&direction);
                }
            }
            GeneratorStep::PerArtist(n) | GeneratorStep::PerAlbum(n) => {
                let artist = matches!(step, GeneratorStep::PerArtist(_));
                let amount = Rc::new(Cell::new(*n));
                let per_artist = Rc::new(Cell::new(artist));
                let make = {
                    let (amount, per_artist) = (amount.clone(), per_artist.clone());
                    move || {
                        if per_artist.get() {
                            GeneratorStep::PerArtist(amount.get())
                        } else {
                            GeneratorStep::PerAlbum(amount.get())
                        }
                    }
                };
                let make2 = make.clone();
                let amount2 = amount.clone();
                row.append(&self.number_entry(*n, &tr("At most"), move |view, n| {
                    amount2.set(n);
                    change(view, make2());
                }));
                // Translators: "At most [2] per [artist]" in a generator step.
                row.append(&label(&tr("per"), &["hk-t2"]));
                let what = dropdown(&[tr("artist"), tr("album")], usize::from(!artist), 100);
                what.update_property(&[gtk::accessible::Property::Label(&tr("Per"))]);
                let weak = Rc::downgrade(self);
                what.connect_selected_notify(move |d| {
                    let Some(view) = weak.upgrade() else { return };
                    per_artist.set(d.selected() == 0);
                    change(&view, make());
                    view.steps_changed(false);
                });
                row.append(&what);
            }
            GeneratorStep::Limit(_) | GeneratorStep::LimitMinutes(_) => {
                let minutes = matches!(step, GeneratorStep::LimitMinutes(_));
                let value = match step {
                    GeneratorStep::Limit(n) => *n,
                    GeneratorStep::LimitMinutes(m) => usize::try_from(*m).unwrap_or(usize::MAX),
                    _ => 0,
                };
                let amount = Rc::new(Cell::new(value));
                let in_minutes = Rc::new(Cell::new(minutes));
                let make = {
                    let (amount, in_minutes) = (amount.clone(), in_minutes.clone());
                    move || {
                        if in_minutes.get() {
                            GeneratorStep::LimitMinutes(amount.get() as u64)
                        } else {
                            GeneratorStep::Limit(amount.get())
                        }
                    }
                };
                let make2 = make.clone();
                let amount2 = amount.clone();
                row.append(&self.number_entry(value, &tr("Limit"), move |view, n| {
                    amount2.set(n);
                    change(view, make2());
                }));
                let unit = dropdown(&[tr("tracks"), tr("minutes")], usize::from(minutes), 100);
                unit.update_property(&[gtk::accessible::Property::Label(&tr("Unit"))]);
                let weak = Rc::downgrade(self);
                unit.connect_selected_notify(move |d| {
                    let Some(view) = weak.upgrade() else { return };
                    in_minutes.set(d.selected() == 1);
                    change(&view, make());
                    view.steps_changed(false);
                });
                row.append(&unit);
            }
        }
        self.row_end(&row, left, Some(index));
        self.make_movable(&row, index);
        row
    }

    /// Dragging by the grip, Alt+Up and Alt+Down, and Delete.
    fn make_movable(self: &Rc<Self>, row: &gtk::Box, index: usize) {
        row.set_focusable(true);
        let label = fill(&tr("Step {}"), &[&(index + 2).to_string()]);
        row.update_property(&[gtk::accessible::Property::Label(&label)]);
        if let Some(grip) = row.first_child() {
            let source = gtk::DragSource::new();
            source.set_actions(gdk::DragAction::MOVE);
            source.connect_prepare(move |_, _, _| {
                Some(gdk::ContentProvider::for_value(
                    &StepIndex(index as u32).to_value(),
                ))
            });
            grip.add_controller(source);
        }
        let target = gtk::DropTarget::new(StepIndex::static_type(), gdk::DragAction::MOVE);
        let weak_row = row.downgrade();
        target.connect_motion(move |_, _, y| {
            if let Some(row) = weak_row.upgrade() {
                let before = y < f64::from(row.height()) / 2.0;
                let classes = mark_classes(&row, before);
                let classes: Vec<&str> = classes.iter().map(String::as_str).collect();
                row.set_css_classes(&classes);
            }
            gdk::DragAction::MOVE
        });
        let weak_row = row.downgrade();
        target.connect_leave(move |_| {
            if let Some(row) = weak_row.upgrade() {
                row.remove_css_class("hk-drop-before");
                row.remove_css_class("hk-drop-after");
            }
        });
        let weak = Rc::downgrade(self);
        let weak_row = row.downgrade();
        target.connect_drop(move |_, value, _, y| {
            let (Some(view), Ok(dragged)) = (weak.upgrade(), value.get::<StepIndex>()) else {
                return false;
            };
            let before = weak_row
                .upgrade()
                .is_none_or(|row| y < f64::from(row.height()) / 2.0);
            let to = if before { index } else { index + 1 };
            view.move_step(dragged.0 as usize, to);
            true
        });
        row.add_controller(target);
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        let weak_row = row.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let alt = state.contains(gdk::ModifierType::ALT_MASK);
            match key {
                gdk::Key::Up if alt && index > 0 => {
                    view.move_step(index, index - 1);
                    glib::Propagation::Stop
                }
                gdk::Key::Down if alt => {
                    view.move_step(index, index + 2);
                    glib::Propagation::Stop
                }
                gdk::Key::Delete if weak_row.upgrade().is_some_and(|row| row.has_focus()) => {
                    view.remove_step(index);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        row.add_controller(keys);
    }

    /// Moves the step at `from` to before the step now at `to` (the steps'
    /// count for the end); the source stays first.
    pub(crate) fn move_step(self: &Rc<Self>, from: usize, to: usize) {
        let moved = {
            let mut draft = self.draft.borrow_mut();
            let len = draft.steps.len();
            if from >= len || to > len || to == from || to == from + 1 {
                None
            } else {
                let step = draft.steps.remove(from);
                let at = if to > from { to - 1 } else { to };
                draft.steps.insert(at, step);
                Some(at)
            }
        };
        if let Some(at) = moved {
            self.focus_step.set(Some(at));
            self.steps_changed(true);
        }
    }

    fn remove_step(self: &Rc<Self>, index: usize) {
        let removed = {
            let mut draft = self.draft.borrow_mut();
            (index < draft.steps.len()).then(|| draft.steps.remove(index))
        };
        if removed.is_some() {
            let len = self.draft.borrow().steps.len();
            if len > 0 {
                self.focus_step.set(Some(index.min(len - 1)));
            }
            self.steps_changed(true);
        }
    }

    /// Adds a step of the `n`th kind at the end and focuses it.
    pub(crate) fn add_step(self: &Rc<Self>, n: usize) {
        let Some(kind) = StepKind::ALL.get(n).copied() else {
            return;
        };
        let first = self.playlists.borrow().first().cloned();
        let index = {
            let mut draft = self.draft.borrow_mut();
            draft.steps.push(kind.new_step(first.as_deref()));
            draft.steps.len() - 1
        };
        self.focus_step.set(Some(index));
        self.steps_changed(true);
    }

    /// The selected tracks as a context menu target.
    pub(crate) fn target(&self) -> Option<Target> {
        if self.editing.get() {
            self.results.target()
        } else {
            self.view.target()
        }
    }

    /// The anchor for asking a new name: the name in the hero.
    pub(crate) fn name_label(&self) -> gtk::Label {
        self.name.clone()
    }

    /// Installs the page's own actions: a chip opens the editor; the "Add
    /// step" menu adds a step.
    pub(crate) fn install_actions(self: &Rc<Self>) {
        let group = gio::SimpleActionGroup::new();
        let edit = gio::SimpleAction::new("edit", None);
        let weak = Rc::downgrade(self);
        edit.connect_activate(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.set_editing(true);
            }
        });
        group.add_action(&edit);
        let add = gio::SimpleAction::new("add-step", Some(glib::VariantTy::UINT32));
        let weak = Rc::downgrade(self);
        add.connect_activate(move |_, value| {
            if let (Some(view), Some(n)) = (weak.upgrade(), value.and_then(u32::from_variant)) {
                view.add_step(n as usize);
            }
        });
        group.add_action(&add);
        self.root
            .insert_action_group("generator-page", Some(&group));
    }

    /// The list, for keyboard focus.
    pub(crate) fn focus_target(&self) -> gtk::Widget {
        if self.editing.get() {
            self.results.list.clone().upcast()
        } else {
            self.view.list.clone().upcast()
        }
    }

    /// Reads the tracks again after the library changed, without making the
    /// playlist again.
    pub(crate) fn reload(self: &Rc<Self>) {
        if self.making.get() {
            return;
        }
        let editing = self.editing.get();
        let Some(key) = self.key.get() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| Ok((l.generator(key)?, choices(l, key)?)))
                .await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            if let Ok((Some(info), names)) = result {
                view.playlists.replace(names);
                let steps_kept = view
                    .info
                    .borrow()
                    .as_ref()
                    .is_some_and(|i| i.steps == info.steps);
                view.info.replace(Some(info));
                view.name.set_text(&view.title());
                if steps_kept {
                    view.show_chips();
                }
                if editing {
                    view.try_now();
                }
            }
        });
    }

    /// What the page shows (for tests): the chips, the summary, the status
    /// bar, the number of tracks, and the headers.
    pub(crate) fn state(&self) -> GeneratorState {
        let mut chips = Vec::new();
        let mut child = self.chips.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(b) = widget.downcast_ref::<gtk::Button>() {
                let text = b
                    .child()
                    .and_then(|line| line.last_child())
                    .and_downcast::<gtk::Label>()
                    .map(|l| l.text().to_string())
                    .unwrap_or_default();
                chips.push((text, b.has_css_class("problem")));
            }
        }
        let mut steps = Vec::new();
        let mut child = self.steps_box.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if widget.has_css_class("hk-step") {
                let mut count = String::new();
                let mut kind = String::new();
                let mut part = widget.first_child();
                while let Some(p) = part {
                    part = p.next_sibling();
                    if kind.is_empty()
                        && let Some(d) = p.downcast_ref::<gtk::DropDown>()
                    {
                        kind = d
                            .selected_item()
                            .and_downcast::<gtk::StringObject>()
                            .map(|s| s.string().to_string())
                            .unwrap_or_default();
                    }
                    if p.has_css_class("hk-step-count")
                        && let Some(l) = p.downcast_ref::<gtk::Label>()
                    {
                        count = l.text().to_string();
                    }
                }
                steps.push((kind, count));
            }
        }
        let notice = find_named(self.steps_box.upcast_ref(), "step-problem")
            .and_downcast::<gtk::Label>()
            .map(|l| l.text().to_string());
        GeneratorState {
            chips,
            summary: self.summary.text().to_string(),
            status: self.status_left.text().to_string(),
            tracks: self.shown.borrow().len(),
            headers: self.view.header_texts(),
            artist_under: self.view.first_row_shape().0,
            editing: self.editing.get(),
            steps,
            notice,
            results: format!(
                "{} {} {}",
                self.results_count.text(),
                self.results_time.text(),
                self.results_note.text()
            ),
            empty: self.body.visible_child_name().as_deref() == Some("empty"),
            draft: self.draft.borrow().to_string(),
        }
    }
}

/// What the generator page shows, for tests.
#[derive(Debug, Clone, Default)]
pub struct GeneratorState {
    /// The chips' words, and whether each carries the warning.
    pub chips: Vec<(String, bool)>,
    /// The summary under the name.
    pub summary: String,
    /// The left of the status bar.
    pub status: String,
    /// The number of tracks in the table.
    pub tracks: usize,
    /// The table's headers.
    pub headers: Vec<String>,
    /// The artist under the title of the first row (in the narrow window).
    pub artist_under: Option<String>,
    /// Whether the steps are being edited.
    pub editing: bool,
    /// The editor's rows: the kind and the tracks left after it.
    pub steps: Vec<(String, String)>,
    /// The first line of the notice under a step that cannot run.
    pub notice: Option<String>,
    /// The head of the results pane.
    pub results: String,
    /// Whether the empty sentence shows in place of the table.
    pub empty: bool,
    /// The edited steps in their text form.
    pub draft: String,
}

/// A step dragged by its grip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, glib::Boxed)]
#[boxed_type(name = "HkGeneratorStepIndex")]
struct StepIndex(u32);

/// The classes of `row` with the insertion mark before or after it.
fn mark_classes(row: &gtk::Box, before: bool) -> Vec<String> {
    let mut classes: Vec<String> = row
        .css_classes()
        .iter()
        .map(|c| c.to_string())
        .filter(|c| c != "hk-drop-before" && c != "hk-drop-after")
        .collect();
    classes.push(
        if before {
            "hk-drop-before"
        } else {
            "hk-drop-after"
        }
        .to_owned(),
    );
    classes
}

fn find_named(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if root.widget_name() == name {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(found) = find_named(&widget, name) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

/// The id of the column of a sort field.
fn sort_id(by: GeneratorSort) -> &'static str {
    match by {
        GeneratorSort::Rating => "rating",
        GeneratorSort::Plays => "plays",
        GeneratorSort::Skips => "skips",
        GeneratorSort::LastPlayed => "last-played",
        GeneratorSort::Added => "added",
        GeneratorSort::Year => "year",
        GeneratorSort::Title => "title",
        GeneratorSort::Artist => "artist",
        GeneratorSort::Album => "album",
        GeneratorSort::Length => "length",
        GeneratorSort::Random => "random",
    }
}

/// The text of the column `id` in the row of `track` (or of `row` alone).
fn cell_text(track: Option<&GeneratorTrack>, row: &TrackRow, id: &str) -> String {
    match id {
        "title" => row.title.clone(),
        "artist" => row.artist.clone(),
        "album" => row.album_title.clone(),
        "length" => row.duration.map(format::duration).unwrap_or_default(),
        other => match (SORTS.into_iter().find(|b| sort_id(*b) == other), track) {
            (Some(by), Some(track)) if track.row.key == row.key => sort_value(by, track),
            (Some(by), _) => sort_value(
                by,
                &GeneratorTrack {
                    row: row.clone(),
                    ..GeneratorTrack::default()
                },
            ),
            _ => String::new(),
        },
    }
}

/// Whether `run` was made just now.
fn run_is_fresh(run: &GeneratorRun) -> bool {
    run.fresh && run.problem.is_none()
}

/// The names a step can choose: every playlist, smart playlist and other
/// generator playlist.
fn choices(
    library: &dyn crate::backend::LibrarySource,
    key: PlaylistKey,
) -> Result<Vec<String>, crate::backend::BackendError> {
    Ok(library
        .playlists()?
        .into_iter()
        .filter(|p| p.kind != PlaylistType::Folder && p.key != key)
        .map(|p| p.name)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_steps_that_read_back_are_stored() {
        let good: GeneratorSteps = "from library\nwhere rating>=4\nlimit 5".parse().unwrap();
        assert!(storable(&good));
        let mut bad = good.clone();
        bad.steps.push(GeneratorStep::Where("(unclosed".into()));
        assert!(!storable(&bad));
        let mut empty = good;
        empty.steps.push(GeneratorStep::Where(String::new()));
        assert!(!storable(&empty));
    }
}
