// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The "Replaced" section of Missing files: missing tracks whose recording is
//! in the library again as another file (the same title and artist, lengths
//! within 2 seconds), with what would move to the new file; the new file is
//! chosen among several where there are more. "Carry to N tracks" moves the
//! plays, skips, history and places in playlists of the ticked rows as one
//! run, which the Import window lists as "Replaced files" and which is undone
//! like an import: by its toast's Undo (not by Ctrl+Z) or by "Undo this
//! import".
//!
//! A single click selects a row, Space ticks it, and a double click or Enter
//! plays its new file.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::file_tools::run_in_background;
use super::tool_window::{CHECK, GAP, footer, primary_button, table_header};
use crate::backend::interchange::{InterchangeServices, ReplacedTrack};
use crate::backend::{Backend, PlayerRequest, TrackKey};
use crate::dialogs::common::{compact_button, tilde};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The width of the "What moves" column.
const MOVES: i32 = 236;

/// The section's page and its state.
pub(super) struct Replaced {
    pub(super) root: gtk::Box,
    main: glib::WeakRef<MainWindow>,
    backend: Backend,
    services: Arc<dyn InterchangeServices>,
    tracks: RefCell<Vec<ReplacedTrack>>,
    /// The rows listed, by index into `tracks`.
    shown: RefCell<Vec<usize>>,
    /// The ticked rows; new rows are ticked.
    unticked: RefCell<HashSet<TrackKey>>,
    /// The chosen new file of a row with several, by index.
    choices: RefCell<HashMap<TrackKey, usize>>,
    filling: Cell<bool>,
    working: Cell<bool>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    filter: gtk::SearchEntry,
    stack: gtk::Stack,
    empty: gtk::Label,
    carry: gtk::Button,
    again: gtk::Button,
    /// Told the number of rows after each load.
    counted: Box<dyn Fn(usize)>,
}

fn children(widget: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.as_ref().first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        out.push(c);
    }
    out
}

fn set_label(widget: &gtk::Widget, text: &str) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        label.set_text(text);
    }
}

/// A rating in half stars as five stars: the filled ones, then the rest.
fn stars(half_stars: u8) -> (String, String) {
    let filled = usize::from(half_stars.div_ceil(2).min(5));
    ("★".repeat(filled), "★".repeat(5 - filled))
}

impl Replaced {
    /// The header's summary: how many missing tracks have a new file.
    pub(super) fn summary(&self) -> String {
        let count = self.tracks.borrow().len();
        fill(
            &ntr(
                "{} missing track has a new file of the same recording",
                "{} missing tracks have a new file of the same recording",
                count,
            ),
            &[&format::count(count)],
        )
    }

    /// Reads the replaced tracks again.
    pub(super) fn load(self: &Rc<Self>) {
        let services = self.services.clone();
        let weak = Rc::downgrade(self);
        run_in_background(
            move |_| services.replaced_tracks(),
            |_, _| {},
            move |tracks| {
                let Some(this) = weak.upgrade() else { return };
                match tracks {
                    Ok(tracks) => {
                        this.tracks.replace(tracks);
                    }
                    Err(error) => this.empty.set_text(&error.0),
                }
                this.rebuild();
                (this.counted)(this.tracks.borrow().len());
            },
        );
    }

    fn rebuild(&self) {
        let text = self.filter.text().trim().to_lowercase();
        let shown: Vec<usize> = self
            .tracks
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                text.is_empty()
                    || [t.title.as_deref(), t.artist.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|s| s.to_lowercase().contains(&text))
                    || t.old_path.to_string_lossy().to_lowercase().contains(&text)
                    || t.new
                        .iter()
                        .any(|(_, p)| p.to_string_lossy().to_lowercase().contains(&text))
            })
            .map(|(i, _)| i)
            .collect();
        let count = shown.len();
        self.shown.replace(shown);
        let items: Vec<glib::BoxedAnyObject> = (0..count).map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &items);
        self.empty.set_text(&if self.tracks.borrow().is_empty() {
            tr("No missing track has a new file of the same recording.")
        } else {
            tr("No track matches the filter.")
        });
        self.stack
            .set_visible_child_name(if count == 0 { "empty" } else { "table" });
        self.update_footer();
    }

    /// The ticked rows with their chosen new tracks.
    fn pairs(&self) -> Vec<(TrackKey, TrackKey)> {
        let unticked = self.unticked.borrow();
        let choices = self.choices.borrow();
        self.tracks
            .borrow()
            .iter()
            .filter(|t| !unticked.contains(&t.old))
            .filter_map(|t| {
                let choice = choices.get(&t.old).copied().unwrap_or(0);
                t.new.get(choice).map(|(new, _)| (t.old, *new))
            })
            .collect()
    }

    fn update_footer(&self) {
        let ticked = self.pairs().len();
        self.carry.set_label(&fill(
            &ntr("Carry to {} track", "Carry to {} tracks", ticked),
            &[&format::count(ticked)],
        ));
        self.carry.set_sensitive(ticked > 0 && !self.working.get());
        self.again.set_sensitive(!self.working.get());
    }

    fn row(&self, position: u32) -> Option<ReplacedTrack> {
        let index = *self.shown.borrow().get(position as usize)?;
        self.tracks.borrow().get(index).cloned()
    }

    fn toggle_selected(&self) -> bool {
        let position = self.selection.selected();
        let Some(track) = self.row(position) else {
            return false;
        };
        let on = self.unticked.borrow().contains(&track.old);
        if on {
            self.unticked.borrow_mut().remove(&track.old);
        } else {
            self.unticked.borrow_mut().insert(track.old);
        }
        self.store.items_changed(position, 1, 1);
        self.update_footer();
        true
    }

    fn play(&self, position: u32) {
        let Some(track) = self.row(position) else {
            return;
        };
        let choice = self.choices.borrow().get(&track.old).copied().unwrap_or(0);
        let Some((new, _)) = track.new.get(choice) else {
            return;
        };
        self.backend.send(PlayerRequest::PlayTracks {
            tracks: vec![*new],
            start: 0,
            title: tr("Missing files"),
            shuffle: false,
        });
    }

    /// "Carry to N tracks".
    fn carry(self: &Rc<Self>) {
        let pairs = self.pairs();
        if pairs.is_empty() || self.working.get() {
            return;
        }
        self.working.set(true);
        self.update_footer();
        let services = self.services.clone();
        let weak = Rc::downgrade(self);
        let count = pairs.len();
        run_in_background(
            move |_| services.carry_stats(&pairs),
            |_, _| {},
            move |result| {
                let Some(this) = weak.upgrade() else { return };
                this.working.set(false);
                match result {
                    Ok(run) => {
                        if let Some(main) = this.main.upgrade() {
                            main.playlists_changed(None);
                            super::import::refresh();
                            let weak_main = main.downgrade();
                            let services = this.services.clone();
                            let again = Rc::downgrade(&this);
                            main.toast_led(
                                &fill(
                                    &ntr(
                                        "Carried plays and history to {} track",
                                        "Carried plays and history to {} tracks",
                                        count,
                                    ),
                                    &[&format::count(count)],
                                ),
                                "helikodu-check-symbolic",
                                &tr("Undo"),
                                true,
                                move || {
                                    let again = again.clone();
                                    super::import::undo_run(
                                        &weak_main,
                                        services.clone(),
                                        run,
                                        &tr("The plays and history went back to the missing tracks."),
                                        move || {
                                            if let Some(this) = again.upgrade() {
                                                this.load();
                                            }
                                        },
                                    );
                                },
                            );
                        }
                    }
                    Err(error) => {
                        if let Some(main) = this.main.upgrade() {
                            main.toast(&error.0, true);
                        }
                    }
                }
                this.load();
            },
        );
    }

    fn bind(&self, row: &gtk::Box, item: &gtk::ListItem, position: u32) {
        let Some(track) = self.row(position) else {
            return;
        };
        let cells = children(row);
        let [check_cell, files, moves] = cells.as_slice() else {
            return;
        };
        self.filling.set(true);
        let key = track.old.0;
        if let Some(check) = check_cell.first_child().and_downcast::<gtk::CheckButton>() {
            check.set_widget_name(&format!("carry-{key}"));
            check.set_active(!self.unticked.borrow().contains(&track.old));
        }
        let title = track.title.clone().unwrap_or_else(|| {
            track
                .old_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        let lines = children(files);
        if let [top, old, new] = lines.as_slice() {
            if let [t, a] = children(top).as_slice() {
                set_label(t, &title);
                set_label(
                    a,
                    &track
                        .artist
                        .as_ref()
                        .map(|artist| fill(&tr("· {}"), &[artist]))
                        .unwrap_or_default(),
                );
            }
            let old_text = tilde(&track.old_path);
            set_label(old, &old_text);
            old.set_tooltip_text(Some(&old_text));
            if let [single, choice, of] = children(new).as_slice() {
                let several = track.new.len() > 1;
                single.set_visible(!several);
                choice.set_visible(several);
                of.set_visible(several);
                let chosen = self.choices.borrow().get(&track.old).copied().unwrap_or(0);
                if several {
                    if let Some(dropdown) = choice.downcast_ref::<gtk::DropDown>() {
                        let names: Vec<String> = track.new.iter().map(|(_, p)| tilde(p)).collect();
                        let names: Vec<&str> = names.iter().map(String::as_str).collect();
                        dropdown.set_model(Some(&gtk::StringList::new(&names)));
                        dropdown.set_widget_name(&format!("carry-choice-{key}"));
                        dropdown.set_selected(u32::try_from(chosen).unwrap_or(0));
                    }
                    set_label(
                        of,
                        &fill(
                            &tr("{} of {} possible files"),
                            &[&format::count(chosen + 1), &format::count(track.new.len())],
                        ),
                    );
                } else if let Some((_, path)) = track.new.first() {
                    let text = tilde(path);
                    set_label(single, &text);
                    single.set_tooltip_text(Some(&text));
                }
            }
        }
        if let [first, second] = children(moves).as_slice() {
            if let [plays, filled, empty] = children(first).as_slice() {
                set_label(
                    plays,
                    &fill(
                        &ntr(
                            "{} play",
                            "{} plays",
                            usize::try_from(track.plays).unwrap_or(0),
                        ),
                        &[&format::count(track.plays)],
                    ),
                );
                let (on, off) = track.rating.map(stars).unwrap_or_default();
                set_label(filled, &on);
                set_label(empty, &off);
                filled.set_visible(track.rating.is_some());
                empty.set_visible(track.rating.is_some());
            }
            let history = fill(
                &ntr("{} history entry", "{} history entries", track.history),
                &[&format::count(track.history)],
            );
            set_label(
                second,
                &if track.playlists > 0 {
                    fill(
                        &tr("{} · {}"),
                        &[
                            &history,
                            &fill(
                                &ntr("{} playlist", "{} playlists", track.playlists),
                                &[&format::count(track.playlists)],
                            ),
                        ],
                    )
                } else {
                    history
                },
            );
        }
        self.filling.set(false);
        let rating = track
            .rating
            .map(|r| {
                fill(
                    &ntr(
                        "rated {} star",
                        "rated {} stars",
                        usize::from(r.div_ceil(2)),
                    ),
                    &[&format::count(r.div_ceil(2))],
                )
            })
            .unwrap_or_default();
        item.set_accessible_label(&fill(
            // Translators: the name a screen reader gives a replaced track: {0}
            // its title, {1} its plays, {2} its rating or nothing.
            &ntr(
                "{}, {} play, {}",
                "{}, {} plays, {}",
                usize::try_from(track.plays).unwrap_or(0),
            ),
            &[&title, &format::count(track.plays), &rating],
        ));
    }
}

fn row_widget(replaced: &std::rc::Weak<Replaced>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    row.add_css_class("hk-replaced-row");
    let check_cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    check_cell.set_size_request(CHECK - GAP, -1);
    let check = gtk::CheckButton::new();
    check.set_valign(gtk::Align::Center);
    check.update_property(&[gtk::accessible::Property::Label(&tr(
        "Carry the statistics of this track",
    ))]);
    let weak = replaced.clone();
    check.connect_toggled(move |check| {
        let Some(this) = weak.upgrade() else { return };
        if this.filling.get() {
            return;
        }
        let Some(key) = check
            .widget_name()
            .strip_prefix("carry-")
            .and_then(|n| n.parse::<u64>().ok())
            .map(TrackKey)
        else {
            return;
        };
        if check.is_active() {
            this.unticked.borrow_mut().remove(&key);
        } else {
            this.unticked.borrow_mut().insert(key);
        }
        this.update_footer();
    });
    check_cell.append(&check);
    row.append(&check_cell);

    let files = gtk::Box::new(gtk::Orientation::Vertical, 2);
    files.set_hexpand(true);
    files.set_valign(gtk::Align::Center);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    top.append(&label("", &["hk-medium"]));
    top.append(&label("", &["hk-t3"]));
    files.append(&top);
    let old = label("", &["hk-mono", "hk-replaced-old", "hk-t3"]);
    old.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    old.set_max_width_chars(1);
    files.append(&old);
    let new = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let single = label("", &["hk-mono", "hk-replaced-new"]);
    single.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    single.set_max_width_chars(1);
    single.set_hexpand(true);
    new.append(&single);
    let choice = gtk::DropDown::from_strings(&[]);
    choice.add_css_class("hk-replaced-choice");
    choice.update_property(&[gtk::accessible::Property::Label(&tr("The new file"))]);
    let weak = replaced.clone();
    choice.connect_selected_notify(move |dropdown| {
        let Some(this) = weak.upgrade() else { return };
        if this.filling.get() {
            return;
        }
        let Some(key) = dropdown
            .widget_name()
            .strip_prefix("carry-choice-")
            .and_then(|n| n.parse::<u64>().ok())
            .map(TrackKey)
        else {
            return;
        };
        let chosen = dropdown.selected() as usize;
        this.choices.borrow_mut().insert(key, chosen);
        let total = this
            .tracks
            .borrow()
            .iter()
            .find(|t| t.old == key)
            .map_or(0, |t| t.new.len());
        if let Some(of) = dropdown.next_sibling().and_downcast::<gtk::Label>() {
            of.set_text(&fill(
                &tr("{} of {} possible files"),
                &[&format::count(chosen + 1), &format::count(total)],
            ));
        }
    });
    new.append(&choice);
    let of = label("", &["hk-t3"]);
    new.append(&of);
    files.append(&new);
    row.append(&files);

    let moves = gtk::Box::new(gtk::Orientation::Vertical, 2);
    moves.set_size_request(MOVES, -1);
    moves.set_hexpand(false);
    moves.set_valign(gtk::Align::Center);
    let first = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let plays = label("", &["hk-medium"]);
    plays.set_margin_end(6);
    first.append(&plays);
    for classes in [
        ["hk-replaced-stars", "hk-t2"],
        ["hk-replaced-stars", "hk-t3"],
    ] {
        let stars = label("", &classes);
        stars.set_ellipsize(gtk::pango::EllipsizeMode::None);
        first.append(&stars);
    }
    moves.append(&first);
    moves.append(&label("", &["hk-t3", "hk-secondary"]));
    row.append(&moves);
    row
}

/// The page of the section in `window`; `counted` is told the number of
/// replaced tracks after each load.
pub(super) fn new(
    main: &MainWindow,
    backend: Backend,
    counted: impl Fn(usize) + 'static,
) -> Rc<Replaced> {
    let services = backend.services().interchange();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_widget_name("replaced-page");
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    toolbar.add_css_class("hk-toolbar");
    toolbar.add_css_class("hk-tool-toolbar");
    let filter = gtk::SearchEntry::new();
    filter.add_css_class("hk-input");
    filter.set_hexpand(true);
    filter.set_valign(gtk::Align::Center);
    filter.set_placeholder_text(Some(&tr("Filter by title, artist or folder")));
    filter.update_property(&[gtk::accessible::Property::Label(&tr(
        "Filter the replaced tracks",
    ))]);
    toolbar.append(&filter);
    let again = compact_button(&tr("Look again"));
    again.set_widget_name("replaced-again");
    toolbar.append(&again);
    root.append(&toolbar);
    root.append(&table_header(&[
        (String::new(), CHECK - GAP, false),
        (tr("Track, its old file and its new file"), -1, false),
        (tr("What moves"), MOVES, false),
    ]));

    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let factory = gtk::SignalListItemFactory::new();
    let view = gtk::ListView::new(Some(selection.clone()), Some(factory.clone()));
    view.set_tab_behavior(gtk::ListTabBehavior::Item);
    view.add_css_class("hk-dup-list");
    view.add_css_class("hk-organize-list");
    view.set_widget_name("replaced-table");
    view.update_property(&[gtk::accessible::Property::Label(&tr("Replaced tracks"))]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&view));
    let empty = label("", &["hk-t3", "hk-tool-empty"]);
    empty.set_widget_name("replaced-empty");
    empty.set_xalign(0.5);
    empty.set_valign(gtk::Align::Start);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&scroller, Some("table"));
    stack.add_named(&empty, Some("empty"));
    root.append(&stack);
    let (bar, summary) = footer();
    summary.set_wrap(true);
    summary.set_single_line_mode(false);
    summary.set_ellipsize(gtk::pango::EllipsizeMode::None);
    summary.set_text(&tr(
        "Plays, skips, history and places in playlists move to the new file, and the counts add up. The new file keeps its own rating if it has one.",
    ));
    let carry = primary_button(&tr("Carry"));
    carry.set_widget_name("replaced-carry");
    bar.append(&carry);
    root.append(&bar);

    let replaced = Rc::new(Replaced {
        root,
        main: main.downgrade(),
        backend,
        services,
        tracks: RefCell::new(Vec::new()),
        shown: RefCell::new(Vec::new()),
        unticked: RefCell::new(HashSet::new()),
        choices: RefCell::new(HashMap::new()),
        filling: Cell::new(false),
        working: Cell::new(false),
        store,
        selection,
        filter: filter.clone(),
        stack,
        empty,
        carry: carry.clone(),
        again: again.clone(),
        counted: Box::new(counted),
    });
    let weak = Rc::downgrade(&replaced);
    factory.connect_setup(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&row_widget(&weak)));
        }
    });
    let weak = Rc::downgrade(&replaced);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(this), Some(row)) = (weak.upgrade(), item.child().and_downcast::<gtk::Box>())
        else {
            return;
        };
        this.bind(&row, item, item.position());
    });
    let weak = Rc::downgrade(&replaced);
    filter.connect_search_changed(move |_| {
        if let Some(this) = weak.upgrade() {
            this.rebuild();
        }
    });
    let weak = Rc::downgrade(&replaced);
    again.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.load();
        }
    });
    let weak = Rc::downgrade(&replaced);
    carry.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.carry();
        }
    });
    let weak = Rc::downgrade(&replaced);
    view.connect_activate(move |_, position| {
        if let Some(this) = weak.upgrade() {
            this.play(position);
        }
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&replaced);
    keys.connect_key_pressed(move |_, key, _, state| {
        if key == gdk::Key::space
            && state.is_empty()
            && let Some(this) = weak.upgrade()
            && this.toggle_selected()
        {
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    view.add_controller(keys);
    replaced.update_footer();
    replaced
}
