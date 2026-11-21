// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The convolution editor: a folder of impulse responses and files chosen by
//! hand for single rates, a table of the rates that have a file (and the
//! rate in use), and the mix of the processed sound.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use helikodu_model::settings::{ConvolutionSettings, ImpulseFile};

use super::{Live, info_note, lead};
use crate::backend::dsp::{ImpulseEntry, ImpulseProblem, entry_for};
use crate::dialogs::common::{compact_button, note, row, switch_row, tilde};
use crate::format::{decimal, khz};
use crate::i18n::{fill, tr};
use crate::widgets::label;

/// A rate as `44.1 kHz`.
fn rate_text(hz: u32) -> String {
    fill(&tr("{} kHz"), &[&khz(hz)])
}

/// The rates that have a file, for the chain: `44.1 · 48 · 96 kHz`, beyond
/// three `5 rates`, without any `No file`.
pub(super) fn rates_summary(entries: &[ImpulseEntry]) -> String {
    match entries.len() {
        0 => tr("No file"),
        1..=3 => fill(
            &tr("{} kHz"),
            &[&entries
                .iter()
                .map(|e| khz(e.rate_hz))
                .collect::<Vec<_>>()
                .join(" · ")],
        ),
        n => fill(&tr("{} rates"), &[&n.to_string()]),
    }
}

/// Why a file cannot be used, in words after its name.
fn reason(problem: ImpulseProblem) -> String {
    match problem {
        ImpulseProblem::Missing => tr("is missing"),
        ImpulseProblem::NotWav => tr("cannot be read: it is not a WAV file"),
        ImpulseProblem::TooLong => tr("is longer than 20 seconds"),
        ImpulseProblem::Unreadable => tr("cannot be read"),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The widths of the table's columns.
const RATE: i32 = 92;
const LENGTH: i32 = 60;
const CHANNELS: i32 = 62;
const ACTION: i32 = 58;
const WHERE: i32 = 150;

/// A cell of `width` (or growing with `grow`) holding `widget`.
fn cell(widget: &impl IsA<gtk::Widget>, width: i32, grow: bool) -> gtk::Box {
    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    if width > 0 {
        cell.set_size_request(width, -1);
    }
    cell.set_hexpand(grow);
    cell.append(widget);
    cell
}

fn text(content: &str, classes: &[&str]) -> gtk::Label {
    let l = label(content, classes);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_xalign(0.0);
    l
}

/// What a row of the table asks for.
#[derive(Debug, Clone)]
enum Action {
    /// Choose a file for the rate.
    Choose(u32),
    /// Forget the file chosen by hand for the rate.
    Forget(u32),
}

/// What the table shows: the files, the rate in use, whether a folder is set.
type Shown = (Vec<ImpulseEntry>, Option<u32>, bool);

/// The editor's widgets that change.
struct Editor {
    live: Rc<Live>,
    folder: gtk::Label,
    clear: gtk::Button,
    choose_file: gtk::Button,
    table: gtk::Box,
    rows: gtk::Box,
    under: gtk::Label,
    empty: gtk::Label,
    normalize: gtk::Switch,
    gain: gtk::Scale,
    gain_value: gtk::Label,
    wet: gtk::Scale,
    wet_value: gtk::Label,
    updating: Cell<bool>,
    shown: RefCell<Option<Shown>>,
}

impl Editor {
    fn settings(&self) -> ConvolutionSettings {
        self.live
            .settings()
            .map(|s| s.dsp.convolution.clone())
            .unwrap_or_default()
    }

    fn refresh(self: &Rc<Self>) {
        let convolution = self.settings();
        let facts = self.live.facts();
        self.updating.set(true);
        match &convolution.folder {
            Some(folder) => {
                self.folder.set_text(&tilde(folder));
                self.folder.remove_css_class("hk-t3");
            }
            None => {
                self.folder.set_text(&tr("No folder"));
                self.folder.add_css_class("hk-t3");
            }
        }
        self.clear.set_sensitive(convolution.folder.is_some());
        let empty = !convolution.has_source();
        self.empty.set_visible(empty);
        self.choose_file.set_visible(empty);
        self.table.set_visible(!empty);
        self.under.set_visible(!empty);
        if self.normalize.is_active() != convolution.normalize {
            self.normalize.set_active(convolution.normalize);
        }
        if (self.gain.value() - convolution.gain_db).abs() > 0.05 {
            self.gain.set_value(convolution.gain_db);
        }
        if (self.wet.value() - convolution.wet_percent).abs() > 0.5 {
            self.wet.set_value(convolution.wet_percent);
        }
        self.gain_value.set_text(&gain_text(convolution.gain_db));
        self.wet_value.set_text(&wet_text(convolution.wet_percent));
        self.updating.set(false);
        let key = (
            facts.impulses.clone(),
            facts.rate,
            convolution.folder.is_some(),
        );
        if self.shown.borrow().as_ref() != Some(&key) {
            self.rebuild_rows(&facts.impulses, facts.rate);
            self.shown.replace(Some(key));
        }
    }

    fn rebuild_rows(self: &Rc<Self>, entries: &[ImpulseEntry], in_use: Option<u32>) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        let used = in_use.and_then(|rate| entry_for(entries, rate).map(|e| (rate, e)));
        let mut rates: Vec<u32> = entries.iter().map(|e| e.rate_hz).collect();
        if let Some(rate) = in_use
            && !rates.contains(&rate)
            && used.is_some()
        {
            rates.push(rate);
        }
        rates.sort_unstable();
        for rate in rates {
            let now = in_use == Some(rate);
            let row = match entries.iter().find(|e| e.rate_hz == rate) {
                Some(entry) if entry.problem.is_some() => self.problem_row(entry, now),
                Some(entry) => self.file_row(entry, now),
                None => {
                    let from = used.map_or(rate, |(_, e)| e.rate_hz);
                    self.resampled_row(rate, from)
                }
            };
            self.rows.append(&row);
        }
    }

    fn action_button(self: &Rc<Self>, action: Action) -> gtk::Widget {
        match action {
            Action::Choose(rate) => {
                let button = gtk::Button::with_label(&tr("Choose…"));
                button.add_css_class("hk-btn");
                button.add_css_class("text");
                button.add_css_class("small");
                button.set_widget_name(&format!("ir-choose-{rate}"));
                button.update_property(&[gtk::accessible::Property::Label(&fill(
                    &tr("Choose a file for {}"),
                    &[&rate_text(rate)],
                ))]);
                let weak = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(editor) = weak.upgrade() {
                        editor.choose_file(Some(rate));
                    }
                });
                button.upcast()
            }
            Action::Forget(rate) => {
                let button = gtk::Button::from_icon_name("window-close-symbolic");
                button.add_css_class("hk-small-icon");
                button.add_css_class("flat");
                button.set_size_request(22, 22);
                button.set_halign(gtk::Align::End);
                button.set_widget_name(&format!("ir-forget-{rate}"));
                let words = if self.settings().folder.is_some() {
                    fill(
                        &tr("Use the file of the folder for {} again"),
                        &[&rate_text(rate)],
                    )
                } else {
                    fill(&tr("Remove the file for {}"), &[&rate_text(rate)])
                };
                button.set_tooltip_text(Some(&words));
                button.update_property(&[gtk::accessible::Property::Label(&words)]);
                let live = self.live.clone();
                button.connect_clicked(move |_| {
                    live.apply(|s| s.dsp.convolution.files.retain(|f| f.rate_hz != rate));
                });
                button.upcast()
            }
        }
    }

    fn line(&self) -> gtk::Box {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.add_css_class("hk-ir-row");
        line
    }

    fn file_row(self: &Rc<Self>, entry: &ImpulseEntry, now: bool) -> gtk::Box {
        let line = self.line();
        line.set_widget_name(&format!("ir-row-{}", entry.rate_hz));
        let rate = text(&rate_text(entry.rate_hz), &["hk-strong"]);
        line.append(&cell(&rate, RATE, false));
        line.append(&cell(
            &text(&file_name(&entry.path), &["hk-mono", "hk-ir-file"]),
            0,
            true,
        ));
        let length = entry
            .seconds
            .map(|s| fill(&tr("{} s"), &[&decimal(s, 2)]))
            .unwrap_or_default();
        line.append(&cell(&text(&length, &["hk-mono", "hk-t2"]), LENGTH, false));
        let channels = match entry.channels {
            Some(1) => tr("Mono"),
            Some(_) => tr("Stereo"),
            None => String::new(),
        };
        line.append(&cell(&text(&channels, &["hk-t2"]), CHANNELS, false));
        let mut source = if entry.from_folder {
            tr("From the folder")
        } else {
            tr("Chosen by hand")
        };
        if now {
            source = fill(&tr("{} · in use now"), &[&source]);
        }
        let source = text(&source, &["hk-t2"]);
        source.set_tooltip_text(Some(&entry.path.display().to_string()));
        let where_from = cell(&source, WHERE, false);
        line.append(&where_from);
        let action = if entry.from_folder {
            Action::Choose(entry.rate_hz)
        } else {
            Action::Forget(entry.rate_hz)
        };
        let action = self.action_button(action);
        let end = cell(&action, ACTION, false);
        end.set_halign(gtk::Align::End);
        action.set_hexpand(true);
        action.set_halign(gtk::Align::End);
        line.append(&end);
        line
    }

    fn resampled_row(self: &Rc<Self>, rate: u32, from: u32) -> gtk::Box {
        let line = self.line();
        line.set_widget_name(&format!("ir-row-{rate}"));
        line.append(&cell(&text(&rate_text(rate), &["hk-strong"]), RATE, false));
        let uses = fill(&tr("Uses the {} file, resampled"), &[&rate_text(from)]);
        // The sentence takes the room of the file, its length and channels.
        line.append(&cell(&text(&uses, &["hk-t2"]), 0, true));
        let now = text(&tr("In use now"), &["hk-strong"]);
        let where_from = cell(&now, WHERE, false);
        line.append(&where_from);
        let action = self.action_button(Action::Choose(rate));
        action.set_hexpand(true);
        action.set_halign(gtk::Align::End);
        line.append(&cell(&action, ACTION, false));
        line
    }

    fn problem_row(self: &Rc<Self>, entry: &ImpulseEntry, now: bool) -> gtk::Box {
        let line = self.line();
        line.add_css_class("problem");
        line.set_widget_name(&format!("ir-row-{}", entry.rate_hz));
        let rate = text(&rate_text(entry.rate_hz), &["hk-strong"]);
        rate.set_valign(gtk::Align::Center);
        line.append(&cell(&rate, RATE - 9, false));
        let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
        column.set_hexpand(true);
        column.set_valign(gtk::Align::Center);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::from_icon_name("helikodu-warning-symbolic");
        icon.set_pixel_size(12);
        head.append(&icon);
        head.append(&label(&file_name(&entry.path), &["hk-mono", "hk-ir-file"]));
        let why = text(
            &entry.problem.map(reason).unwrap_or_default(),
            &["hk-medium"],
        );
        why.set_widget_name(&format!("ir-problem-{}", entry.rate_hz));
        head.append(&why);
        column.append(&head);
        if now {
            column.append(&note(&tr(
                "In use now. Convolution is left out until this is fixed.",
            )));
        }
        line.append(&column);
        let choose = compact_button(&tr("Choose…"));
        choose.add_css_class("small");
        choose.set_widget_name(&format!("ir-choose-{}", entry.rate_hz));
        choose.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("Choose a file for {}"),
            &[&rate_text(entry.rate_hz)],
        ))]);
        let weak = Rc::downgrade(self);
        let rate = entry.rate_hz;
        choose.connect_clicked(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.choose_file(Some(rate));
            }
        });
        line.append(&choose);
        line
    }

    /// Asks for a WAV file for `rate` (for the rate of the file when `None`).
    fn choose_file(self: &Rc<Self>, rate: Option<u32>) {
        let Some(context) = self.live.context() else {
            return;
        };
        let title = match rate {
            Some(rate) => fill(&tr("Choose a file for {}"), &[&rate_text(rate)]),
            None => tr("Choose an impulse response"),
        };
        let weak = Rc::downgrade(self);
        crate::chooser::open_file(
            &context.window,
            &title,
            &tr("WAV files"),
            &["*.wav", "*.WAV"],
            move |path| {
                let (Some(editor), Some(path)) = (weak.upgrade(), path) else {
                    return;
                };
                editor.use_file(rate, path);
            },
        );
    }

    fn use_file(&self, rate: Option<u32>, path: PathBuf) {
        let Some(context) = self.live.context() else {
            return;
        };
        let rate = rate
            .or_else(|| context.services.dsp().impulse_rate(&path))
            .or(self.live.facts().rate)
            .unwrap_or(48_000);
        self.live.apply(|s| {
            let files = &mut s.dsp.convolution.files;
            files.retain(|f| f.rate_hz != rate);
            files.push(ImpulseFile {
                rate_hz: rate,
                path: path.clone(),
            });
            files.sort_by_key(|f| f.rate_hz);
        });
        // The same name may hold another file now.
        self.live.reread_impulses();
    }
}

fn gain_text(db: f64) -> String {
    crate::widgets::bipolar_scale::decibels(db)
}

fn wet_text(percent: f64) -> String {
    fill(&tr("{} %"), &[&format!("{percent:.0}")])
}

/// A slider with its title and value above it.
fn slider(title: &str, range: (f64, f64, f64), name: &str) -> (gtk::Box, gtk::Scale, gtk::Label) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let l = label(title, &[]);
    l.set_hexpand(true);
    head.append(&l);
    let value = label("", &["hk-mono", "hk-t2"]);
    head.append(&value);
    root.append(&head);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, range.0, range.1, range.2);
    scale.add_css_class("hk-slider");
    scale.set_draw_value(false);
    scale.set_widget_name(name);
    scale.update_property(&[gtk::accessible::Property::Label(title)]);
    root.append(&scale);
    (root, scale, value)
}

/// The convolution editor and what refreshes it.
pub(super) fn editor(live: &Rc<Live>) -> (gtk::Widget, Rc<dyn Fn()>) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&lead(&tr(
        "Applies an impulse response to everything that plays, for example a room correction measured with a program such as REW. The files are WAV files, one for each sample rate.",
    )));

    // Impulse responses.
    let responses = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let folder_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let field = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    field.add_css_class("hk-ir-folder");
    field.set_hexpand(true);
    field.append(&gtk::Image::from_icon_name("folder-symbolic"));
    let folder = text("", &["hk-mono"]);
    folder.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    folder.set_widget_name("ir-folder");
    field.append(&folder);
    folder_line.append(&field);
    let choose = compact_button(&tr("Choose…"));
    choose.set_widget_name("ir-choose-folder");
    choose.update_property(&[gtk::accessible::Property::Label(&tr(
        "Choose a folder of impulse responses",
    ))]);
    folder_line.append(&choose);
    let choose_file = compact_button(&tr("Choose a file…"));
    choose_file.set_widget_name("ir-choose-file");
    folder_line.append(&choose_file);
    let clear = compact_button(&tr("Clear"));
    clear.set_widget_name("ir-clear");
    folder_line.append(&clear);
    responses.append(&folder_line);
    let empty = label(
        &tr("Choose a folder of impulse responses, or a file for a single sample rate."),
        &["hk-t2"],
    );
    empty.set_wrap(true);
    empty.set_xalign(0.0);
    empty.set_widget_name("ir-empty");
    responses.append(&empty);

    let table = gtk::Box::new(gtk::Orientation::Vertical, 0);
    table.add_css_class("hk-ir-table");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("hk-ir-head");
    let head = |t: &str| {
        let l = label(t, &["hk-micro-label"]);
        l.set_ellipsize(gtk::pango::EllipsizeMode::None);
        l.set_xalign(0.0);
        l
    };
    header.append(&cell(&head(&tr("Sample rate")), RATE, false));
    let file_head = cell(&head(&tr("File")), 0, true);
    header.append(&file_head);
    header.append(&cell(&head(&tr("Length")), LENGTH, false));
    header.append(&cell(&head(&tr("Channels")), CHANNELS, false));
    let where_head = cell(&head(&tr("Where from")), WHERE, false);
    header.append(&where_head);
    header.append(&cell(&text("", &[]), ACTION, false));
    table.append(&header);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.set_widget_name("ir-rows");
    rows.update_property(&[gtk::accessible::Property::Label(&tr("Impulse responses"))]);
    table.append(&rows);
    responses.append(&table);
    let under = note(&tr(
        "A file chosen by hand comes before the folder. Any other sample rate uses the file with the nearest rate, resampled.",
    ));
    responses.append(&under);
    column.append(&row(&tr("Impulse responses"), None, &responses));

    // The mix.
    let mix = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let (normalize_row, normalize) = switch_row(
        &tr("Normalize"),
        Some(&tr(
            "The loudest frequency is brought to 0 dB, so the response boosts nothing.",
        )),
    );
    normalize.set_widget_name("ir-normalize");
    mix.append(&normalize_row);
    let sliders = gtk::Box::new(gtk::Orientation::Horizontal, 28);
    sliders.set_homogeneous(true);
    let (gain_box, gain, gain_value) = slider(
        &tr("Gain"),
        (
            *ConvolutionSettings::GAIN_RANGE.start(),
            *ConvolutionSettings::GAIN_RANGE.end(),
            0.5,
        ),
        "ir-gain",
    );
    sliders.append(&gain_box);
    let (wet_box, wet, wet_value) = slider(
        &tr("Share of the processed sound"),
        (0.0, 100.0, 1.0),
        "ir-wet",
    );
    sliders.append(&wet_box);
    mix.append(&sliders);
    column.append(&row(&tr("Mix"), None, &mix));
    column.append(&info_note(&tr(
        "Convolution adds about 11 ms of delay at any sample rate.",
    )));

    let editor = Rc::new(Editor {
        live: live.clone(),
        folder,
        clear: clear.clone(),
        choose_file: choose_file.clone(),
        table,
        rows,
        under,
        empty,
        normalize: normalize.clone(),
        gain: gain.clone(),
        gain_value,
        wet: wet.clone(),
        wet_value,
        updating: Cell::new(false),
        shown: RefCell::new(None),
    });

    let weak = Rc::downgrade(&editor);
    choose.connect_clicked(move |_| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        let Some(context) = editor.live.context() else {
            return;
        };
        let start = editor.settings().folder;
        let weak = Rc::downgrade(&editor);
        crate::chooser::select_folder(
            &context.window,
            &tr("Choose a folder of impulse responses"),
            None,
            start.as_deref(),
            move |path| {
                let (Some(editor), Some(path)) = (weak.upgrade(), path) else {
                    return;
                };
                editor
                    .live
                    .apply(|s| s.dsp.convolution.folder = Some(path.clone()));
                editor.live.reread_impulses();
            },
        );
    });
    let weak = Rc::downgrade(&editor);
    choose_file.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            editor.choose_file(None);
        }
    });
    let weak = Rc::downgrade(&editor);
    clear.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            editor.live.apply(|s| s.dsp.convolution.folder = None);
        }
    });
    let weak = Rc::downgrade(&editor);
    normalize.connect_active_notify(move |switch| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        if editor.updating.get() {
            return;
        }
        let on = switch.is_active();
        if !editor.live.apply(|s| s.dsp.convolution.normalize = on) {
            switch.set_active(!on);
        }
    });
    let weak = Rc::downgrade(&editor);
    gain.connect_value_changed(move |scale| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        let db = (scale.value() * 2.0).round() / 2.0;
        editor.gain_value.set_text(&gain_text(db));
        if !editor.updating.get() {
            editor
                .live
                .apply_soon(move |s| s.dsp.convolution.gain_db = db);
        }
    });
    let weak = Rc::downgrade(&editor);
    wet.connect_value_changed(move |scale| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        let percent = scale.value().round();
        editor.wet_value.set_text(&wet_text(percent));
        if !editor.updating.get() {
            editor
                .live
                .apply_soon(move |s| s.dsp.convolution.wet_percent = percent);
        }
    });
    editor.refresh();
    let refresh: Rc<dyn Fn()> = Rc::new(move || editor.refresh());
    (column.upcast(), refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rate_hz: u32) -> ImpulseEntry {
        ImpulseEntry {
            rate_hz,
            path: format!("/ir/{rate_hz}.wav").into(),
            from_folder: true,
            seconds: Some(0.74),
            channels: Some(2),
            problem: None,
        }
    }

    #[test]
    fn the_chain_names_the_rates() {
        assert_eq!(rates_summary(&[]), "No file");
        let three = [entry(44_100), entry(48_000), entry(96_000)];
        assert_eq!(rates_summary(&three), "44.1 · 48 · 96 kHz");
        let five: Vec<ImpulseEntry> = [44_100, 48_000, 88_200, 96_000, 192_000]
            .into_iter()
            .map(entry)
            .collect();
        assert_eq!(rates_summary(&five), "5 rates");
        assert_eq!(
            reason(ImpulseProblem::NotWav),
            "cannot be read: it is not a WAV file"
        );
    }
}
