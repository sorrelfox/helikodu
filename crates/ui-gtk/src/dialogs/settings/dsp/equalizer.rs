// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The equalizer's editor: the preset row (graphic presets, AutoEQ import and
//! export through the file chooser), the mode row with the preamp, the graph,
//! and under it the filter table (parametric) or the help line and "Reset to
//! flat" (graphic). Each mode keeps its own values.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::eq::graphic;
use helikodu_model::settings::{
    BuiltInPreset, EqMode, EqualizerSettings, FilterKind, GRAPHIC_10_BANDS_HZ, GRAPHIC_31_BANDS_HZ,
    GraphicPreset, ParametricFilter,
};
use helikodu_service::autoeq;

use super::Live;
use crate::dialogs::common::{
    bind_segmented, compact_button, dropdown, note, on_selected, segmented,
};
use crate::i18n::{fill, ntr, tr};
use crate::widgets::bipolar_scale::decibels;
use crate::widgets::eq_graph::{EqGraph, hz_text, signed};
use crate::widgets::{icon_button, label};

const MODES: [EqMode; 3] = [EqMode::Parametric, EqMode::Graphic10, EqMode::Graphic31];

/// The rate graphic bands are exported for.
const EXPORT_RATE: f64 = 48_000.0;

const KINDS: [FilterKind; 5] = [
    FilterKind::Peak,
    FilterKind::LowShelf,
    FilterKind::HighShelf,
    FilterKind::LowPass,
    FilterKind::HighPass,
];

fn kind_name(kind: FilterKind) -> String {
    match kind {
        FilterKind::Peak => tr("Peaking"),
        FilterKind::LowShelf => tr("Low shelf"),
        FilterKind::HighShelf => tr("High shelf"),
        FilterKind::LowPass => tr("Low pass"),
        FilterKind::HighPass => tr("High pass"),
    }
}

/// The preamp as the mode row shows it: `−4.5 dB (auto)`.
fn preamp_text(eq: &EqualizerSettings) -> String {
    let db = decibels(eq.effective_preamp_db());
    if eq.auto_preamp {
        fill(&tr("{} (auto)"), &[&db])
    } else {
        db
    }
}

/// A preset's gains for `bands` bands.
pub(crate) fn preset_gains(octaves: &[f64], bands: usize) -> Vec<f64> {
    match (octaves.len(), bands) {
        (10, 31) => {
            let mut ten = [0.0; 10];
            ten.copy_from_slice(octaves);
            BuiltInPreset {
                id: "",
                name: "",
                octaves: ten,
            }
            .third_octaves()
            .to_vec()
        }
        // The octave centres of the 31 bands.
        (31, 10) => (0..10).map(|i| octaves[2 + 3 * i]).collect(),
        _ => octaves.to_vec(),
    }
}

/// The translated name of a built-in preset.
pub(crate) fn preset_name(preset: &BuiltInPreset) -> String {
    match preset.id {
        "flat" => tr("Flat"),
        "bass-boost" => tr("Bass boost"),
        "bass-cut" => tr("Bass cut"),
        "treble-boost" => tr("Treble boost"),
        "treble-cut" => tr("Treble cut"),
        "loudness" => tr("Loudness"),
        "vocal" => tr("Vocal"),
        "rock" => tr("Rock"),
        "pop" => tr("Pop"),
        "jazz" => tr("Jazz"),
        "classical" => tr("Classical"),
        "electronic" => tr("Electronic"),
        "acoustic" => tr("Acoustic"),
        _ => preset.name.to_owned(),
    }
}

/// The entries of the preset list: the built-in presets, then the user's.
pub(crate) fn preset_entries(eq: &EqualizerSettings) -> Vec<(String, Vec<f64>)> {
    BuiltInPreset::ALL
        .iter()
        .map(|p| (preset_name(p), p.octaves.to_vec()))
        .chain(eq.presets.iter().map(|p| (p.name.clone(), p.bands.clone())))
        .collect()
}

/// Which entry the bands of the mode in use match, if any.
pub(crate) fn matching_preset(eq: &EqualizerSettings) -> Option<usize> {
    let gains = eq.graphic_gains()?;
    preset_entries(eq).iter().position(|(_, bands)| {
        let bands = preset_gains(bands, gains.len());
        bands.len() == gains.len() && bands.iter().zip(gains).all(|(a, b)| (a - b).abs() < 0.05)
    })
}

/// The filters a graphic equalizer is exported as: its bands as peaking
/// filters with the overlap-corrected gains the player uses.
fn graphic_as_filters(eq: &EqualizerSettings) -> Vec<ParametricFilter> {
    let (centres, gains): (&[f64], &[f64]) = match eq.mode {
        EqMode::Graphic31 => (&GRAPHIC_31_BANDS_HZ, &eq.graphic_31),
        _ => (&GRAPHIC_10_BANDS_HZ, &eq.graphic_10),
    };
    let q = graphic::band_q(centres.len());
    graphic::corrected_gains(centres, gains, EXPORT_RATE)
        .iter()
        .zip(centres)
        .map(|(gain, hz)| ParametricFilter {
            kind: FilterKind::Peak,
            frequency_hz: *hz,
            gain_db: (gain * 10.0).round() / 10.0,
            q,
            enabled: true,
        })
        .collect()
}

/// A number typed into a cell: `105`, `1.2k`, `+4.5 dB`, `0,7`.
fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim().replace(',', ".").replace('−', "-");
    let digits: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'))
        .collect();
    let value: f64 = digits.parse().ok()?;
    let rest = text[digits.len()..].trim_start().to_lowercase();
    let value = if rest.starts_with('k') {
        value * 1000.0
    } else {
        value
    };
    value.is_finite().then_some(value)
}

fn frequency_text(hz: f64) -> String {
    if hz >= 100.0 {
        fill(&tr("{} Hz"), &[&format!("{hz:.0}")])
    } else {
        hz_text(hz)
    }
}

/// The editor's widgets that follow the settings.
struct Editor {
    /// The table's columns: each header cell and the cells under it share a
    /// width.
    columns: [gtk::SizeGroup; 6],
    live: Rc<Live>,
    graph: EqGraph,
    mode_buttons: Vec<gtk::ToggleButton>,
    preamp: gtk::Label,
    under: gtk::Stack,
    rows: gtk::Box,
    count: gtk::Label,
    add: gtk::Button,
    preset_choice: gtk::DropDown,
    preset_strings: gtk::StringList,
    import: gtk::Button,
    save: gtk::MenuButton,
    updating: Cell<bool>,
    shown_presets: RefCell<Vec<String>>,
}

impl Editor {
    fn eq(&self) -> EqualizerSettings {
        self.live
            .settings()
            .map(|s| s.dsp.equalizer.clone())
            .unwrap_or_default()
    }

    /// Shows the stored settings, except the cells of the filter table.
    fn refresh(&self) {
        let eq = self.eq();
        self.updating.set(true);
        if let Some(i) = MODES.iter().position(|m| *m == eq.mode)
            && let Some(button) = self.mode_buttons.get(i)
            && !button.is_active()
        {
            button.set_active(true);
        }
        self.graph.set_settings(&eq);
        self.preamp.set_text(&preamp_text(&eq));
        let graphic = eq.mode != EqMode::Parametric;
        self.under
            .set_visible_child_name(if graphic { "graphic" } else { "parametric" });
        self.preset_choice.set_visible(graphic);
        self.save.set_visible(graphic);
        self.import.set_label(&if graphic {
            tr("Import")
        } else {
            tr("Import AutoEQ file")
        });
        let names: Vec<String> = preset_entries(&eq)
            .into_iter()
            .map(|(name, _)| name)
            .chain([tr("Custom")])
            .collect();
        if *self.shown_presets.borrow() != names {
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            self.preset_strings
                .splice(0, self.preset_strings.n_items(), &refs);
            self.shown_presets.replace(names.clone());
        }
        let chosen = matching_preset(&eq).unwrap_or(names.len() - 1);
        self.preset_choice
            .set_selected(u32::try_from(chosen).unwrap_or(0));
        let n = eq.parametric.len();
        self.count.set_text(&fill(
            &ntr(
                "{} of {} filter",
                "{} of {} filters",
                EqualizerSettings::MAX_FILTERS,
            ),
            &[&n.to_string(), &EqualizerSettings::MAX_FILTERS.to_string()],
        ));
        self.add.set_sensitive(n < EqualizerSettings::MAX_FILTERS);
        self.updating.set(false);
    }

    /// Builds the rows of the filter table again from the stored filters.
    fn rebuild_rows(self: &Rc<Self>) {
        remove_children(&self.rows);
        let eq = self.eq();
        for (i, filter) in eq.parametric.iter().enumerate() {
            self.rows.append(&self.filter_row(i, filter));
        }
        if eq.parametric.is_empty() {
            let empty = note(&tr(
                "No filters yet. Add one, or import a ParametricEQ.txt file from AutoEQ.",
            ));
            empty.set_margin_top(8);
            empty.set_margin_bottom(8);
            self.rows.append(&empty);
        }
    }

    /// Changes filter `i` with `change`; the result says whether the store
    /// took it.
    fn change_filter(&self, i: usize, change: impl Fn(&mut ParametricFilter)) -> bool {
        self.live.apply(|s| {
            if let Some(filter) = s.dsp.equalizer.parametric.get_mut(i) {
                change(filter);
            }
        })
    }

    fn value_cell(
        self: &Rc<Self>,
        i: usize,
        text: String,
        name: &str,
        set: fn(&mut ParametricFilter, f64),
        show: fn(&ParametricFilter) -> String,
    ) -> gtk::Entry {
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-plain-entry");
        entry.add_css_class("hk-mono");
        entry.add_css_class("hk-eq-cell");
        entry.set_text(&text);
        entry.set_width_chars(4);
        entry.set_max_width_chars(12);
        entry.set_hexpand(true);
        entry.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{} of filter {}"),
            &[name, &(i + 1).to_string()],
        ))]);
        let weak = Rc::downgrade(self);
        let save = move |entry: &gtk::Entry| {
            let Some(editor) = weak.upgrade() else {
                return;
            };
            let stored = editor.eq().parametric.get(i).copied();
            let Some(stored) = stored else {
                return;
            };
            match parse_number(&entry.text()) {
                Some(value) => {
                    let mut changed = stored;
                    set(&mut changed, value);
                    if changed != stored && !editor.change_filter(i, |f| set(f, value)) {
                        entry.set_text(&show(&stored));
                    } else {
                        entry.set_text(&show(&changed));
                    }
                }
                None => entry.set_text(&show(&stored)),
            }
        };
        let on_activate = save.clone();
        entry.connect_activate(move |entry| on_activate(entry));
        let focus = gtk::EventControllerFocus::new();
        let weak_entry = entry.downgrade();
        focus.connect_leave(move |_| {
            if let Some(entry) = weak_entry.upgrade() {
                save(&entry);
            }
        });
        entry.add_controller(focus);
        entry
    }

    /// A table cell of column `column`, at least `width` wide; the column's
    /// cells and header share the widest width.
    fn cell(&self, column: usize, widget: &impl IsA<gtk::Widget>, width: i32) -> gtk::Box {
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        cell.set_size_request(width, -1);
        // The cell keeps its width; only the spacer of the row takes room.
        cell.set_hexpand(false);
        let widget = widget.as_ref();
        widget.set_hexpand(true);
        cell.append(widget);
        self.columns[column].add_widget(&cell);
        cell
    }

    fn filter_row(self: &Rc<Self>, i: usize, filter: &ParametricFilter) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("hk-filter-row");
        if !filter.enabled {
            row.add_css_class("hk-dim");
        }
        let number = (i + 1).to_string();
        let on = gtk::CheckButton::new();
        on.set_active(filter.enabled);
        on.set_size_request(24, -1);
        on.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("Use filter {}"),
            &[&number],
        ))]);
        let weak = Rc::downgrade(self);
        let weak_row = row.downgrade();
        on.connect_toggled(move |check| {
            let Some(editor) = weak.upgrade() else {
                return;
            };
            let enabled = check.is_active();
            if editor.change_filter(i, |f| f.enabled = enabled) {
                if let Some(row) = weak_row.upgrade() {
                    if enabled {
                        row.remove_css_class("hk-dim");
                    } else {
                        row.add_css_class("hk-dim");
                    }
                }
            } else {
                check.set_active(!enabled);
            }
        });
        self.columns[0].add_widget(&on);
        row.append(&on);
        let index = label(&number, &["hk-t3", "hk-numeric"]);
        index.set_size_request(18, -1);
        self.columns[1].add_widget(&index);
        row.append(&index);
        let names: Vec<String> = KINDS.iter().map(|&k| kind_name(k)).collect();
        let kind = dropdown(
            &names,
            KINDS.iter().position(|k| *k == filter.kind).unwrap_or(0),
            150,
        );
        kind.add_css_class("hk-eq-kind");
        kind.add_css_class(crate::dialogs::common::OWN_LABEL);
        kind.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("Type of filter {}"),
            &[&number],
        ))]);
        let weak = Rc::downgrade(self);
        on_selected(&kind, move |k| {
            if let (Some(editor), Some(&kind)) = (weak.upgrade(), KINDS.get(k)) {
                editor.change_filter(i, |f| f.kind = kind);
            }
        });
        row.append(&self.cell(2, &kind, 150));
        row.append(&self.cell(
            3,
            &self.value_cell(
                i,
                frequency_text(filter.frequency_hz),
                &tr("Frequency"),
                |f, v| {
                    f.frequency_hz = v.clamp(
                        *ParametricFilter::FREQUENCY_RANGE.start(),
                        *ParametricFilter::FREQUENCY_RANGE.end(),
                    );
                },
                |f| frequency_text(f.frequency_hz),
            ),
            130,
        ));
        row.append(&self.cell(
            4,
            &self.value_cell(
                i,
                fill(&tr("{} dB"), &[&signed(filter.gain_db)]),
                &tr("Gain"),
                |f, v| {
                    f.gain_db = ((v * 10.0).round() / 10.0).clamp(
                        *ParametricFilter::GAIN_RANGE.start(),
                        *ParametricFilter::GAIN_RANGE.end(),
                    );
                },
                |f| fill(&tr("{} dB"), &[&signed(f.gain_db)]),
            ),
            110,
        ));
        row.append(&self.cell(
            5,
            &self.value_cell(
                i,
                format!("{:.2}", filter.q),
                &tr("Q"),
                |f, v| {
                    f.q = v.clamp(
                        *ParametricFilter::Q_RANGE.start(),
                        *ParametricFilter::Q_RANGE.end(),
                    );
                },
                |f| format!("{:.2}", f.q),
            ),
            70,
        ));
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        row.append(&spacer);
        let remove = icon_button(
            "window-close-symbolic",
            &fill(&tr("Remove filter {}"), &[&number]),
        );
        remove.add_css_class("hk-small-icon");
        let weak = Rc::downgrade(self);
        remove.connect_clicked(move |_| {
            if let Some(editor) = weak.upgrade()
                && editor.live.apply(|s| {
                    if i < s.dsp.equalizer.parametric.len() {
                        s.dsp.equalizer.parametric.remove(i);
                    }
                })
            {
                editor.rebuild_rows();
                // Keep the keyboard in the table.
                if let Some(row) = editor.rows.observe_children().item(
                    u32::try_from(i.min(editor.eq().parametric.len().saturating_sub(1)))
                        .unwrap_or(0),
                ) && let Ok(row) = row.downcast::<gtk::Widget>()
                {
                    row.child_focus(gtk::DirectionType::TabForward);
                }
            }
        });
        row.append(&remove);
        row
    }
}

fn remove_children(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn table_header(count: &gtk::Label, columns: &[gtk::SizeGroup; 6]) -> gtk::Box {
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    head.add_css_class("hk-filter-head");
    let cells: [(String, i32); 6] = [
        (String::new(), 24),
        ("#".to_owned(), 18),
        (tr("Type"), 150),
        (tr("Frequency"), 130),
        (tr("Gain"), 110),
        (tr("Q"), 70),
    ];
    for (i, (text, width)) in cells.into_iter().enumerate() {
        let cell = label(&text, &["hk-micro-label"]);
        cell.set_size_request(width, -1);
        columns[i].add_widget(&cell);
        head.append(&cell);
    }
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    head.append(&spacer);
    head.append(count);
    head
}

/// Opens the file chooser for an AutoEQ file and imports it into the
/// parametric equalizer.
fn import(editor: &Rc<Editor>) {
    let Some(context) = editor.live.context() else {
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
    let weak = Rc::downgrade(editor);
    let window = context.window.clone();
    glib::spawn_future_local(async move {
        let Ok(file) = dialog.open_future(Some(&window)).await else {
            return;
        };
        let bytes = match file.load_contents_future().await {
            Ok((bytes, _)) => bytes,
            Err(e) => {
                if let Some(editor) = weak.upgrade()
                    && let Some(context) = editor.live.context()
                {
                    context.toast(&fill(&tr("Cannot read the file: {}"), &[e.message()]));
                }
                return;
            }
        };
        let Some(editor) = weak.upgrade() else {
            return;
        };
        let Some(context) = editor.live.context() else {
            return;
        };
        match autoeq::parse_bytes(&bytes) {
            Ok(imported) => {
                let filters = imported.filters.clone();
                let preamp = imported.preamp_db;
                let applied = editor.live.apply(|s| {
                    let eq = &mut s.dsp.equalizer;
                    eq.mode = EqMode::Parametric;
                    eq.parametric = filters.clone();
                    eq.parametric.truncate(EqualizerSettings::MAX_FILTERS);
                    if let Some(db) = preamp {
                        eq.auto_preamp = false;
                        eq.preamp_db = db.clamp(
                            *EqualizerSettings::PREAMP_RANGE.start(),
                            *EqualizerSettings::PREAMP_RANGE.end(),
                        );
                    } else {
                        eq.auto_preamp = true;
                    }
                });
                if applied {
                    editor.rebuild_rows();
                    let n = imported.filters.len();
                    let text = fill(
                        &ntr("{} filter imported", "{} filters imported", n),
                        &[&n.to_string()],
                    );
                    let skipped = imported.warnings.len();
                    context.toast(&if skipped == 0 {
                        text
                    } else {
                        fill(
                            &ntr(
                                "{}; {} line was skipped or changed",
                                "{}; {} lines were skipped or changed",
                                skipped,
                            ),
                            &[&text, &skipped.to_string()],
                        )
                    });
                }
            }
            Err(_) => context.toast(&tr(
                "This file has no parametric equalizer filters that Helikodu can read.",
            )),
        }
    });
}

/// Saves the equalizer in use as a ParametricEQ.txt file.
fn export(editor: &Rc<Editor>) {
    let Some(context) = editor.live.context() else {
        return;
    };
    let eq = editor.eq();
    let filters = if eq.mode == EqMode::Parametric {
        eq.parametric.clone()
    } else {
        graphic_as_filters(&eq)
    };
    let text = autoeq::format(Some(eq.effective_preamp_db()), &filters);
    let dialog = gtk::FileDialog::builder()
        .title(tr("Export equalizer"))
        .modal(true)
        .initial_name("ParametricEQ.txt")
        .build();
    let window = context.window.clone();
    let weak = Rc::downgrade(&context);
    glib::spawn_future_local(async move {
        let Ok(file) = dialog.save_future(Some(&window)).await else {
            return;
        };
        let result = file
            .replace_contents_future(
                text.into_bytes(),
                None,
                false,
                gio::FileCreateFlags::REPLACE_DESTINATION,
            )
            .await;
        if let Some(context) = weak.upgrade() {
            match result {
                Ok(_) => context.toast(&tr("Equalizer exported")),
                Err((_, e)) => {
                    context.toast(&fill(&tr("Cannot write the file: {}"), &[e.message()]));
                }
            }
        }
    });
}

/// A popover under "Save preset" that names the bands of the graphic mode in
/// use as a preset.
fn save_preset_popover(editor: &Rc<Editor>, button: &gtk::MenuButton) {
    let popover = gtk::Popover::new();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.set_margin_top(10);
    column.set_margin_bottom(10);
    column.set_margin_start(10);
    column.set_margin_end(10);
    let entry = gtk::Entry::new();
    entry.add_css_class("hk-input");
    entry.set_placeholder_text(Some(&tr("Name of the preset")));
    entry.update_property(&[gtk::accessible::Property::Label(&tr("Name of the preset"))]);
    entry.set_width_chars(24);
    column.append(&entry);
    let save = compact_button(&tr("Save"));
    save.set_halign(gtk::Align::End);
    column.append(&save);
    popover.set_child(Some(&column));
    popover.set_default_widget(Some(&save));
    entry.set_activates_default(true);
    let weak = Rc::downgrade(editor);
    let weak_entry = entry.downgrade();
    let weak_popover = popover.downgrade();
    save.connect_clicked(move |_| {
        let (Some(editor), Some(entry), Some(popover)) =
            (weak.upgrade(), weak_entry.upgrade(), weak_popover.upgrade())
        else {
            return;
        };
        let name = entry.text().trim().to_owned();
        if name.is_empty() {
            entry.grab_focus();
            return;
        }
        let eq = editor.eq();
        let Some(bands) = eq.graphic_gains().map(<[f64]>::to_vec) else {
            return;
        };
        if editor.live.apply(|s| {
            let presets = &mut s.dsp.equalizer.presets;
            presets.retain(|p| p.name != name);
            presets.push(GraphicPreset {
                name: name.clone(),
                bands: bands.clone(),
            });
        }) {
            entry.set_text("");
            popover.popdown();
            if let Some(context) = editor.live.context() {
                context.toast(&fill(&tr("Preset “{}” saved"), &[&name]));
            }
        }
    });
    let weak_entry = entry.downgrade();
    popover.connect_show(move |_| {
        if let Some(entry) = weak_entry.upgrade() {
            entry.grab_focus();
        }
    });
    button.set_popover(Some(&popover));
}

/// The equalizer's editor, and what refreshes it from the stored settings.
/// The preset row goes into `presets`.
pub(super) fn editor(live: &Rc<Live>, presets: &gtk::Box) -> (gtk::Widget, Rc<dyn Fn()>) {
    let eq = live
        .settings()
        .map(|s| s.dsp.equalizer.clone())
        .unwrap_or_default();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 14);

    // The preset row.
    presets.append(&label(&tr("Preset"), &["hk-row-label"]));
    let preset_strings = gtk::StringList::new(&[]);
    let preset_choice = dropdown(&[], 0, 220);
    preset_choice.set_model(Some(&preset_strings));
    preset_choice.set_widget_name("eq-preset");
    preset_choice.update_property(&[gtk::accessible::Property::Label(&tr("Preset"))]);
    presets.append(&preset_choice);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    presets.append(&spacer);
    let import_button = gtk::Button::with_label(&tr("Import AutoEQ file"));
    import_button.add_css_class("hk-btn");
    import_button.add_css_class("small");
    import_button.set_widget_name("eq-import");
    presets.append(&import_button);
    let export_button = gtk::Button::with_label(&tr("Export"));
    export_button.add_css_class("hk-btn");
    export_button.add_css_class("small");
    export_button.set_widget_name("eq-export");
    presets.append(&export_button);
    let save_button = gtk::MenuButton::new();
    save_button.set_label(&tr("Save preset"));
    save_button.set_always_show_arrow(false);
    save_button.add_css_class("hk-menu-btn");
    save_button.set_widget_name("eq-save-preset");
    presets.append(&save_button);

    // The mode row.
    let modes = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let (group, mode_buttons) = segmented(&[
        tr("Parametric"),
        tr("Graphic, 10 bands"),
        tr("Graphic, 31 bands"),
    ]);
    group.update_property(&[gtk::accessible::Property::Label(&tr("Equalizer mode"))]);
    group.set_hexpand(true);
    modes.append(&group);
    modes.append(&label(&tr("Preamp"), &["hk-t2"]));
    let preamp = label(&preamp_text(&eq), &["hk-mono", "hk-strong"]);
    preamp.set_widget_name("eq-preamp");
    modes.append(&preamp);
    column.append(&modes);

    // The graph.
    let graph = EqGraph::default();
    graph.set_widget_name("eq-graph");
    graph.set_settings(&eq);
    graph.update_property(&[gtk::accessible::Property::Label(&tr(
        "Response of the equalizer",
    ))]);
    column.append(&graph);

    // Under the graph.
    let under = gtk::Stack::new();
    under.set_vhomogeneous(false);
    let table = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let count = label("", &["hk-t3", "hk-secondary"]);
    let columns: [gtk::SizeGroup; 6] =
        std::array::from_fn(|_| gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal));
    table.append(&table_header(&count, &columns));
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.set_widget_name("eq-filters");
    rows.update_property(&[gtk::accessible::Property::Label(&tr("Filters"))]);
    table.append(&rows);
    let add = gtk::Button::with_label(&tr("Add filter"));
    add.add_css_class("hk-btn");
    add.add_css_class("text");
    add.set_halign(gtk::Align::Start);
    add.set_margin_top(4);
    add.set_widget_name("eq-add-filter");
    table.append(&add);
    under.add_named(&table, Some("parametric"));
    let help = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let help_text = note(&tr(
        "Drag a handle, or select it and press the up and down arrow keys. A double-click returns it to 0 dB.",
    ));
    help_text.set_hexpand(true);
    help.append(&help_text);
    let flat = compact_button(&tr("Reset to flat"));
    flat.set_widget_name("eq-flat");
    help.append(&flat);
    under.add_named(&help, Some("graphic"));
    column.append(&under);

    let editor = Rc::new(Editor {
        live: live.clone(),
        graph: graph.clone(),
        mode_buttons: mode_buttons.clone(),
        preamp,
        under,
        rows,
        columns,
        count,
        add: add.clone(),
        preset_choice: preset_choice.clone(),
        preset_strings,
        import: import_button.clone(),
        save: save_button.clone(),
        updating: Cell::new(false),
        shown_presets: RefCell::new(Vec::new()),
    });
    editor.refresh();
    editor.rebuild_rows();
    save_preset_popover(&editor, &save_button);

    let weak = Rc::downgrade(&editor);
    bind_segmented(
        &mode_buttons,
        MODES.iter().position(|m| *m == eq.mode).unwrap_or(0),
        move |i| {
            let Some(editor) = weak.upgrade() else {
                return;
            };
            if editor.updating.get() {
                return;
            }
            let mode = MODES[i];
            editor.live.apply(|s| s.dsp.equalizer.mode = mode);
        },
    );
    let weak = Rc::downgrade(&editor);
    graph.connect_band_changed(move |_, _| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        let shown = editor.graph.settings();
        let (ten, thirty_one) = (shown.graphic_10, shown.graphic_31);
        let mode = shown.mode;
        editor.live.apply_soon(move |s| match mode {
            EqMode::Graphic10 => s.dsp.equalizer.graphic_10 = ten,
            EqMode::Graphic31 => s.dsp.equalizer.graphic_31 = thirty_one,
            EqMode::Parametric => {}
        });
    });
    let weak = Rc::downgrade(&editor);
    on_selected(&preset_choice, move |i| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        if editor.updating.get() {
            return;
        }
        let eq = editor.eq();
        let Some((_, octaves)) = preset_entries(&eq).into_iter().nth(i) else {
            // "Custom" keeps the bands.
            return;
        };
        let mode = eq.mode;
        let bands = preset_gains(&octaves, if mode == EqMode::Graphic31 { 31 } else { 10 });
        editor.live.apply(move |s| {
            let eq = &mut s.dsp.equalizer;
            match mode {
                EqMode::Graphic10 => {
                    for (gain, value) in eq.graphic_10.iter_mut().zip(&bands) {
                        *gain = *value;
                    }
                }
                EqMode::Graphic31 => {
                    for (gain, value) in eq.graphic_31.iter_mut().zip(&bands) {
                        *gain = *value;
                    }
                }
                EqMode::Parametric => {}
            }
        });
    });
    let weak = Rc::downgrade(&editor);
    flat.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            let mode = editor.eq().mode;
            editor.live.apply(|s| match mode {
                EqMode::Graphic10 => s.dsp.equalizer.graphic_10 = [0.0; 10],
                EqMode::Graphic31 => s.dsp.equalizer.graphic_31 = [0.0; 31],
                EqMode::Parametric => {}
            });
        }
    });
    let weak = Rc::downgrade(&editor);
    add.connect_clicked(move |_| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        if editor.live.apply(|s| {
            let filters = &mut s.dsp.equalizer.parametric;
            if filters.len() < EqualizerSettings::MAX_FILTERS {
                filters.push(ParametricFilter::default());
            }
        }) {
            editor.rebuild_rows();
            if let Some(last) = editor.rows.last_child() {
                last.child_focus(gtk::DirectionType::TabForward);
            }
        }
    });
    let weak = Rc::downgrade(&editor);
    import_button.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            import(&editor);
        }
    });
    let weak = Rc::downgrade(&editor);
    export_button.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            export(&editor);
        }
    });
    let refresh: Rc<dyn Fn()> = Rc::new(move || editor.refresh());
    (column.upcast(), refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_numbers() {
        assert_eq!(parse_number("105 Hz"), Some(105.0));
        assert_eq!(parse_number("1.2k"), Some(1200.0));
        assert_eq!(parse_number("−2,5 dB"), Some(-2.5));
        assert_eq!(parse_number("+4.5"), Some(4.5));
        assert_eq!(parse_number("loud"), None);
    }

    #[test]
    fn presets_fit_either_graphic_mode() {
        let rock = BuiltInPreset::find("rock").unwrap();
        assert_eq!(preset_gains(&rock.octaves, 10), rock.octaves.to_vec());
        let thirds = preset_gains(&rock.octaves, 31);
        assert_eq!(thirds.len(), 31);
        assert_eq!(preset_gains(&thirds, 10), rock.octaves.to_vec());
        let mut eq = EqualizerSettings {
            mode: EqMode::Graphic31,
            ..EqualizerSettings::default()
        };
        eq.graphic_31.copy_from_slice(&thirds);
        assert_eq!(
            matching_preset(&eq),
            BuiltInPreset::ALL.iter().position(|p| p.id == "rock")
        );
        eq.graphic_31[4] += 1.0;
        assert_eq!(matching_preset(&eq), None);
    }

    #[test]
    fn graphic_bands_export_as_their_corrected_filters() {
        let mut eq = EqualizerSettings {
            mode: EqMode::Graphic10,
            ..EqualizerSettings::default()
        };
        eq.graphic_10 = [6.0; 10];
        let filters = graphic_as_filters(&eq);
        assert_eq!(filters.len(), 10);
        // Corrected for the overlap, so each band is cut below its slider.
        assert!(filters[5].gain_db < 6.0);
        assert!(filters.iter().all(|f| f.kind == FilterKind::Peak));
    }
}
