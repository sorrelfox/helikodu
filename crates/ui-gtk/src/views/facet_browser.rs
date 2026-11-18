// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The facet browser above the track table: three linked panes, genre, album
//! artist and album at first. Each pane's field can be changed from its head.
//!
//! A choice narrows the panes to its right and the table; Ctrl and Shift choose
//! several values. The filter text of the table filters first. Every pane is
//! filled by one query in the backend; nothing is filtered here.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::{Backend, FacetChoice, FacetField, FacetKey, FacetList, TrackQuery};
use crate::format;
use crate::i18n::tr;
use crate::widgets::label;

/// The fields of the three panes when the browser opens.
const DEFAULT_FIELDS: [FacetField; 3] = [
    FacetField::Genre,
    FacetField::AlbumArtist,
    FacetField::Album,
];

/// A machine name of a field, for the menu's action targets.
fn field_id(field: FacetField) -> &'static str {
    match field {
        FacetField::Genre => "genre",
        FacetField::AlbumArtist => "album-artist",
        FacetField::Album => "album",
        FacetField::Artist => "artist",
        FacetField::Composer => "composer",
        FacetField::Year => "year",
        FacetField::Format => "format",
    }
}

fn field_from_id(id: &str) -> Option<FacetField> {
    FacetField::ALL.into_iter().find(|f| field_id(*f) == id)
}

/// The name of a field in a pane head and its menu.
pub(crate) fn field_title(field: FacetField) -> String {
    match field {
        FacetField::Genre => tr("Genre"),
        FacetField::AlbumArtist => tr("Album artist"),
        FacetField::Album => tr("Album"),
        FacetField::Artist => tr("Artist"),
        FacetField::Composer => tr("Composer"),
        FacetField::Year => tr("Year"),
        FacetField::Format => tr("Format"),
    }
}

/// The first row of a pane.
fn all_title(field: FacetField) -> String {
    match field {
        FacetField::Genre => tr("All genres"),
        FacetField::AlbumArtist => tr("All album artists"),
        FacetField::Album => tr("All albums"),
        FacetField::Artist => tr("All artists"),
        FacetField::Composer => tr("All composers"),
        FacetField::Year => tr("All years"),
        FacetField::Format => tr("All formats"),
    }
}

/// A codec as the window names it: `FLAC`, `Opus`, `DSD`.
pub(crate) fn codec_name(codec: &str) -> String {
    let lower = codec.to_ascii_lowercase();
    if lower.starts_with("dsd") {
        return "DSD".to_owned();
    }
    if lower.starts_with("pcm") {
        return "PCM".to_owned();
    }
    match lower.as_str() {
        "opus" => "Opus".to_owned(),
        "vorbis" => "Vorbis".to_owned(),
        "wavpack" => "WavPack".to_owned(),
        "musepack" | "mpc" => "Musepack".to_owned(),
        _ => codec.to_uppercase(),
    }
}

/// One row of a pane: `None` is the "All …" row.
#[derive(Debug, Clone)]
struct Row {
    key: Option<FacetKey>,
    name: String,
    tracks: usize,
}

/// One pane.
struct Pane {
    root: gtk::Box,
    field: Cell<FacetField>,
    field_button: gtk::MenuButton,
    count: gtk::Label,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    list: gtk::ListView,
    /// The values chosen; empty for all.
    chosen: RefCell<Vec<FacetKey>>,
    /// Set while the selection is changed from code.
    syncing: Cell<bool>,
}

impl Pane {
    fn new(field: FacetField) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-facet-pane");
        root.set_hexpand(true);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.add_css_class("hk-pane-head");
        let field_button = gtk::MenuButton::new();
        field_button.add_css_class("hk-facet-field");
        field_button.set_always_show_arrow(true);
        field_button.set_valign(gtk::Align::Center);
        field_button.set_tooltip_text(Some(&tr("Choose the field of this pane")));
        head.append(&field_button);
        let count = label("", &["hk-numeric"]);
        count.set_hexpand(true);
        count.set_xalign(1.0);
        head.append(&count);
        root.append(&head);
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let name = label("", &[]);
            name.set_hexpand(true);
            row.append(&name);
            let tracks = label("", &["hk-facet-count", "hk-numeric"]);
            tracks.set_xalign(1.0);
            row.append(&tracks);
            item.set_child(Some(&row));
        });
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(object), Some(row)) = (
                item.item().and_downcast::<glib::BoxedAnyObject>(),
                item.child().and_downcast::<gtk::Box>(),
            ) else {
                return;
            };
            let value = object.borrow::<Row>();
            if let Some(name) = row.first_child().and_downcast::<gtk::Label>() {
                name.set_text(&value.name);
            }
            if let Some(tracks) = row.last_child().and_downcast::<gtk::Label>() {
                tracks.set_text(&format::count(value.tracks));
            }
            item.set_accessible_label(&value.name);
        });
        let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
        // Tab leaves the list after the focused row, instead of visiting
        // every row: Tab and Shift+Tab reach the rest of the window.
        list.set_tab_behavior(gtk::ListTabBehavior::Item);
        list.add_css_class("hk-list");
        list.add_css_class("facet");
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        root.append(&scroller);
        let pane = Self {
            root,
            field: Cell::new(field),
            field_button,
            count,
            store,
            selection,
            list,
            chosen: RefCell::new(Vec::new()),
            syncing: Cell::new(false),
        };
        pane.show_field();
        pane
    }

    fn show_field(&self) {
        let field = self.field.get();
        self.field_button.set_label(&field_title(field));
        self.list
            .update_property(&[gtk::accessible::Property::Label(&field_title(field))]);
    }

    /// Fills the pane with `list` and selects the chosen values in it.
    fn fill(&self, list: &FacetList) {
        let field = self.field.get();
        let mut rows: Vec<glib::BoxedAnyObject> = Vec::with_capacity(list.values.len() + 1);
        rows.push(glib::BoxedAnyObject::new(Row {
            key: None,
            name: all_title(field),
            tracks: list.tracks,
        }));
        rows.extend(list.values.iter().map(|value| {
            glib::BoxedAnyObject::new(Row {
                key: Some(value.key.clone()),
                name: if field == FacetField::Format {
                    codec_name(&value.name)
                } else if value.name.is_empty() {
                    tr("Unknown")
                } else {
                    value.name.clone()
                },
                tracks: value.tracks,
            })
        }));
        self.syncing.set(true);
        self.store.splice(0, self.store.n_items(), &rows);
        self.count.set_text(&format::count(list.values.len()));
        let chosen = self.chosen.borrow().clone();
        self.selection.unselect_all();
        if chosen.is_empty() {
            self.selection.select_item(0, false);
        } else {
            for (i, value) in list.values.iter().enumerate() {
                if chosen.contains(&value.key) {
                    self.selection.select_item(i as u32 + 1, false);
                }
            }
        }
        self.syncing.set(false);
    }

    /// The keys the selection holds; empty when "All" or nothing is selected.
    fn selected_keys(&self) -> Vec<FacetKey> {
        let selected = self.selection.selection();
        let mut keys = Vec::new();
        for i in 0..selected.size() {
            let position = selected.nth(i as u32);
            if position == 0 {
                continue;
            }
            if let Some(object) = self
                .store
                .item(position)
                .and_downcast::<glib::BoxedAnyObject>()
                && let Some(key) = object.borrow::<Row>().key.clone()
            {
                keys.push(key);
            }
        }
        keys
    }
}

type Changed = Box<dyn Fn()>;

/// The browser.
pub(crate) struct FacetBrowser {
    pub(crate) root: gtk::Box,
    panes: Vec<Pane>,
    backend: Backend,
    /// The filter text of the table.
    text: RefCell<String>,
    generation: Cell<u64>,
    /// Called when the choices change, so the table loads again.
    changed: RefCell<Option<Changed>>,
}

impl std::fmt::Debug for FacetBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FacetBrowser").finish_non_exhaustive()
    }
}

impl FacetBrowser {
    pub(crate) fn new(backend: &Backend) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("hk-facets");
        root.set_homogeneous(true);
        root.set_size_request(-1, 222);
        root.set_vexpand(false);
        root.update_property(&[gtk::accessible::Property::Label(&tr("Browse"))]);
        let panes: Vec<Pane> = DEFAULT_FIELDS.into_iter().map(Pane::new).collect();
        for pane in &panes {
            root.append(&pane.root);
        }
        let browser = Rc::new(Self {
            root,
            panes,
            backend: backend.clone(),
            text: RefCell::new(String::new()),
            generation: Cell::new(0),
            changed: RefCell::new(None),
        });
        for index in 0..browser.panes.len() {
            browser.connect_pane(index);
        }
        browser
    }

    fn connect_pane(self: &Rc<Self>, index: usize) {
        let pane = &self.panes[index];
        // The field menu.
        let group = gio::SimpleActionGroup::new();
        let action = gio::SimpleAction::new_stateful(
            "field",
            Some(glib::VariantTy::STRING),
            &field_id(pane.field.get()).to_variant(),
        );
        let weak = Rc::downgrade(self);
        action.connect_activate(move |action, target| {
            let Some(id) = target.and_then(|t| t.get::<String>()) else {
                return;
            };
            let (Some(browser), Some(field)) = (weak.upgrade(), field_from_id(&id)) else {
                return;
            };
            action.set_state(&id.to_variant());
            browser.set_field(index, field);
        });
        group.add_action(&action);
        pane.root
            .insert_action_group(&format!("facet{index}"), Some(&group));
        let menu = gio::Menu::new();
        for field in FacetField::ALL {
            let item = gio::MenuItem::new(Some(&field_title(field)), None);
            item.set_action_and_target_value(
                Some(&format!("facet{index}.field")),
                Some(&field_id(field).to_variant()),
            );
            menu.append_item(&item);
        }
        crate::widgets::menu_names::set_menu_later(&pane.field_button, &menu);
        // Choosing values.
        let weak = Rc::downgrade(self);
        pane.selection
            .connect_selection_changed(move |selection, position, n| {
                let Some(browser) = weak.upgrade() else {
                    return;
                };
                let pane = &browser.panes[index];
                if pane.syncing.get() {
                    return;
                }
                // "All" and the values exclude each other: choosing "All" clears
                // the values, choosing a value clears "All", and nothing chosen
                // is "All".
                pane.syncing.set(true);
                let all_changed = position == 0 && n > 0;
                if all_changed && selection.is_selected(0) {
                    selection.select_item(0, true);
                } else if selection.is_selected(0) && selection.selection().size() > 1 {
                    selection.unselect_item(0);
                } else if selection.selection().is_empty() {
                    selection.select_item(0, false);
                }
                pane.syncing.set(false);
                let keys = pane.selected_keys();
                if *pane.chosen.borrow() != keys {
                    pane.chosen.replace(keys);
                    browser.refresh_from(index + 1, true);
                }
            });
    }

    fn set_field(self: &Rc<Self>, index: usize, field: FacetField) {
        let pane = &self.panes[index];
        if pane.field.replace(field) == field {
            return;
        }
        pane.show_field();
        let had = !pane.chosen.replace(Vec::new()).is_empty();
        self.refresh_from(index, had);
    }

    /// Calls `changed` when the choices change.
    pub(crate) fn connect_changed(&self, changed: impl Fn() + 'static) {
        self.changed.replace(Some(Box::new(changed)));
    }

    /// The choices of every pane, in pane order.
    pub(crate) fn chosen(&self) -> Vec<FacetChoice> {
        self.panes
            .iter()
            .map(|pane| FacetChoice {
                field: pane.field.get(),
                keys: pane.chosen.borrow().clone(),
            })
            .collect()
    }

    /// Whether any pane has values chosen.
    pub(crate) fn narrows(&self) -> bool {
        self.panes.iter().any(|p| !p.chosen.borrow().is_empty())
    }

    /// The filter text changed (or the library did): every pane is filled again.
    pub(crate) fn set_text(self: &Rc<Self>, text: &str) {
        self.text.replace(text.to_owned());
        self.refresh_from(0, false);
    }

    /// Fills the panes from `first` on again; the chosen values that are still
    /// listed stay chosen. Calls `changed` when `notify` or when choices were
    /// dropped.
    fn refresh_from(self: &Rc<Self>, first: usize, notify: bool) {
        if first >= self.panes.len() {
            if notify {
                self.notify();
            }
            return;
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let query = TrackQuery {
            text: self.text.borrow().trim().to_owned(),
            ..TrackQuery::default()
        };
        let choices = self.chosen();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local({
            let backend = self.backend.clone();
            async move {
                let result = backend
                    .query(move |library| {
                        let mut choices = choices;
                        let mut lists = Vec::new();
                        for index in first..choices.len() {
                            let list = library.facet_values(
                                &query,
                                &choices[..index],
                                choices[index].field,
                            )?;
                            choices[index]
                                .keys
                                .retain(|key| list.values.iter().any(|v| v.key == *key));
                            lists.push(list);
                        }
                        Ok((lists, choices))
                    })
                    .await;
                let Some(browser) = weak.upgrade() else {
                    return;
                };
                if browser.generation.get() != generation {
                    return;
                }
                let Ok((lists, choices)) = result else { return };
                let mut dropped = false;
                for (offset, list) in lists.iter().enumerate() {
                    let index = first + offset;
                    let pane = &browser.panes[index];
                    if *pane.chosen.borrow() != choices[index].keys {
                        dropped = true;
                        pane.chosen.replace(choices[index].keys.clone());
                    }
                    pane.fill(list);
                }
                if notify || dropped {
                    browser.notify();
                }
            }
        });
    }

    fn notify(&self) {
        if let Some(changed) = self.changed.borrow().as_ref() {
            changed();
        }
    }

    /// The names and counts each pane lists, and which rows are chosen (for
    /// tests).
    pub(crate) fn rows(&self) -> Vec<Vec<(String, usize, bool)>> {
        self.panes
            .iter()
            .map(|pane| {
                (0..pane.store.n_items())
                    .filter_map(|i| {
                        let object = pane.store.item(i).and_downcast::<glib::BoxedAnyObject>()?;
                        let row = object.borrow::<Row>();
                        Some((row.name.clone(), row.tracks, pane.selection.is_selected(i)))
                    })
                    .collect()
            })
            .collect()
    }

    /// Chooses the rows `positions` of pane `index`, as clicks would (for tests).
    pub(crate) fn choose(&self, index: usize, positions: &[u32]) {
        let Some(pane) = self.panes.get(index) else {
            return;
        };
        for (i, position) in positions.iter().enumerate() {
            pane.selection.select_item(*position, i == 0);
        }
    }

    /// Changes the field of pane `index` (for tests).
    pub(crate) fn choose_field(self: &Rc<Self>, index: usize, field: FacetField) {
        self.set_field(index, field);
    }
}
