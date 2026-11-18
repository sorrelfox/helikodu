// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Convert: the presets at the left; Format, Quality, Sample rate and Channels;
//! where the files go and how they are named; the table of tracks with where
//! each stands; and the footer with the expected size and "Convert N tracks",
//! the progress and "Stop" while it works, and "Open folder" afterwards.
//!
//! The conversion runs in the background (see `jobs`) and goes on when the
//! window is closed. Files already in the target format are copied as they are.
//! A single click selects a track, a double click or Enter plays it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::jobs::{self, JobKind, JobView, RowState};
use super::tool_window::{
    Frame, GAP, cell, footer, frame, primary_button, shown_path, side_row, table_header,
};
use crate::backend::export::{
    CharacterSet, ConvertRow, EncodeFormat, ExportServices, ExportState, JobOptions, NamingPattern,
    PlannedAction, Preset, PresetQuality,
};
use crate::backend::{Backend, PlayerRequest, TrackKey, TrackQuery, TrackRow};
use crate::dialogs::common::{compact_button, dropdown, find_open, on_selected};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "converter";

/// The width of the From and To columns.
const FORMAT_COLUMN: i32 = 100;
/// The width of the Result column.
const RESULT_COLUMN: i32 = 200;
thread_local! {
    /// The rows of the conversion that runs or ran last, for a window opened
    /// again while it runs.
    static JOB_PLAN: RefCell<(Vec<ConvertRow>, Preset)> =
        RefCell::new((Vec::new(), Preset::builtins().remove(0)));
}

/// The files worked on at a time, as the dropdown offers them.
const WORKERS: [usize; 4] = [1, 2, 4, 8];

/// The name of a built-in preset, or the user's name.
pub(crate) fn preset_name(preset: &Preset) -> String {
    match preset.id.as_str() {
        "opus-128" => "Opus 128".into(),
        "opus-192" => "Opus 192".into(),
        "mp3-v0" => "MP3 V0".into(),
        "aac-256" => "AAC 256".into(),
        "flac" => "FLAC".into(),
        "alac" => "ALAC".into(),
        "wav-cd" => tr("WAV 16-bit / 44.1 kHz"),
        _ if preset.name.is_empty() => preset_label(preset),
        _ => preset.name.clone(),
    }
}

/// One line on what a preset is for.
fn preset_line(preset: &Preset) -> String {
    match preset.id.as_str() {
        "opus-128" => tr("For phones · about 1 MB a minute"),
        "opus-192" => tr("Close to the original"),
        "mp3-v0" => tr("For old players and car stereos"),
        "aac-256" => tr("For Apple devices"),
        "flac" => tr("Lossless · level 5"),
        "alac" => tr("Lossless for Apple devices"),
        "wav-cd" => tr("For burning or editing"),
        _ => preset_label(preset),
    }
}

/// The short form of a preset's format: `Opus 128`, `MP3 V0`, `FLAC`.
pub(crate) fn preset_label(preset: &Preset) -> String {
    let format = preset.encode_format();
    match preset.quality {
        PresetQuality::Bitrate { kbps } => format!("{} {kbps}", format.name()),
        PresetQuality::Vbr { level } => format!("{} V{level}", format.name()),
        PresetQuality::Lossless { .. } if format == EncodeFormat::Wav => {
            format!("WAV {}", preset.bits.unwrap_or(16))
        }
        PresetQuality::Lossless { .. } => format.name().to_owned(),
    }
}

/// A choice of the Quality dropdown.
#[derive(Debug, Clone, Copy, PartialEq)]
struct QualityChoice {
    quality: PresetQuality,
    bits: Option<u8>,
}

fn quality_choices(format: EncodeFormat) -> Vec<(String, QualityChoice)> {
    let kbps = |list: &[u32]| {
        list.iter()
            .map(|&k| {
                (
                    fill(&tr("{} kbps"), &[&k.to_string()]),
                    QualityChoice {
                        quality: PresetQuality::Bitrate { kbps: k },
                        bits: None,
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    match format {
        EncodeFormat::Opus => kbps(&[64, 96, 128, 160, 192, 256]),
        EncodeFormat::Vorbis => kbps(&[96, 128, 160, 192, 256, 320]),
        EncodeFormat::Aac => kbps(&[128, 192, 256, 320]),
        EncodeFormat::Mp3 => {
            let mut out: Vec<(String, QualityChoice)> = [0u8, 2, 4]
                .iter()
                .map(|&level| {
                    (
                        fill(&tr("V{} (variable)"), &[&level.to_string()]),
                        QualityChoice {
                            quality: PresetQuality::Vbr { level },
                            bits: None,
                        },
                    )
                })
                .collect();
            out.extend(kbps(&[128, 192, 256, 320]));
            out
        }
        EncodeFormat::Flac => [0u8, 5, 8]
            .iter()
            .map(|&level| {
                (
                    fill(&tr("Level {}"), &[&level.to_string()]),
                    QualityChoice {
                        quality: PresetQuality::Lossless { level: Some(level) },
                        bits: None,
                    },
                )
            })
            .collect(),
        EncodeFormat::Wav => [16u8, 24]
            .iter()
            .map(|&bits| {
                (
                    fill(&tr("{}-bit"), &[&bits.to_string()]),
                    QualityChoice {
                        quality: PresetQuality::Lossless { level: None },
                        bits: Some(bits),
                    },
                )
            })
            .collect(),
        _ => vec![(
            tr("Lossless"),
            QualityChoice {
                quality: PresetQuality::Lossless { level: None },
                bits: None,
            },
        )],
    }
}

fn rate_choices(format: EncodeFormat) -> Vec<(String, Option<u32>)> {
    if format == EncodeFormat::Opus {
        return vec![(tr("48 kHz, as Opus needs"), Some(48_000))];
    }
    let mut out = vec![(tr("As the source"), None)];
    let rates: &[u32] = if format == EncodeFormat::Mp3 {
        &[44_100, 48_000]
    } else {
        &[44_100, 48_000, 88_200, 96_000]
    };
    for &rate in rates {
        out.push((fill(&tr("{} kHz"), &[&format::khz(rate)]), Some(rate)));
    }
    out
}

fn channel_choices() -> Vec<(String, Option<u16>)> {
    vec![
        (tr("As the source"), None),
        (tr("Stereo"), Some(2)),
        (tr("Mono"), Some(1)),
    ]
}

fn style_of(id: &str) -> CharacterSet {
    CharacterSet::from_id(id).unwrap_or_default()
}

struct Converter {
    window: gtk::Window,
    backend: Backend,
    export: Arc<dyn ExportServices>,
    state: RefCell<ExportState>,
    encoders: Vec<(EncodeFormat, Option<String>)>,
    current: RefCell<Preset>,
    selection_only: bool,
    tracks: RefCell<Vec<TrackKey>>,
    info: RefCell<HashMap<TrackKey, TrackRow>>,
    plan: RefCell<Vec<ConvertRow>>,
    list: gtk::ListBox,
    title: gtk::Label,
    header_summary: gtk::Label,
    format: gtk::DropDown,
    quality: gtk::DropDown,
    rate: gtk::DropDown,
    channels: gtk::DropDown,
    folder: gtk::Entry,
    pattern: gtk::Entry,
    pattern_note: gtk::Label,
    copy_tags: gtk::CheckButton,
    replace: gtk::CheckButton,
    workers: gtk::DropDown,
    store: gio::ListStore,
    stack: gtk::Stack,
    empty: gtk::Label,
    summary: gtk::Label,
    meter: gtk::ProgressBar,
    primary: gtk::Button,
    stop: gtk::Button,
    open_folder: gtk::Button,
    filling: Cell<bool>,
    generation: Cell<u64>,
}

impl Converter {
    fn presets(&self) -> Vec<Preset> {
        self.state.borrow().all_presets()
    }

    fn save(&self) {
        let state = self.state.borrow().clone();
        let _ = self.export.save_state(&state);
    }

    /// The preset of the user's own on `row`.
    fn own_preset(&self, row: &gtk::ListBoxRow) -> Option<Preset> {
        let name = row.widget_name();
        let id = name.strip_prefix("preset-")?;
        self.state
            .borrow()
            .presets
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    /// Asks for a new name of the preset on `row`.
    fn rename_preset(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let Some(preset) = self.own_preset(row) else {
            return;
        };
        let weak = Rc::downgrade(self);
        crate::window::playlists::ask_name(
            row.upcast_ref(),
            &preset_name(&preset),
            &tr("Preset name"),
            move |name| {
                let Some(this) = weak.upgrade() else { return };
                if let Some(own) = this
                    .state
                    .borrow_mut()
                    .presets
                    .iter_mut()
                    .find(|p| p.id == preset.id)
                {
                    own.name = name;
                }
                this.save();
                this.fill_presets();
                if this.current.borrow().id == preset.id {
                    let current = this.presets().into_iter().find(|p| p.id == preset.id);
                    if let Some(current) = current {
                        this.title.set_text(&preset_name(&current));
                        this.current.replace(current);
                    }
                }
            },
        );
    }

    /// Asks before deleting the preset on `row`.
    fn delete_preset(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let Some(preset) = self.own_preset(row) else {
            return;
        };
        let weak = Rc::downgrade(self);
        crate::tools::confirm_delete(
            &self.window,
            &fill(&tr("Delete the preset “{}”?"), &[&preset_name(&preset)]),
            &tr("Files converted with it stay as they are."),
            move || {
                let Some(this) = weak.upgrade() else { return };
                let first = {
                    let mut state = this.state.borrow_mut();
                    state.presets.retain(|p| p.id != preset.id);
                    if state.converter.preset == preset.id {
                        let first = state.all_presets().into_iter().next();
                        if let Some(first) = &first {
                            state.converter.preset = first.id.clone();
                        }
                        first
                    } else {
                        None
                    }
                };
                this.save();
                this.fill_presets();
                if let Some(first) = first {
                    this.show_preset(&first);
                    this.replan();
                }
            },
        );
    }

    /// Fills the left pane.
    fn fill_presets(self: &Rc<Self>) {
        crate::widgets::remove_rows(&self.list);
        let chosen = self.state.borrow().converter.preset.clone();
        let mut select = None;
        for preset in self.presets() {
            let (row, _) = side_row(&preset_name(&preset), &preset_line(&preset));
            row.set_widget_name(&format!("preset-{}", preset.id));
            if preset.id == chosen {
                select = Some(row.clone());
            }
            self.list.append(&row);
        }
        let new_row = gtk::ListBoxRow::new();
        new_row.set_widget_name("new-preset");
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.append(&gtk::Image::from_icon_name("list-add-symbolic"));
        content.append(&label(&tr("New preset"), &["hk-t2"]));
        new_row.set_child(Some(&content));
        new_row.update_property(&[gtk::accessible::Property::Label(&tr("New preset"))]);
        new_row.add_css_class("hk-new-row");
        self.list.append(&new_row);
        self.filling.set(true);
        self.list.select_row(select.as_ref());
        self.filling.set(false);
    }

    /// Shows `preset` in the options.
    fn show_preset(self: &Rc<Self>, preset: &Preset) {
        self.current.replace(preset.clone());
        self.title.set_text(&preset_name(preset));
        self.filling.set(true);
        let format = preset.encode_format();
        let index = EncodeFormat::ALL
            .iter()
            .position(|f| *f == format)
            .unwrap_or(0);
        self.format.set_selected(index as u32);
        self.fill_dependent(preset);
        self.filling.set(false);
    }

    /// Fills Quality and Sample rate for the format of `preset` and selects its
    /// values.
    fn fill_dependent(&self, preset: &Preset) {
        let format = preset.encode_format();
        let qualities = quality_choices(format);
        let names: Vec<String> = qualities.iter().map(|(n, _)| n.clone()).collect();
        self.quality.set_model(Some(&gtk::StringList::new(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        )));
        let q = qualities
            .iter()
            .position(|(_, c)| {
                c.quality == preset.quality && (c.bits.is_none() || c.bits == preset.bits)
            })
            .unwrap_or(0);
        self.quality.set_selected(q as u32);
        let rates = rate_choices(format);
        let names: Vec<String> = rates.iter().map(|(n, _)| n.clone()).collect();
        self.rate.set_model(Some(&gtk::StringList::new(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        )));
        let r = rates
            .iter()
            .position(|(_, rate)| *rate == preset.sample_rate)
            .unwrap_or(0);
        self.rate.set_selected(r as u32);
        let c = channel_choices()
            .iter()
            .position(|(_, ch)| *ch == preset.channels)
            .unwrap_or(0);
        self.channels.set_selected(c as u32);
    }

    /// Reads the options into the working preset.
    fn read_options(self: &Rc<Self>) {
        if self.filling.get() {
            return;
        }
        let mut preset = self.current.borrow().clone();
        let format = EncodeFormat::ALL
            .get(self.format.selected() as usize)
            .copied()
            .unwrap_or(EncodeFormat::Opus);
        if format != preset.encode_format() {
            preset.format = format.id().to_owned();
            let first = quality_choices(format)[0].1;
            preset.quality = first.quality;
            preset.bits = first.bits;
            preset.sample_rate = rate_choices(format)[0].1;
            self.filling.set(true);
            self.fill_dependent(&preset);
            self.filling.set(false);
        }
        if let Some((_, choice)) = quality_choices(format).get(self.quality.selected() as usize) {
            preset.quality = choice.quality;
            if format.is_lossless() {
                preset.bits = choice.bits.or(if format == EncodeFormat::Wav {
                    Some(16)
                } else {
                    None
                });
            }
        }
        if let Some((_, rate)) = rate_choices(format).get(self.rate.selected() as usize) {
            preset.sample_rate = *rate;
        }
        if let Some((_, ch)) = channel_choices().get(self.channels.selected() as usize) {
            preset.channels = *ch;
        }
        self.current.replace(preset);
        self.replan();
    }

    /// The pattern as typed, or `None` with the problem shown under it.
    fn pattern(&self) -> Option<NamingPattern> {
        match NamingPattern::parse(&self.pattern.text()) {
            Ok(p) => {
                self.pattern_note.set_visible(false);
                Some(p)
            }
            Err(e) => {
                self.pattern_note
                    .set_text(&crate::tools::naming_problem(&e));
                self.pattern_note.set_visible(true);
                None
            }
        }
    }

    fn folder_path(&self) -> Option<PathBuf> {
        let text = self.folder.text().trim().to_owned();
        if text.is_empty() {
            return None;
        }
        Some(match text.strip_prefix("~/") {
            Some(rest) => glib::home_dir().join(rest),
            None if text == "~" => glib::home_dir(),
            None => PathBuf::from(text),
        })
    }

    /// Stores the choices and plans the conversion again.
    fn replan(self: &Rc<Self>) {
        if self.filling.get() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            let c = &mut state.converter;
            c.preset = self.current.borrow().id.clone();
            c.folder = self.folder_path();
            c.pattern = self.pattern.text().to_string();
            c.copy_tags = self.copy_tags.is_active();
            c.replace = self.replace.is_active();
            c.workers = WORKERS
                .get(self.workers.selected() as usize)
                .copied()
                .unwrap_or(4);
        }
        self.save();
        if jobs::current().is_some_and(|j| j.kind == JobKind::Convert) {
            // The table shows the job until it is cleared.
            return;
        }
        let (Some(pattern), Some(folder)) = (self.pattern(), self.folder_path()) else {
            self.plan.borrow_mut().clear();
            self.rebuild();
            return;
        };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let export = self.export.clone();
        let tracks = self.tracks.borrow().clone();
        let preset = self.current.borrow().clone();
        let style = style_of(&self.state.borrow().converter.names);
        let replace = self.replace.is_active();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                export.plan_conversion(&tracks, &preset, &folder, &pattern, style, replace)
            })
            .await;
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            match result {
                Ok(Ok(plan)) => {
                    this.plan.replace(plan);
                }
                Ok(Err(error)) => {
                    this.plan.borrow_mut().clear();
                    this.empty.set_text(&error.0);
                }
                Err(_) => this.plan.borrow_mut().clear(),
            }
            this.rebuild();
        });
    }

    fn rebuild(&self) {
        let n = self.plan.borrow().len();
        let items: Vec<glib::BoxedAnyObject> = (0..n).map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &items);
        self.stack
            .set_visible_child_name(if n == 0 { "empty" } else { "table" });
        if n == 0 && self.empty.text().is_empty() {
            self.empty.set_text(&tr("There are no tracks to convert."));
        }
        let seconds: f64 = self.plan.borrow().iter().map(|r| r.seconds).sum();
        let tracks = self.tracks.borrow().len();
        let mut summary = fill(
            &ntr("{} track", "{} tracks", tracks),
            &[&format::count(tracks)],
        );
        if seconds > 0.0 {
            summary = fill(
                &tr("{} · {}"),
                &[
                    &summary,
                    &format::duration_hours(std::time::Duration::from_secs_f64(seconds)),
                ],
            );
        }
        if self.selection_only {
            summary = fill(&tr("{} · the selection"), &[&summary]);
        }
        self.header_summary.set_text(&summary);
        self.update_footer(jobs::current().as_ref());
    }

    /// The rows the button converts or copies.
    fn to_do(&self) -> usize {
        self.plan
            .borrow()
            .iter()
            .filter(|r| r.action != PlannedAction::AlreadyThere)
            .count()
    }

    fn update_footer(&self, job: Option<&JobView>) {
        let job = job.filter(|j| j.kind == JobKind::Convert);
        let format_ok = self
            .encoders
            .iter()
            .find(|(f, _)| *f == self.current.borrow().encode_format())
            .and_then(|(_, why)| why.clone());
        match job {
            Some(job) if job.is_running() => {
                self.meter.set_visible(true);
                self.meter.set_fraction(job.fraction());
                let mut text = fill(
                    &tr("{} of {} · {} written"),
                    &[
                        &format::count(job.done()),
                        &format::count(job.rows.len()),
                        &format::size(job.bytes),
                    ],
                );
                if let Some(left) = job.seconds_left() {
                    text = fill(&tr("{} · {}"), &[&text, &time_left(left)]);
                }
                self.summary.set_text(&text);
                self.primary.set_visible(false);
                self.stop.set_visible(true);
                self.open_folder.set_visible(false);
            }
            Some(job) => {
                self.meter.set_visible(false);
                let summary = job.finished.clone().unwrap_or_default();
                let mut text = fill(
                    &ntr("{} file written", "{} files written", summary.written),
                    &[&format::count(summary.written)],
                );
                text = fill(&tr("{} · {}"), &[&text, &format::size(summary.bytes)]);
                if summary.failed > 0 {
                    text = fill(
                        &tr("{} · {}"),
                        &[
                            &text,
                            &fill(
                                &ntr("{} failed", "{} failed", summary.failed),
                                &[&format::count(summary.failed)],
                            ),
                        ],
                    );
                }
                if summary.cancelled {
                    text = fill(&tr("{} · stopped"), &[&text]);
                }
                self.summary.set_text(&text);
                self.primary.set_visible(true);
                self.primary.set_label(&tr("Done"));
                self.primary.set_sensitive(true);
                self.stop.set_visible(false);
                self.open_folder.set_visible(true);
            }
            None => {
                self.meter.set_visible(false);
                let plan = self.plan.borrow();
                let bytes: u64 = plan.iter().map(|r| r.expected_bytes).sum();
                let there = plan
                    .iter()
                    .filter(|r| r.action == PlannedAction::AlreadyThere)
                    .count();
                let mut text = fill(&tr("About {} in all"), &[&format::size(bytes)]);
                if there > 0 {
                    text = fill(
                        &tr("{} · {}"),
                        &[
                            &text,
                            &fill(
                                &ntr("{} is there already", "{} are there already", there),
                                &[&format::count(there)],
                            ),
                        ],
                    );
                }
                if let Some(why) = &format_ok {
                    text = why.clone();
                }
                drop(plan);
                self.summary.set_text(&text);
                let n = self.to_do();
                self.primary.set_visible(true);
                self.primary.set_label(&fill(
                    &ntr("Convert {} track", "Convert {} tracks", n),
                    &[&format::count(n)],
                ));
                self.primary.set_sensitive(n > 0 && format_ok.is_none());
                self.stop.set_visible(false);
                self.open_folder.set_visible(false);
            }
        }
    }

    fn start(self: &Rc<Self>) {
        if jobs::current().is_some_and(|j| !j.is_running() && j.kind == JobKind::Convert) {
            // "Done": back to the plan.
            jobs::clear();
            self.replan();
            return;
        }
        let rows = self.plan.borrow().clone();
        if rows.is_empty() {
            return;
        }
        let preset = self.current.borrow().clone();
        let workers = WORKERS
            .get(self.workers.selected() as usize)
            .copied()
            .unwrap_or(4);
        let options = JobOptions {
            workers,
            replace: self.replace.is_active(),
            copy_cover: self.copy_tags.is_active(),
        };
        let copy_tags = self.copy_tags.is_active();
        let export = self.export.clone();
        let n = rows.len();
        JOB_PLAN.with(|p| p.replace((rows.clone(), preset.clone())));
        if let Err(error) = jobs::start(JobKind::Convert, String::new(), n, move |events| {
            export.start_conversion(&rows, &preset, options, copy_tags, events)
        }) {
            self.summary.set_text(&error.0);
        }
    }

    fn play(&self, position: u32) {
        let Some(track) = self.plan.borrow().get(position as usize).map(|r| r.track) else {
            return;
        };
        self.backend.send(PlayerRequest::PlayTracks {
            tracks: vec![track],
            start: 0,
            title: tr("Convert"),
            shuffle: false,
        });
    }

    fn bind(&self, row: &gtk::Box, item: &gtk::ListItem, index: usize) {
        let plan = self.plan.borrow();
        let Some(entry) = plan.get(index) else { return };
        let info = self.info.borrow();
        let track = info.get(&entry.track);
        let cells = children(row.upcast_ref());
        let [name, from, to, result] = cells.as_slice() else {
            return;
        };
        let title = track.map(|t| t.title.clone()).unwrap_or_default();
        let artist = track.map(|t| t.artist.clone()).unwrap_or_default();
        if let Some(name) = name.downcast_ref::<gtk::Label>() {
            let markup = if artist.is_empty() {
                glib::markup_escape_text(&title).to_string()
            } else {
                format!(
                    "{} <span alpha=\"60%\">· {}</span>",
                    glib::markup_escape_text(&title),
                    glib::markup_escape_text(&artist)
                )
            };
            name.set_markup(&markup);
        }
        let from_text = track.map(|t| format::short(&t.format)).unwrap_or_default();
        set_label(from, &from_text);
        let to_text = match entry.action {
            PlannedAction::Copy => from_text.clone(),
            _ => preset_label(&self.current.borrow()),
        };
        set_label(to, &to_text);
        let job = jobs::current().filter(|j| j.kind == JobKind::Convert);
        let state = job.as_ref().and_then(|j| j.rows.get(index).cloned());
        let (text, fraction) = match (state, entry.action) {
            (Some(RowState::Working(f)), _) => (format!("{:.0} %", f * 100.0), Some(f)),
            (Some(RowState::Written(bytes)), _) => (format!("✓  {}", format::size(bytes)), None),
            (Some(RowState::Skipped), _) | (None, PlannedAction::AlreadyThere) => {
                (tr("Already there"), None)
            }
            (Some(RowState::Failed(why)), _) => (fill(&tr("Failed: {}"), &[&why]), None),
            (Some(RowState::Waiting), _) if job.as_ref().is_some_and(JobView::is_running) => {
                (tr("Waiting"), None)
            }
            (_, PlannedAction::Copy) => (tr("Will be copied as it is"), None),
            _ => (String::new(), None),
        };
        if let Some(result) = result.downcast_ref::<gtk::Box>() {
            let (Some(meter), Some(label)) = (
                result.first_child().and_downcast::<gtk::ProgressBar>(),
                result.last_child().and_downcast::<gtk::Label>(),
            ) else {
                return;
            };
            meter.set_visible(fraction.is_some());
            meter.set_fraction(f64::from(fraction.unwrap_or(0.0)));
            label.set_text(&text);
            label.set_tooltip_text(Some(&shown_path(&entry.target)));
        }
        item.set_accessible_label(&fill(
            &tr("{}, {}, from {} to {}, {}"),
            &[&title, &artist, &from_text, &to_text, &text],
        ));
    }
}

/// "about 2 minutes left", "less than a minute left".
fn time_left(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as usize;
    if minutes < 1 {
        tr("less than a minute left")
    } else if minutes < 90 {
        fill(
            &ntr("about {} minute left", "about {} minutes left", minutes),
            &[&minutes.to_string()],
        )
    } else {
        let hours = (minutes as f64 / 60.0).round() as usize;
        fill(
            &ntr("about {} hour left", "about {} hours left", hours),
            &[&hours.to_string()],
        )
    }
}

fn children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.first_child();
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

fn row_widget() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    row.add_css_class("hk-convert-row");
    let name = cell(-1, &[], false);
    name.set_max_width_chars(1);
    row.append(&name);
    row.append(&cell(FORMAT_COLUMN, &["hk-mono-meta", "hk-t3"], false));
    row.append(&cell(FORMAT_COLUMN, &["hk-mono-meta", "hk-t3"], false));
    let result = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    result.set_size_request(RESULT_COLUMN, -1);
    result.set_hexpand(false);
    let meter = gtk::ProgressBar::new();
    meter.add_css_class("hk-tool-meter");
    meter.set_size_request(84, -1);
    meter.set_valign(gtk::Align::Center);
    meter.set_visible(false);
    result.append(&meter);
    let text = label("", &["hk-t2"]);
    result.append(&text);
    row.append(&result);
    row
}

/// A labelled control: the label above it.
fn labelled(text: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let caption = label(text, &["hk-field-label"]);
    column.append(&caption);
    control
        .upcast_ref::<gtk::Widget>()
        .update_relation(&[gtk::accessible::Relation::LabelledBy(&[
            caption.upcast_ref()
        ])]);
    column.append(control);
    column
}

/// Opens Convert over `main` for the whole library, or brings it to the front.
pub(crate) fn open(main: &MainWindow) -> Option<gtk::Window> {
    open_with(main, None)
}

/// Opens Convert for `tracks`, or for the whole library with `None`.
pub(crate) fn open_with(main: &MainWindow, tracks: Option<Vec<TrackKey>>) -> Option<gtk::Window> {
    let parent = main.upcast_ref::<gtk::Window>();
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        return Some(window);
    }
    let backend = main.backend()?;
    let export = backend.services().export();
    let state = export.state();
    let encoders = export.encoders();
    let Frame {
        window,
        list,
        side_note,
        title,
        summary: header_summary,
        main: column,
    } = frame(parent, NAME, &tr("Convert"), &tr("Presets"));
    side_note.set_text(&tr(
        "Converting happens on this computer. The originals are never changed.",
    ));

    // Options.
    let options = gtk::Box::new(gtk::Orientation::Vertical, 14);
    options.add_css_class("hk-tool-options");
    let first = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    first.set_homogeneous(true);
    let format_names: Vec<String> = EncodeFormat::ALL
        .iter()
        .map(|f| {
            match encoders
                .iter()
                .find(|(e, _)| e == f)
                .and_then(|(_, w)| w.clone())
            {
                Some(_) => fill(&tr("{} (no encoder)"), &[f.name()]),
                None => f.name().to_owned(),
            }
        })
        .collect();
    let format_dd = dropdown(&format_names, 0, -1);
    format_dd.set_halign(gtk::Align::Fill);
    format_dd.set_widget_name("format");
    let quality = dropdown(&[String::new()], 0, -1);
    quality.set_halign(gtk::Align::Fill);
    quality.set_widget_name("quality");
    let rate = dropdown(&[String::new()], 0, -1);
    rate.set_halign(gtk::Align::Fill);
    rate.set_widget_name("sample-rate");
    let channel_names: Vec<String> = channel_choices().into_iter().map(|(n, _)| n).collect();
    let channels = dropdown(&channel_names, 0, -1);
    channels.set_halign(gtk::Align::Fill);
    channels.set_widget_name("channels");
    first.append(&labelled(&tr("Format"), &format_dd));
    first.append(&labelled(&tr("Quality"), &quality));
    first.append(&labelled(&tr("Sample rate"), &rate));
    first.append(&labelled(&tr("Channels"), &channels));
    options.append(&first);

    let second = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    second.set_homogeneous(true);
    let save_to = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let folder = gtk::Entry::new();
    folder.add_css_class("hk-input");
    folder.add_css_class("hk-mono");
    folder.set_hexpand(true);
    folder.set_widget_name("folder");
    folder.set_text(
        &state
            .converter
            .folder
            .as_deref()
            .map(shown_path)
            .unwrap_or_default(),
    );
    folder.set_placeholder_text(Some(&tr("Choose a folder")));
    save_to.append(&folder);
    let choose = compact_button(&tr("Choose…"));
    choose.set_widget_name("choose-folder");
    save_to.append(&choose);
    second.append(&labelled(&tr("Save to"), &save_to));
    let pattern_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let pattern = gtk::Entry::new();
    pattern.add_css_class("hk-input");
    pattern.add_css_class("hk-mono");
    pattern.add_css_class("hk-query");
    pattern.set_widget_name("pattern");
    pattern.set_text(&state.converter.pattern);
    pattern_box.append(&pattern);
    second.append(&labelled(&tr("Folders and names"), &pattern_box));
    options.append(&second);
    let pattern_note = label("", &["hk-query-problem"]);
    pattern_note.set_halign(gtk::Align::End);
    pattern_note.set_visible(false);

    let third = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    let copy_tags = gtk::CheckButton::with_label(&tr("Copy tags and covers"));
    copy_tags.set_active(state.converter.copy_tags);
    copy_tags.set_widget_name("copy-tags");
    third.append(&copy_tags);
    let replace = gtk::CheckButton::with_label(&tr("Replace files that are already there"));
    replace.set_active(state.converter.replace);
    replace.set_widget_name("replace");
    third.append(&replace);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    third.append(&spacer);
    let work_label = label(&tr("Work on"), &["hk-t2"]);
    third.append(&work_label);
    let worker_names: Vec<String> = WORKERS
        .iter()
        .map(|&n| {
            fill(
                &ntr("{} file at a time", "{} files at a time", n),
                &[&n.to_string()],
            )
        })
        .collect();
    let workers = dropdown(
        &worker_names,
        WORKERS
            .iter()
            .position(|&n| n == state.converter.workers)
            .unwrap_or(2),
        160,
    );
    workers.set_widget_name("workers");
    workers.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        work_label.upcast_ref()
    ])]);
    third.append(&workers);
    options.append(&third);
    options.append(&pattern_note);
    column.append(&options);

    column.append(&table_header(&[
        (tr("Track"), -1, false),
        (tr("From"), FORMAT_COLUMN, false),
        (tr("To"), FORMAT_COLUMN, false),
        (tr("Result"), RESULT_COLUMN, false),
    ]));
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let factory = gtk::SignalListItemFactory::new();
    let view = gtk::ListView::new(Some(selection), Some(factory.clone()));
    // Tab leaves the list after the focused row, instead of visiting
    // every row: Tab and Shift+Tab reach the rest of the window.
    view.set_tab_behavior(gtk::ListTabBehavior::Item);
    view.add_css_class("hk-dup-list");
    view.set_widget_name("convert-table");
    view.update_property(&[gtk::accessible::Property::Label(&tr("Tracks to convert"))]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&view));
    let empty = label("", &["hk-t3", "hk-tool-empty"]);
    empty.set_xalign(0.5);
    empty.set_valign(gtk::Align::Start);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&scroller, Some("table"));
    stack.add_named(&empty, Some("empty"));
    column.append(&stack);

    let (bar, summary) = footer();
    let meter = gtk::ProgressBar::new();
    meter.add_css_class("hk-tool-meter");
    meter.set_size_request(180, -1);
    meter.set_valign(gtk::Align::Center);
    meter.set_visible(false);
    bar.prepend(&meter);
    let open_folder = compact_button(&tr("Open folder"));
    open_folder.set_widget_name("open-folder");
    open_folder.set_visible(false);
    bar.append(&open_folder);
    let stop = compact_button(&tr("Stop"));
    stop.set_widget_name("stop");
    stop.set_visible(false);
    bar.append(&stop);
    let primary = primary_button(&tr("Convert"));
    primary.set_widget_name("convert");
    bar.append(&primary);
    column.append(&bar);

    let current = state
        .preset(&state.converter.preset)
        .unwrap_or_else(|| Preset::builtins().remove(0));
    let converter = Rc::new(Converter {
        window: window.clone(),
        backend: backend.clone(),
        export,
        state: RefCell::new(state),
        encoders,
        current: RefCell::new(current.clone()),
        selection_only: tracks.is_some(),
        tracks: RefCell::new(tracks.clone().unwrap_or_default()),
        info: RefCell::new(HashMap::new()),
        plan: RefCell::new(Vec::new()),
        list: list.clone(),
        title,
        header_summary,
        format: format_dd.clone(),
        quality: quality.clone(),
        rate: rate.clone(),
        channels: channels.clone(),
        folder: folder.clone(),
        pattern: pattern.clone(),
        pattern_note,
        copy_tags: copy_tags.clone(),
        replace: replace.clone(),
        workers: workers.clone(),
        store,
        stack,
        empty,
        summary,
        meter,
        primary: primary.clone(),
        stop: stop.clone(),
        open_folder: open_folder.clone(),
        filling: Cell::new(false),
        generation: Cell::new(0),
    });

    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&row_widget()));
        }
    });
    let weak = Rc::downgrade(&converter);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(this), Some(row), Some(object)) = (
            weak.upgrade(),
            item.child().and_downcast::<gtk::Box>(),
            item.item().and_downcast::<glib::BoxedAnyObject>(),
        ) else {
            return;
        };
        let index = *object.borrow::<usize>();
        this.bind(&row, item, index);
    });

    converter.fill_presets();
    converter.show_preset(&current);

    let (weak_rename, weak_delete) = (Rc::downgrade(&converter), Rc::downgrade(&converter));
    let weak_own = Rc::downgrade(&converter);
    crate::tools::edit_rows(
        &list,
        move |row| {
            weak_own
                .upgrade()
                .is_some_and(|this| this.own_preset(row).is_some())
        },
        move |row| {
            if let Some(this) = weak_rename.upgrade() {
                this.rename_preset(row);
            }
        },
        move |row| {
            if let Some(this) = weak_delete.upgrade() {
                this.delete_preset(row);
            }
        },
    );
    // "New preset" acts when activated (a click, Enter), not when selected:
    // moving through the list with the arrow keys must not make presets.
    let weak = Rc::downgrade(&converter);
    list.connect_row_activated(move |list, row| {
        let Some(this) = weak.upgrade() else {
            return;
        };
        if row.widget_name() == "new-preset" {
            let mut preset = this.current.borrow().clone();
            let mut state = this.state.borrow_mut();
            preset.id = state.new_preset_id();
            preset.name = preset_label(&preset);
            state.presets.push(preset.clone());
            state.converter.preset = preset.id.clone();
            drop(state);
            this.save();
            this.fill_presets();
            this.show_preset(&preset);
            this.replan();
            let _ = list;
        }
    });
    let weak = Rc::downgrade(&converter);
    list.connect_row_selected(move |_, row| {
        let (Some(this), Some(row)) = (weak.upgrade(), row) else {
            return;
        };
        if this.filling.get() {
            return;
        }
        let id = row.widget_name();
        let Some(id) = id.strip_prefix("preset-") else {
            return;
        };
        if let Some(preset) = this.presets().into_iter().find(|p| p.id == id) {
            this.show_preset(&preset);
            this.replan();
        }
    });
    for dd in [&format_dd, &quality, &rate, &channels] {
        let weak = Rc::downgrade(&converter);
        on_selected(dd, move |_| {
            if let Some(this) = weak.upgrade() {
                this.read_options();
            }
        });
    }
    let weak = Rc::downgrade(&converter);
    on_selected(&workers, move |_| {
        if let Some(this) = weak.upgrade() {
            this.replan();
        }
    });
    for entry in [&folder, &pattern] {
        let weak = Rc::downgrade(&converter);
        entry.connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.replan();
            }
        });
    }
    for check in [&copy_tags, &replace] {
        let weak = Rc::downgrade(&converter);
        check.connect_toggled(move |_| {
            if let Some(this) = weak.upgrade() {
                this.replan();
            }
        });
    }
    let weak = Rc::downgrade(&converter);
    choose.connect_clicked(move |_| {
        let Some(this) = weak.upgrade() else { return };
        let dialog = gtk::FileDialog::builder()
            .title(tr("Save the converted files in"))
            .modal(true)
            .build();
        if let Some(path) = this.folder_path() {
            dialog.set_initial_folder(Some(&gio::File::for_path(path)));
        }
        let weak = Rc::downgrade(&this);
        dialog.select_folder(Some(&this.window), gio::Cancellable::NONE, move |file| {
            if let (Some(this), Ok(Some(path))) = (weak.upgrade(), file.map(|f| f.path())) {
                this.folder.set_text(&shown_path(&path));
            }
        });
    });
    let weak = Rc::downgrade(&converter);
    primary.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.start();
        }
    });
    let weak = Rc::downgrade(&converter);
    stop.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.export.stop();
        }
    });
    let weak = Rc::downgrade(&converter);
    open_folder.connect_clicked(move |_| {
        let Some(this) = weak.upgrade() else { return };
        if let Some(path) = this.folder_path() {
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
            launcher.launch(Some(&this.window), gio::Cancellable::NONE, |_| {});
        }
    });
    let weak = Rc::downgrade(&converter);
    view.connect_activate(move |_, position| {
        if let Some(this) = weak.upgrade() {
            this.play(position);
        }
    });

    let weak = Rc::downgrade(&converter);
    let listener = jobs::listen(move |job, row| {
        let Some(this) = weak.upgrade() else { return };
        if job.kind != JobKind::Convert {
            return;
        }
        // New items, so that the list binds the rows again.
        match row {
            Some(row) if row < this.store.n_items() as usize => {
                this.store
                    .splice(row as u32, 1, &[glib::BoxedAnyObject::new(row)]);
            }
            _ => {
                let n = this.store.n_items();
                let items: Vec<glib::BoxedAnyObject> =
                    (0..n as usize).map(glib::BoxedAnyObject::new).collect();
                this.store.splice(0, n, &items);
            }
        }
        this.update_footer(Some(job));
    });
    let keep_alive = RefCell::new(Some(converter.clone()));
    window.connect_destroy(move |_| {
        jobs::unlisten(listener);
        keep_alive.borrow_mut().take();
    });

    // The tracks: the selection, or the whole library.
    let weak = Rc::downgrade(&converter);
    glib::spawn_future_local(async move {
        let keys = match tracks {
            Some(keys) => Ok(keys),
            None => {
                backend
                    .query(|l| l.track_keys(&TrackQuery::default()))
                    .await
            }
        };
        let Ok(keys) = keys else { return };
        let query_keys = keys.clone();
        let rows = backend
            .query(move |l| l.track_rows(&query_keys))
            .await
            .unwrap_or_default();
        let Some(this) = weak.upgrade() else { return };
        this.tracks.replace(keys);
        this.info
            .replace(rows.into_iter().map(|r| (r.key, r)).collect());
        // A job that runs keeps its table; the plan is made again afterwards.
        if let Some(job) = jobs::current().filter(|j| j.kind == JobKind::Convert) {
            let (plan, preset) = JOB_PLAN.with(|p| p.borrow().clone());
            if job.rows.len() == plan.len() {
                this.plan.replace(plan);
                this.current.replace(preset);
                this.rebuild();
                return;
            }
            jobs::clear();
        }
        this.replan();
    });
    converter.update_footer(jobs::current().as_ref());
    window.present();
    Some(window)
}
