// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! "Equalizer for this track" in the Playback group of the tag editor's File
//! info tab: a menu button that names the track's own equalizer ("None"
//! without one) and chooses a whole one: a copy of the main equalizer as it is
//! now, a preset, or an imported AutoEQ file; with several tracks in the
//! editor, the same for all of them; or none. Its filters are not edited here.
//!
//! The equalizer is kept in the library, not in the file: a change is stored at
//! once, is not part of "Save", and applies from the next time the track
//! starts. A line under the row says what the track plays with.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::audio::OutputMode;
use helikodu_model::settings::{EqMode, EqualizerSettings};

use crate::backend::{Backend, SignalState, TrackEqualizer, TrackKey};
use crate::dialogs::settings::dsp::equalizer::{matching_preset, preset_entries, preset_gains};
use crate::i18n::{fill, ntr, tr};
use crate::widgets::bipolar_scale::decibels;
use crate::widgets::label;
use crate::window::MainWindow;

/// The widget name of the row's menu button, which the context menu's
/// "Equalizer for This Track…" focuses.
pub(crate) const BUTTON: &str = "track-equalizer";

/// How long "From the next time the track starts" shows after a change.
const CHANGED_NOTE: Duration = Duration::from_secs(2);

/// The kind and size of `eq` in a few words: "Parametric · 4 filters ·
/// preamp −3.0 dB", "Graphic, 10 bands · preamp −2.0 dB".
pub(crate) fn summary(eq: &EqualizerSettings) -> String {
    let kind = match eq.mode {
        EqMode::Parametric => {
            let n = eq.parametric.len();
            fill(
                &tr("Parametric · {}"),
                &[&fill(&ntr("{} filter", "{} filters", n), &[&n.to_string()])],
            )
        }
        EqMode::Graphic10 => fill(&tr("Graphic, {} bands"), &["10"]),
        EqMode::Graphic31 => fill(&tr("Graphic, {} bands"), &["31"]),
    };
    fill(
        &tr("{} · preamp {}"),
        &[&kind, &decibels(eq.effective_preamp_db())],
    )
}

/// An equalizer that does nothing: the track plays without any equalizer,
/// the main one included ("No Equalizer for This Track" in the context menu).
pub(crate) fn no_equalizer() -> TrackEqualizer {
    TrackEqualizer {
        name: tr("No equalizer"),
        settings: EqualizerSettings::default(),
    }
}

/// Whether `eq` plays the track without any equalizer.
pub(crate) fn is_none(eq: &TrackEqualizer) -> bool {
    !eq.settings.enabled
}

/// The main equalizer as it is now, as a track's own: named "Copy of" its
/// preset, or "Own settings".
fn current_equalizer(main: &EqualizerSettings) -> TrackEqualizer {
    let name = match matching_preset(main).and_then(|i| preset_entries(main).get(i).cloned()) {
        Some((preset, _)) => fill(&tr("Copy of {}"), &[&preset]),
        None => tr("Own settings"),
    };
    let mut settings = main.clone();
    settings.enabled = true;
    settings.presets.clear();
    TrackEqualizer { name, settings }
}

/// The preset `index` of the preset list (the built-in ones, then the user's).
fn preset_equalizer(main: &EqualizerSettings, index: usize) -> Option<TrackEqualizer> {
    let (name, gains) = preset_entries(main).get(index).cloned()?;
    let mut settings = EqualizerSettings {
        enabled: true,
        ..EqualizerSettings::default()
    };
    if gains.len() == 31 {
        settings.mode = EqMode::Graphic31;
        settings.graphic_31.copy_from_slice(&gains);
        settings
            .graphic_10
            .copy_from_slice(&preset_gains(&gains, 10));
    } else {
        settings.mode = EqMode::Graphic10;
        settings
            .graphic_10
            .copy_from_slice(&preset_gains(&gains, 10));
        settings
            .graphic_31
            .copy_from_slice(&preset_gains(&gains, 31));
    }
    Some(TrackEqualizer { name, settings })
}

/// An AutoEQ `ParametricEQ.txt` file as a track's equalizer, named after the
/// file.
pub(crate) fn imported_equalizer(file_name: &str, bytes: &[u8]) -> Option<TrackEqualizer> {
    let imported = helikodu_service::autoeq::parse_bytes(bytes).ok()?;
    let mut settings = EqualizerSettings {
        enabled: true,
        mode: EqMode::Parametric,
        ..EqualizerSettings::default()
    };
    settings.parametric = imported.filters;
    settings.parametric.truncate(EqualizerSettings::MAX_FILTERS);
    if let Some(db) = imported.preamp_db {
        settings.auto_preamp = false;
        settings.preamp_db = db.clamp(
            *EqualizerSettings::PREAMP_RANGE.start(),
            *EqualizerSettings::PREAMP_RANGE.end(),
        );
    }
    settings.validate().ok()?;
    let stem = std::path::Path::new(file_name).file_stem().map_or_else(
        || file_name.to_owned(),
        |s| s.to_string_lossy().into_owned(),
    );
    // AutoEQ names its files "<headphones> ParametricEQ".
    let name = stem
        .strip_suffix("ParametricEQ")
        .map(str::trim_end)
        .filter(|s| !s.is_empty())
        .unwrap_or(&stem)
        .to_owned();
    Some(TrackEqualizer { name, settings })
}

/// What a menu item of the row does.
type RowAction = Box<dyn Fn(&Rc<Row>, Option<&glib::Variant>)>;

/// The row and what it shows.
struct Row {
    backend: Backend,
    main: glib::WeakRef<MainWindow>,
    track: TrackKey,
    others: Vec<TrackKey>,
    button: gtk::MenuButton,
    name: gtk::Label,
    detail: gtk::Label,
    line: gtk::Label,
    actions: gio::SimpleActionGroup,
    current: RefCell<Option<TrackEqualizer>>,
    /// Counts the changes, so that the note after one goes away only if no
    /// other came since.
    changes: Cell<u32>,
    /// While "From the next time the track starts" shows.
    changed_note: Cell<bool>,
}

impl Row {
    /// The line under the row, for the equalizer the track has now.
    fn refresh(&self) {
        let settings = self.backend.services().settings();
        let current = self.current.borrow();
        let (title, tooltip) = match current.as_ref() {
            Some(eq) => (eq.name.clone(), Some(eq.name.clone())),
            None => (tr("None"), None),
        };
        self.name.set_text(&title);
        self.button.set_tooltip_text(tooltip.as_deref());
        self.button
            .update_property(&[gtk::accessible::Property::Label(&fill(
                &tr("Equalizer for this track: {}"),
                &[&title],
            ))]);
        match current.as_ref() {
            Some(eq) if !is_none(eq) => {
                self.detail.set_text(&summary(&eq.settings));
                self.detail.set_visible(true);
            }
            _ => self.detail.set_visible(false),
        }
        let has = current.is_some();
        for (name, enabled) in [("remove", has), ("all", has)] {
            if let Some(action) = self
                .actions
                .lookup_action(name)
                .and_downcast::<gio::SimpleAction>()
            {
                action.set_enabled(enabled);
            }
        }
        let used = settings.dsp.track_equalizer;
        self.button.set_sensitive(used);
        let text = if !used {
            tr("Equalizers of single tracks are turned off in Settings › DSP and EQ.")
        } else if self.changed_note.get() {
            tr("From the next time the track starts")
        } else if current.as_ref().is_some_and(is_none) {
            tr(
                "Plays without any equalizer, the main one included. Kept in the library, not in the file.",
            )
        } else if has && self.bit_perfect(settings.output.mode) {
            tr("Not applied while playback is bit-perfect.")
        } else if has {
            tr(
                "Replaces the main equalizer while this track plays. Kept in the library, not in the file.",
            )
        } else {
            tr("The main equalizer applies.")
        };
        self.line.set_text(&text);
    }

    /// Whether playback is bit-perfect in exclusive mode, so that a track's
    /// equalizer is left out.
    fn bit_perfect(&self, mode: OutputMode) -> bool {
        mode == OutputMode::Exclusive
            && self
                .main
                .upgrade()
                .and_then(|w| w.playback())
                .is_some_and(|p| p.view().output.signal == SignalState::BitPerfect)
    }

    /// Whether the track of the row plays now.
    fn playing(&self) -> bool {
        self.main
            .upgrade()
            .and_then(|w| w.playback())
            .is_some_and(|p| p.view().playing.contains(self.track))
    }

    /// Gives `tracks` `equalizer` (none with `None`) and shows it.
    fn store(self: &Rc<Self>, tracks: Vec<TrackKey>, equalizer: Option<TrackEqualizer>) {
        let services = self.backend.services();
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let stored = equalizer.clone();
            let result = backend
                .work(move || services.set_track_equalizers(&tracks, stored.as_ref()))
                .await;
            let Some(row) = weak.upgrade() else { return };
            match result {
                Some(Ok(())) => {
                    row.current.replace(equalizer);
                    row.after_change();
                }
                Some(Err(error)) => {
                    if let Some(main) = row.main.upgrade() {
                        main.toast(
                            &fill(
                                &tr("The equalizer of the track could not be saved: {}"),
                                &[&error.0],
                            ),
                            true,
                        );
                    }
                }
                None => {}
            }
        });
    }

    /// After a change: while the track plays, the line says for two seconds
    /// that it applies from the next time the track starts.
    fn after_change(self: &Rc<Self>) {
        let change = self.changes.get().wrapping_add(1);
        self.changes.set(change);
        if self.playing() {
            self.changed_note.set(true);
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(CHANGED_NOTE, move || {
                if let Some(row) = weak.upgrade()
                    && row.changes.get() == change
                {
                    row.changed_note.set(false);
                    row.refresh();
                }
            });
        }
        self.refresh();
    }

    fn import(self: &Rc<Self>) {
        let Some(window) = self.button.root().and_downcast::<gtk::Window>() else {
            return;
        };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&tr("Equalizer settings (ParametricEQ.txt)")));
        filter.add_pattern("*.txt");
        filter.add_mime_type("text/plain");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(tr("Import AutoEQ file"))
            .modal(true)
            .filters(&filters)
            .build();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(file) = dialog.open_future(Some(&window)).await else {
                return;
            };
            let name = file
                .basename()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let loaded = file.load_contents_future().await;
            let Some(row) = weak.upgrade() else { return };
            let problem = match loaded {
                Ok((bytes, _)) => match imported_equalizer(&name, &bytes) {
                    Some(eq) => {
                        row.store(vec![row.track], Some(eq));
                        return;
                    }
                    None => {
                        tr("This file has no parametric equalizer filters that Helikodu can read.")
                    }
                },
                Err(e) => fill(&tr("Cannot read the file: {}"), &[e.message()]),
            };
            if let Some(main) = row.main.upgrade() {
                main.toast(&problem, true);
            }
        });
    }
}

/// The menu: the sources of an equalizer, the same for all tracks of the
/// editor, and none.
fn menu(main: &EqualizerSettings, tracks: usize) -> gio::Menu {
    let menu = gio::Menu::new();
    let sources = gio::Menu::new();
    sources.append(
        Some(&tr("Use the Current Equalizer")),
        Some("trackeq.current"),
    );
    let presets = gio::Menu::new();
    for (i, (name, _)) in preset_entries(main).iter().enumerate() {
        let item = gio::MenuItem::new(Some(name), None);
        item.set_action_and_target_value(
            Some("trackeq.preset"),
            Some(&u32::try_from(i).unwrap_or(0).to_variant()),
        );
        presets.append_item(&item);
    }
    sources.append_submenu(Some(&tr("Use a Preset")), &presets);
    sources.append(Some(&tr("Import AutoEQ File…")), Some("trackeq.import"));
    menu.append_section(None, &sources);
    if tracks > 1 {
        let all = gio::Menu::new();
        all.append(
            Some(&fill(
                &ntr("Use for All {} Track", "Use for All {} Tracks", tracks),
                &[&tracks.to_string()],
            )),
            Some("trackeq.all"),
        );
        menu.append_section(None, &all);
    }
    let remove = gio::Menu::new();
    remove.append(Some(&tr("Remove")), Some("trackeq.remove"));
    menu.append_section(None, &remove);
    menu
}

/// The row for `track` of the editor, whose tracks are `tracks`. It shows
/// "None" until the equalizer has been read.
pub(crate) fn row(
    backend: &Backend,
    main: Option<&MainWindow>,
    track: TrackKey,
    tracks: &[TrackKey],
) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.set_widget_name("track-equalizer-row");
    column.add_css_class("hk-track-eq");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.add_css_class("hk-track-eq-row");
    let title = label(&tr("Equalizer for this track"), &[]);
    title.set_hexpand(true);
    line.append(&title);
    let button = gtk::MenuButton::new();
    button.set_widget_name(BUTTON);
    button.add_css_class("hk-sort");
    button.add_css_class("hk-track-eq-button");
    button.set_valign(gtk::Align::Center);
    button.set_always_show_arrow(true);
    let name = label(&tr("None"), &["hk-track-eq-name"]);
    name.set_max_width_chars(14);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    button.set_child(Some(&name));
    button.set_sensitive(false);
    line.append(&button);
    column.append(&line);
    let detail = label("", &["hk-mono-meta", "hk-t3", "hk-track-eq-detail"]);
    detail.set_visible(false);
    column.append(&detail);
    let note = label("", &["hk-t3", "hk-track-eq-line"]);
    note.set_ellipsize(gtk::pango::EllipsizeMode::None);
    note.set_single_line_mode(false);
    note.set_wrap(true);
    note.set_xalign(0.0);
    note.set_max_width_chars(40);
    column.append(&note);

    let main_eq = backend.services().settings().dsp.equalizer.clone();
    let others: Vec<TrackKey> = tracks
        .iter()
        .copied()
        .filter(|t| *t != track && t.0 != 0)
        .collect();
    let actions = gio::SimpleActionGroup::new();
    let row = Rc::new(Row {
        backend: backend.clone(),
        main: main.map(|m| m.downgrade()).unwrap_or_default(),
        track,
        others,
        button: button.clone(),
        name,
        detail,
        line: note,
        actions: actions.clone(),
        current: RefCell::new(None),
        changes: Cell::new(0),
        changed_note: Cell::new(false),
    });

    let add = |name: &str, parameter: Option<&glib::VariantTy>, run: RowAction| {
        let action = gio::SimpleAction::new(name, parameter);
        let weak = Rc::downgrade(&row);
        action.connect_activate(move |_, value| {
            if let Some(row) = weak.upgrade() {
                run(&row, value);
            }
        });
        actions.add_action(&action);
    };
    add(
        "current",
        None,
        Box::new(|row, _| {
            let main = row.backend.services().settings().dsp.equalizer.clone();
            row.store(vec![row.track], Some(current_equalizer(&main)));
        }),
    );
    add(
        "preset",
        Some(glib::VariantTy::UINT32),
        Box::new(|row, value| {
            let main = row.backend.services().settings().dsp.equalizer.clone();
            let index = value.and_then(glib::Variant::get::<u32>).unwrap_or(0);
            if let Some(eq) = preset_equalizer(&main, index as usize) {
                row.store(vec![row.track], Some(eq));
            }
        }),
    );
    add("import", None, Box::new(|row, _| row.import()));
    add(
        "all",
        None,
        Box::new(|row, _| {
            let eq = row.current.borrow().clone();
            if eq.is_some() {
                let mut tracks = row.others.clone();
                tracks.push(row.track);
                row.store(tracks, eq);
            }
        }),
    );
    add(
        "remove",
        None,
        Box::new(|row, _| row.store(vec![row.track], None)),
    );
    column.insert_action_group("trackeq", Some(&actions));
    crate::widgets::menu_names::set_menu_later(&button, &menu(&main_eq, tracks.len()));
    button.connect_notify_local(Some("active"), |button, _| {
        if let Some(popover) = button.popover() {
            popover.add_css_class("hk-track-eq-menu");
        }
    });

    row.refresh();
    let services = backend.services();
    let backend = backend.clone();
    let weak = Rc::downgrade(&row);
    glib::spawn_future_local(async move {
        let read = backend
            .work(move || services.track_equalizer(track))
            .await
            .and_then(Result::ok)
            .flatten();
        let Some(row) = weak.upgrade() else { return };
        row.current.replace(read);
        row.refresh();
    });
    // The row lives as long as its widgets.
    let keep = RefCell::new(Some(row));
    column.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    column
}

#[cfg(test)]
mod tests {
    use super::*;
    use helikodu_model::settings::{FilterKind, ParametricFilter};

    #[test]
    fn the_summary_names_kind_size_and_preamp() {
        let mut eq = EqualizerSettings {
            enabled: true,
            mode: EqMode::Parametric,
            auto_preamp: false,
            preamp_db: -3.0,
            ..EqualizerSettings::default()
        };
        eq.parametric = vec![
            ParametricFilter {
                kind: FilterKind::Peak,
                frequency_hz: 1000.0,
                gain_db: 2.0,
                q: 1.0,
                enabled: true,
            };
            4
        ];
        assert_eq!(summary(&eq), "Parametric · 4 filters · preamp −3.0 dB");
        eq.mode = EqMode::Graphic10;
        eq.preamp_db = -2.0;
        assert_eq!(summary(&eq), "Graphic, 10 bands · preamp −2.0 dB");
    }

    #[test]
    fn a_copy_of_the_main_equalizer_is_named_after_its_preset() {
        let flat = EqualizerSettings {
            mode: EqMode::Graphic10,
            ..EqualizerSettings::default()
        };
        let copy = current_equalizer(&flat);
        assert_eq!(copy.name, "Copy of Flat");
        assert!(copy.settings.enabled);
        let mut own = flat.clone();
        own.graphic_10[3] = 4.5;
        assert_eq!(current_equalizer(&own).name, "Own settings");
        let preset = preset_equalizer(&flat, 1).unwrap();
        assert_eq!(preset.name, "Bass boost");
        assert_eq!(preset.settings.mode, EqMode::Graphic10);
        assert_eq!(preset.settings.graphic_10[0], 6.0);
    }

    #[test]
    fn an_imported_file_gives_its_name() {
        let text = b"Preamp: -6.2 dB\nFilter 1: ON PK Fc 105 Hz Gain -3.5 dB Q 0.70\n";
        let eq = imported_equalizer("Sennheiser HD 600 ParametricEQ.txt", text).unwrap();
        assert_eq!(eq.name, "Sennheiser HD 600");
        assert_eq!(eq.settings.mode, EqMode::Parametric);
        assert_eq!(eq.settings.parametric.len(), 1);
        assert_eq!(eq.settings.preamp_db, -6.2);
        assert!(imported_equalizer("notes.txt", b"nothing here").is_none());
    }
}
