// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Labels in the window: the Labels page (`win.show-labels`, from the Library
//! menu and the sidebar row), "Labels…" of the context menus
//! (`win.sel-labels`), which opens the Labels popover for what the menu was
//! opened on, and the changes of labels with their Undo toast.
//!
//! Label changes are undone by the Undo of their toast, and by Ctrl+Z while that
//! toast is shown, as the other changes of library data are (a playlist edit, a
//! removal from the queue). They do not join the history of tag edits and file
//! runs that Ctrl+Z steps through otherwise: that history is of changes to the
//! files, kept across runs, while a label lives in the library only, and the
//! design gives label changes one toast that covers everything done while the
//! Labels popover was open.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use super::{MainWindow, Page};
use crate::backend::{LabelDone, LabelEdit, LabelEditError, LabelFor, LabelKey, LabelUndo};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::views::actions::Target;
use crate::views::labels::LabelsView;

/// The name of the page in the view stack, and of its sidebar row.
pub(crate) const PAGE: &str = "labels";

/// What a change of labels was made for, as its toast names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    /// This many tracks.
    Tracks(usize),
    /// An album or an artist, by name.
    Named(String),
}

impl Scope {
    /// The section label of the Labels popover: "3 tracks", "Album: Tidewater
    /// Rooms", "Artist: Mari Kask".
    pub(crate) fn heading(&self, target: &LabelFor) -> String {
        match (self, target) {
            (Self::Tracks(n), _) => fill(&ntr("{} track", "{} tracks", *n), &[&format::count(*n)]),
            (Self::Named(name), LabelFor::Album(_)) => fill(&tr("Album: {}"), &[name]),
            (Self::Named(name), _) => fill(&tr("Artist: {}"), &[name]),
        }
    }
}

/// The text of the toast after `edit`.
pub(crate) fn change_text(edit: &LabelEdit, scope: &Scope) -> String {
    match (edit, scope) {
        (LabelEdit::Give { name, .. }, Scope::Tracks(n)) => fill(
            // Translators: {0} is a label, {1} the number of tracks.
            &ntr("Gave “{}” to {} track", "Gave “{}” to {} tracks", *n),
            &[name, &format::count(*n)],
        ),
        (LabelEdit::Give { name, .. }, Scope::Named(item)) => {
            // Translators: {0} is a label, {1} an album or an artist.
            fill(&tr("Gave “{}” to {}"), &[name, item])
        }
        (LabelEdit::Take { name, .. }, Scope::Tracks(n)) => fill(
            // Translators: {0} is a label, {1} the number of tracks.
            &ntr("Took “{}” from {} track", "Took “{}” from {} tracks", *n),
            &[name, &format::count(*n)],
        ),
        (LabelEdit::Take { name, .. }, Scope::Named(item)) => {
            // Translators: {0} is a label, {1} an album or an artist.
            fill(&tr("Took “{}” from {}"), &[name, item])
        }
        (LabelEdit::Rename { name, new_name }, _) => {
            fill(&tr("Renamed “{}” to “{}”"), &[name, new_name])
        }
        (LabelEdit::Merge { from, into }, _) => fill(&tr("Merged “{}” into “{}”"), &[from, into]),
        (LabelEdit::Delete { name }, _) => fill(&tr("Removed “{}” from everything"), &[name]),
    }
}

/// The undo of changes that one toast takes back; it grows while the Labels
/// popover stays open, and is emptied when it is used.
pub(crate) type SharedUndo = Rc<RefCell<LabelUndo>>;

impl MainWindow {
    pub(super) fn setup_labels(&self) {
        let show = gio::ActionEntry::builder("show-labels")
            .activate(|window: &Self, _, _| window.go(Page::Library(PAGE), true))
            .build();
        let popover = gio::ActionEntry::builder("sel-labels")
            .activate(|window: &Self, _, _| {
                if let Some(target) = window.take_target() {
                    window.labels_for_target(target);
                }
            })
            .build();
        self.add_action_entries([show, popover]);
    }

    /// The Labels page, built when it is first wanted.
    fn labels_view(&self) -> Option<Rc<LabelsView>> {
        if let Some(view) = self.imp().labels_view.borrow().clone() {
            return Some(view);
        }
        let pages = self.pages()?;
        let view = LabelsView::new(self, &pages.backend, &pages.covers, &pages.playback);
        self.imp().views.add_named(&view.root, Some(PAGE));
        self.imp().labels_view.replace(Some(view.clone()));
        Some(view)
    }

    /// Shows the Labels page with `label` chosen (or the one chosen before).
    pub(super) fn show_labels(&self, label: Option<LabelKey>) -> String {
        if let Some(view) = self.labels_view() {
            view.show(label);
        }
        self.imp().views.set_visible_child_name(PAGE);
        PAGE.into()
    }

    pub(super) fn labels_focus(&self) -> Option<gtk::Widget> {
        self.labels_view().map(|v| v.focus_target())
    }

    /// Opens the label `key` in the Labels view.
    pub(crate) fn open_label(&self, key: LabelKey) {
        self.go(Page::Label(key), true);
    }

    /// Notes where a context menu opened, so that "Labels…" opens its popover
    /// there.
    pub(crate) fn set_menu_anchor(&self, widget: &gtk::Widget, at: (f64, f64)) {
        self.imp()
            .menu_anchor
            .replace(Some((widget.downgrade(), at.0, at.1)));
    }

    /// Opens the Labels popover for what the context menu was opened on.
    fn labels_for_target(&self, target: Target) {
        let Some(backend) = self.backend() else {
            return;
        };
        let anchor = self.imp().menu_anchor.borrow().clone();
        let (widget, at) = match anchor.and_then(|(w, x, y)| w.upgrade().map(|w| (w, (x, y)))) {
            Some((widget, at)) if widget.root().is_some() => (widget, Some(at)),
            // Near the top of the content when the menu's place has gone.
            _ => (self.clone().upcast(), Some(self.content_point())),
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let (label_for, scope) = match &target {
                Target::Tracks { .. } => {
                    let keys = target.keys(&backend).await;
                    let n = keys.len();
                    (LabelFor::Tracks(keys), Scope::Tracks(n))
                }
                Target::Album(key) => {
                    let key = *key;
                    let title = backend
                        .query(move |l| l.album(key))
                        .await
                        .ok()
                        .flatten()
                        .map(|d| d.row.title)
                        .unwrap_or_default();
                    (LabelFor::Album(key), Scope::Named(title))
                }
                Target::Artist(key) => {
                    let key = *key;
                    let name = backend
                        .query(move |l| l.artist(key))
                        .await
                        .ok()
                        .flatten()
                        .map(|d| d.row.name)
                        .unwrap_or_default();
                    (LabelFor::Artist(key), Scope::Named(name))
                }
                Target::Genre(_) => return,
            };
            if let Some(window) = weak.upgrade() {
                window.open_labels_popover(label_for, scope, &widget, at);
            }
        });
    }

    /// A point near the top of the content area, in the window's coordinates:
    /// where a popover opens that has no widget of its own to open from. (A
    /// popover is never put on the stack of pages, which cannot let go of a
    /// child that is not one of its pages.)
    fn content_point(&self) -> (f64, f64) {
        let views = self.imp().views.get();
        views
            .compute_point(self, &gtk::graphene::Point::new(320.0, 80.0))
            .map_or((560.0, 126.0), |p| (f64::from(p.x()), f64::from(p.y())))
    }

    /// Opens the Labels popover for `target` on `anchor`, at `at` in its
    /// coordinates or under it.
    pub(crate) fn open_labels_popover(
        &self,
        target: LabelFor,
        scope: Scope,
        anchor: &gtk::Widget,
        at: Option<(f64, f64)>,
    ) {
        crate::popovers::labels::open(self, target, scope, anchor, at);
    }

    /// Makes `edit`; `done` gets the outcome. The views that show labels follow
    /// a change.
    pub(crate) fn change_labels(
        &self,
        edit: LabelEdit,
        done: impl FnOnce(&MainWindow, Result<LabelDone, LabelEditError>) + 'static,
    ) {
        let Some(backend) = self.backend() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |l| Ok(l.change_label(&edit)))
                .await
                .unwrap_or_else(|e| Err(LabelEditError::Failed(e)));
            let Some(window) = weak.upgrade() else { return };
            if result.is_ok() {
                window.labels_changed();
            }
            done(&window, result);
        });
    }

    /// Makes `edit` and says so in a toast whose Undo takes it back.
    pub(crate) fn change_labels_with_undo(&self, edit: LabelEdit, scope: Scope) {
        let text = change_text(&edit, &scope);
        self.change_labels(edit, move |window, result| match result {
            Ok(done) => window.toast_label_undo(&text, Rc::new(RefCell::new(done.undo)), None),
            Err(error) => window.label_problem(&error),
        });
    }

    /// Tells the user why a change of labels was not made.
    pub(crate) fn label_problem(&self, error: &LabelEditError) {
        let text = match error {
            LabelEditError::Exists(name) => {
                fill(&tr("There is already a label “{}”."), &[name.as_str()])
            }
            LabelEditError::Failed(error) => error.0.clone(),
        };
        self.toast(&text, true);
    }

    /// Shows `text` in a toast whose Undo puts back what `undo` holds then, and
    /// calls `after` once it has.
    pub(crate) fn toast_label_undo(
        &self,
        text: &str,
        undo: SharedUndo,
        after: Option<Rc<dyn Fn()>>,
    ) {
        let weak = self.downgrade();
        self.toast_undo(text, move || {
            let steps = undo.replace(LabelUndo::default());
            let Some(window) = weak.upgrade() else { return };
            if steps.is_empty() {
                return;
            }
            let after = after.clone();
            window.undo_label_changes(steps, move || {
                if let Some(after) = after {
                    after();
                }
            });
        });
    }

    /// Puts the labels back as they were before the changes of `undo`.
    pub(crate) fn undo_label_changes(&self, undo: LabelUndo, then: impl FnOnce() + 'static) {
        let Some(backend) = self.backend() else {
            return;
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = backend.query(move |l| l.undo_labels(&undo)).await;
            let Some(window) = weak.upgrade() else { return };
            match result {
                Ok(()) => {
                    window.labels_changed();
                    then();
                }
                Err(error) => window.toast(&error.0, true),
            }
        });
    }

    /// Reads the labels again wherever they show: the sidebar's count, the
    /// Labels view, the chips of the album and artist pages.
    pub(crate) fn labels_changed(&self) {
        self.load_counts();
        if let Some(view) = self.imp().labels_view.borrow().clone() {
            view.reload();
        }
        crate::views::label_chips::reload_all();
    }

    /// The labels listed in the Labels view, with their track counts (for
    /// tests).
    #[doc(hidden)]
    pub fn label_list(&self) -> Vec<(String, usize)> {
        self.labels_view().map(|v| v.listed()).unwrap_or_default()
    }

    /// The name of the label the Labels view shows, and the texts of its
    /// detail (for tests).
    #[doc(hidden)]
    pub fn label_detail_texts(&self) -> Vec<String> {
        self.labels_view()
            .map(|v| v.detail_texts())
            .unwrap_or_default()
    }

    /// Whether the Labels view shows its empty state (for tests).
    #[doc(hidden)]
    pub fn labels_empty_shown(&self) -> bool {
        self.labels_view().is_some_and(|v| v.empty_shown())
    }

    /// Chooses the label at `index` of the Labels view's list (for tests).
    #[doc(hidden)]
    pub fn choose_label(&self, index: usize) {
        if let Some(view) = self.labels_view() {
            view.choose(index);
        }
    }

    /// Opens the Labels popover for the tracks `keys`, under the content
    /// header (for tests and screenshots).
    #[doc(hidden)]
    pub fn demo_labels_popover(&self, keys: Vec<crate::backend::TrackKey>) {
        let n = keys.len();
        let anchor: gtk::Widget = self.clone().upcast();
        crate::popovers::labels::open_with(
            self,
            LabelFor::Tracks(keys),
            Scope::Tracks(n),
            &anchor,
            Some(self.content_point()),
            false,
        );
    }

    /// Opens the Labels popover for an album, under the content header (for
    /// tests).
    #[doc(hidden)]
    pub fn demo_album_labels_popover(&self, album: crate::backend::AlbumKey, title: &str) {
        let anchor: gtk::Widget = self.clone().upcast();
        crate::popovers::labels::open_with(
            self,
            LabelFor::Album(album),
            Scope::Named(title.to_owned()),
            &anchor,
            Some(self.content_point()),
            false,
        );
    }

    /// The texts of the label chips on screen, "Add label" last (for tests).
    #[doc(hidden)]
    pub fn label_chip_texts(&self) -> Vec<String> {
        crate::views::label_chips::shown()
            .first()
            .map(|c| c.texts())
            .unwrap_or_default()
    }

    /// Clicks the label chip on screen that says `text` (for tests).
    #[doc(hidden)]
    pub fn click_label_chip(&self, text: &str) -> bool {
        crate::views::label_chips::shown()
            .iter()
            .any(|c| c.click(text))
    }

    /// Runs `name` (rename, merge or delete) of the menu of the label at
    /// `index` of the Labels view (for tests).
    #[doc(hidden)]
    pub fn label_action(&self, name: &str, index: usize) {
        if let Some(view) = self.labels_view() {
            view.act(name, index);
        }
    }

    /// The entry of a rename in place in the Labels view (for tests).
    #[doc(hidden)]
    pub fn label_rename_entry(&self) -> Option<gtk::Entry> {
        self.labels_view().and_then(|v| v.rename_entry())
    }

    /// The Labels popover open now (for tests and screenshots).
    #[doc(hidden)]
    pub fn labels_popover(&self) -> Option<gtk::Popover> {
        crate::popovers::labels::current().map(|p| p.widget())
    }

    /// The rows of the Labels popover: the name, the text at the right, the
    /// state and whether the row can be changed (for tests).
    #[doc(hidden)]
    pub fn labels_popover_rows(
        &self,
    ) -> Vec<(String, String, Option<crate::backend::LabelState>, bool)> {
        crate::popovers::labels::current()
            .map(|p| p.rows())
            .unwrap_or_default()
    }

    /// The check boxes of the Labels popover: ticked, mixed (for tests).
    #[doc(hidden)]
    pub fn labels_popover_checks(&self) -> Vec<(bool, bool)> {
        crate::popovers::labels::current()
            .map(|p| p.checks())
            .unwrap_or_default()
    }

    /// Types into the Labels popover's entry, and presses Enter when `enter`
    /// (for tests).
    #[doc(hidden)]
    pub fn labels_popover_type(&self, text: &str, enter: bool) {
        if let Some(popover) = crate::popovers::labels::current() {
            popover.type_text(text);
            if enter {
                popover.press_enter();
            }
        }
    }

    /// Clicks the row of the Labels popover that starts with `text` (for
    /// tests).
    #[doc(hidden)]
    pub fn labels_popover_click(&self, text: &str) -> bool {
        crate::popovers::labels::current().is_some_and(|p| p.click(text))
    }

    /// Whether the Labels popover's entry has the focus (for tests).
    #[doc(hidden)]
    pub fn labels_popover_entry_focused(&self) -> bool {
        crate::popovers::labels::current().is_some_and(|p| p.entry_focused())
    }
}
