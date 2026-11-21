// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The compressor's editor: a preset (Night, Gentle or Custom), its six
//! values, the lookahead, a hint while the limiter is off, and the activity
//! the engine reports.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use helikodu_model::settings::{CompressorPreset, CompressorSettings, CompressorValues};

use super::{Live, lead, notice};
use crate::dialogs::common::{bind_segmented, note, row, segmented};
use crate::format::decimal;
use crate::i18n::{fill, tr};
use crate::widgets::label;

const PRESETS: [CompressorPreset; 3] = [
    CompressorPreset::Night,
    CompressorPreset::Gentle,
    CompressorPreset::Custom,
];

/// A level in dB with a true minus: `−30 dB`, `0 dB`.
fn signed_db(value: f64, places: usize) -> String {
    let text = if value < 0.0 {
        format!("−{}", decimal(-value, places))
    } else {
        decimal(value, places)
    };
    fill(&tr("{} dB"), &[&text])
}

/// A ratio: `4 : 1`, `2.5 : 1`.
fn ratio(value: f64) -> String {
    fill(&tr("{} : 1"), &[&super::trim_number(value)])
}

/// A time: `0.5 ms`, `5 ms`, `250 ms`.
fn milliseconds(value: f64) -> String {
    let text = if value < 10.0 {
        super::trim_number(value)
    } else {
        format!("{value:.0}")
    };
    fill(&tr("{} ms"), &[&text])
}

/// The compressor in short, for the chain: `Night`, `Gentle`, or
/// `4 : 1 at −18 dB`.
pub(super) fn summary(compressor: &CompressorSettings) -> String {
    match compressor.preset {
        CompressorPreset::Night => tr("Night"),
        CompressorPreset::Gentle => tr("Gentle"),
        CompressorPreset::Custom => fill(
            &tr("{} at {}"),
            &[
                &ratio(compressor.ratio),
                &signed_db(compressor.threshold_db, 0),
            ],
        ),
    }
}

/// One of the six values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Value {
    Threshold,
    Ratio,
    Attack,
    Release,
    Knee,
    Makeup,
}

impl Value {
    const ALL: [Self; 6] = [
        Self::Threshold,
        Self::Ratio,
        Self::Attack,
        Self::Release,
        Self::Knee,
        Self::Makeup,
    ];

    fn title(self) -> String {
        match self {
            Self::Threshold => tr("Threshold"),
            Self::Ratio => tr("Ratio"),
            Self::Attack => tr("Attack"),
            Self::Release => tr("Release"),
            Self::Knee => tr("Knee"),
            Self::Makeup => tr("Makeup gain"),
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Threshold => "compressor-threshold",
            Self::Ratio => "compressor-ratio",
            Self::Attack => "compressor-attack",
            Self::Release => "compressor-release",
            Self::Knee => "compressor-knee",
            Self::Makeup => "compressor-makeup",
        }
    }

    /// The range of the value.
    fn range(self) -> (f64, f64) {
        let r = match self {
            Self::Threshold => CompressorSettings::THRESHOLD_RANGE,
            Self::Ratio => CompressorSettings::RATIO_RANGE,
            Self::Attack => CompressorSettings::ATTACK_RANGE,
            Self::Release => CompressorSettings::RELEASE_RANGE,
            Self::Knee => CompressorSettings::KNEE_RANGE,
            Self::Makeup => CompressorSettings::MAKEUP_RANGE,
        };
        (*r.start(), *r.end())
    }

    /// Attack and release move on a logarithmic scale.
    fn logarithmic(self) -> bool {
        matches!(self, Self::Attack | Self::Release)
    }

    /// Where the slider stands for `value`.
    fn position(self, value: f64) -> f64 {
        if self.logarithmic() {
            value.max(1e-3).log10()
        } else {
            value
        }
    }

    /// The value at slider position `position`, rounded as it is stored.
    fn value(self, position: f64) -> f64 {
        let (min, max) = self.range();
        let value = match self {
            Self::Attack => {
                let ms = 10f64.powf(position);
                if ms < 10.0 {
                    (ms * 10.0).round() / 10.0
                } else {
                    ms.round()
                }
            }
            Self::Release => (10f64.powf(position) / 5.0).round() * 5.0,
            Self::Ratio | Self::Makeup => (position * 2.0).round() / 2.0,
            Self::Threshold | Self::Knee => position.round(),
        };
        value.clamp(min, max)
    }

    fn text(self, value: f64) -> String {
        match self {
            Self::Threshold => signed_db(value, 0),
            Self::Ratio => ratio(value),
            Self::Attack | Self::Release => milliseconds(value),
            Self::Knee => fill(&tr("{} dB"), &[&super::trim_number(value)]),
            Self::Makeup => fill(&tr("{} dB"), &[&format!("+{}", super::trim_number(value))]),
        }
    }

    fn get(self, values: &CompressorValues) -> f64 {
        match self {
            Self::Threshold => values.threshold_db,
            Self::Ratio => values.ratio,
            Self::Attack => values.attack_ms,
            Self::Release => values.release_ms,
            Self::Knee => values.knee_db,
            Self::Makeup => values.makeup_db,
        }
    }

    fn set(self, compressor: &mut CompressorSettings, value: f64) {
        match self {
            Self::Threshold => compressor.threshold_db = value,
            Self::Ratio => compressor.ratio = value,
            Self::Attack => compressor.attack_ms = value,
            Self::Release => compressor.release_ms = value,
            Self::Knee => compressor.knee_db = value,
            Self::Makeup => compressor.makeup_db = value,
        }
    }
}

/// The line under the preset.
fn preset_line(preset: CompressorPreset) -> Option<String> {
    match preset {
        CompressorPreset::Night => Some(tr(
            "Evens the level out strongly. Its values are shown below; choose Custom to set your own.",
        )),
        CompressorPreset::Gentle => Some(tr("Evens the level out a little.")),
        CompressorPreset::Custom => None,
    }
}

/// The line of the limiter hint, with the makeup gain in use.
fn hint_line(compressor: &CompressorSettings) -> String {
    let makeup = super::trim_number(compressor.effective().makeup_db);
    let text = match compressor.preset {
        CompressorPreset::Night => tr(
            "Night adds up to {} dB of makeup gain. A sudden loud sound can go over full scale for a few milliseconds; the limiter catches that.",
        ),
        CompressorPreset::Gentle => tr(
            "Gentle adds up to {} dB of makeup gain. A sudden loud sound can go over full scale for a few milliseconds; the limiter catches that.",
        ),
        CompressorPreset::Custom => tr(
            "Your values add up to {} dB of makeup gain. A sudden loud sound can go over full scale for a few milliseconds; the limiter catches that.",
        ),
    };
    fill(&text, &[&makeup])
}

/// One slider of the values.
struct ValueSlider {
    value: Value,
    root: gtk::Box,
    shown: gtk::Label,
    scale: gtk::Scale,
}

fn value_slider(value: Value) -> ValueSlider {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_hexpand(true);
    root.add_css_class("hk-compressor-value");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = label(&value.title(), &[]);
    name.set_hexpand(true);
    head.append(&name);
    let shown = label("", &["hk-mono", "hk-t2", "hk-compressor-number"]);
    head.append(&shown);
    root.append(&head);
    let (min, max) = value.range();
    let scale = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        value.position(min),
        value.position(max),
        0.01,
    );
    scale.add_css_class("hk-slider");
    scale.set_draw_value(false);
    scale.set_widget_name(value.id());
    let range = fill(&tr("{} to {}"), &[&value.text(min), &value.text(max)]);
    scale.set_tooltip_text(Some(&range));
    scale.update_property(&[
        gtk::accessible::Property::Label(&value.title()),
        gtk::accessible::Property::Description(&range),
    ]);
    root.append(&scale);
    ValueSlider {
        value,
        root,
        shown,
        scale,
    }
}

/// One column of the activity readout.
fn activity_column(title: &str) -> (gtk::Box, gtk::Label) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.append(&label(title, &["hk-micro-label", "hk-readout-label"]));
    let value = label("—", &["hk-mono", "hk-strong", "hk-secondary"]);
    column.append(&value);
    (column, value)
}

/// The compressor's editor and what refreshes it.
pub(super) fn editor(live: &Rc<Live>) -> (gtk::Widget, Rc<dyn Fn()>) {
    let compressor: CompressorSettings = live
        .settings()
        .map(|s| s.dsp.compressor)
        .unwrap_or_default();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&lead(&tr(
        "Brings loud passages down and lifts quiet ones, so that music stays comfortable at a low volume. Night is for listening late, when the volume is low.",
    )));

    // The preset.
    let (group, buttons) = segmented(&[tr("Night"), tr("Gentle"), tr("Custom")]);
    group.update_property(&[gtk::accessible::Property::Label(&tr("Compressor preset"))]);
    let preset_note = note("");
    preset_note.set_widget_name("compressor-preset-line");
    let preset_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    preset_box.append(&group);
    preset_box.append(&preset_note);
    column.append(&row(&tr("Preset"), None, &preset_box));

    // The values.
    let grid = gtk::Grid::new();
    grid.set_row_spacing(10);
    grid.set_column_spacing(28);
    grid.set_column_homogeneous(true);
    let sliders: Rc<Vec<ValueSlider>> =
        Rc::new(Value::ALL.iter().map(|v| value_slider(*v)).collect());
    for (i, slider) in sliders.iter().enumerate() {
        grid.attach(&slider.root, (i % 3) as i32, (i / 3) as i32, 1, 1);
    }
    let values_row = row(&tr("Values"), None, &grid);
    column.append(&values_row);

    // The lookahead.
    let lookahead_line = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    let block = gtk::Box::new(gtk::Orientation::Vertical, 4);
    block.set_size_request(260, -1);
    block.set_valign(gtk::Align::Center);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let range = label(
        &fill(&tr("{} to {}"), &[&milliseconds(1.0), &milliseconds(5.0)]),
        &["hk-mono-meta", "hk-t3"],
    );
    range.set_hexpand(true);
    head.append(&range);
    let ms = |v: f64| fill(&tr("{} ms"), &[&decimal(v, 1)]);
    let lookahead_value = label(&ms(compressor.lookahead_ms), &["hk-mono", "hk-t2"]);
    head.append(&lookahead_value);
    block.append(&head);
    let lookahead = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        *CompressorSettings::LOOKAHEAD_RANGE.start(),
        *CompressorSettings::LOOKAHEAD_RANGE.end(),
        0.5,
    );
    lookahead.add_css_class("hk-slider");
    lookahead.set_draw_value(false);
    lookahead.set_value(compressor.lookahead_ms);
    lookahead.set_widget_name("compressor-lookahead");
    lookahead.update_property(&[gtk::accessible::Property::Label(&tr(
        "Lookahead in milliseconds",
    ))]);
    block.append(&lookahead);
    lookahead_line.append(&block);
    let why = note(&tr(
        "Used by every preset. Longer is gentler; the sound is delayed by the same time.",
    ));
    why.set_hexpand(true);
    why.set_valign(gtk::Align::Center);
    lookahead_line.append(&why);
    column.append(&row(&tr("Lookahead"), None, &lookahead_line));

    // The limiter hint.
    let (hint, hint_message, hint_detail, turn_on) =
        notice("helikodu-warning-symbolic", &tr("Turn the limiter on"));
    hint.set_widget_name("limiter-hint");
    hint_message.set_text(&tr("The limiter is off"));
    turn_on.set_widget_name("turn-limiter-on");
    column.append(&hint);

    // Activity.
    let readout = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    readout.add_css_class("hk-readout");
    readout.add_css_class("hk-dsp-readout");
    let (now, now_value) = activity_column(&tr("Gain reduction now"));
    now.remove(&now_value);
    let meter_line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let meter = gtk::LevelBar::for_interval(0.0, 24.0);
    meter.add_css_class("hk-meter");
    meter.set_hexpand(true);
    meter.set_valign(gtk::Align::Center);
    meter.update_property(&[gtk::accessible::Property::Label(&tr(
        "Gain reduction meter, 0 to 24 decibels",
    ))]);
    meter_line.append(&meter);
    now_value.set_widget_name("compressor-reduction");
    meter_line.append(&now_value);
    now.append(&meter_line);
    now.set_hexpand(true);
    readout.append(&now);
    let (largest, largest_value) = activity_column(&tr("Largest in this track"));
    largest.set_size_request(180, -1);
    readout.append(&largest);
    column.append(&row(&tr("Activity"), None, &readout));

    let updating = Rc::new(Cell::new(false));
    {
        let live = live.clone();
        let updating = updating.clone();
        bind_segmented(
            &buttons,
            PRESETS
                .iter()
                .position(|p| *p == compressor.preset)
                .unwrap_or(0),
            move |i| {
                if updating.get() {
                    return;
                }
                let preset = PRESETS[i];
                live.apply(|s| {
                    let compressor = &mut s.dsp.compressor;
                    if preset == CompressorPreset::Custom {
                        // Start from the values of the preset chosen before.
                        let values = compressor.effective();
                        for value in Value::ALL {
                            value.set(compressor, value.get(&values));
                        }
                    }
                    compressor.preset = preset;
                });
            },
        );
    }
    for slider in sliders.iter() {
        let live = live.clone();
        let updating = updating.clone();
        let value = slider.value;
        let shown = slider.shown.clone();
        slider.scale.connect_value_changed(move |scale| {
            let v = value.value(scale.value());
            shown.set_text(&value.text(v));
            if updating.get() {
                return;
            }
            live.apply_soon(move |s| {
                if s.dsp.compressor.preset == CompressorPreset::Custom {
                    value.set(&mut s.dsp.compressor, v);
                }
            });
        });
    }
    {
        let live = live.clone();
        let updating = updating.clone();
        lookahead.connect_value_changed(move |scale| {
            let v = (scale.value() * 2.0).round() / 2.0;
            lookahead_value.set_text(&ms(v));
            if !updating.get() {
                live.apply_soon(move |s| s.dsp.compressor.lookahead_ms = v);
            }
        });
    }
    {
        let live = live.clone();
        turn_on.connect_clicked(move |_| {
            live.apply(|s| s.dsp.limiter.enabled = true);
        });
    }

    // Read the activity a few times a second while the editor is shown.
    let timer: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
    let weak_live = Rc::downgrade(live);
    let poll = move || {
        let reading = weak_live
            .upgrade()
            .and_then(|l| l.context())
            .and_then(|c| c.services.dsp().compressor_activity());
        match reading {
            Some(r) => {
                meter.set_value(r.reduction_db.clamp(0.0, 24.0));
                now_value.set_text(&signed_db(-r.reduction_db.abs(), 1));
                largest_value.set_text(&signed_db(-r.largest_db.abs(), 1));
            }
            None => {
                meter.set_value(0.0);
                now_value.set_text("—");
                largest_value.set_text("—");
            }
        }
    };
    {
        let timer = timer.clone();
        column.connect_map(move |_| {
            poll();
            let poll = poll.clone();
            if let Some(old) = timer.replace(Some(glib::timeout_add_local(
                Duration::from_millis(250),
                move || {
                    poll();
                    glib::ControlFlow::Continue
                },
            ))) {
                old.remove();
            }
        });
    }
    column.connect_unmap(move |_| {
        if let Some(id) = timer.take() {
            id.remove();
        }
    });

    let weak_live = Rc::downgrade(live);
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        let Some(settings) = weak_live.upgrade().and_then(|l| l.settings()) else {
            return;
        };
        let compressor = settings.dsp.compressor;
        updating.set(true);
        if let Some(i) = PRESETS.iter().position(|p| *p == compressor.preset)
            && !buttons[i].is_active()
        {
            buttons[i].set_active(true);
        }
        match preset_line(compressor.preset) {
            Some(line) => {
                preset_note.set_text(&line);
                preset_note.set_visible(true);
            }
            None => preset_note.set_visible(false),
        }
        let custom = compressor.preset == CompressorPreset::Custom;
        let values = compressor.effective();
        for slider in sliders.iter() {
            let v = slider.value.get(&values);
            let position = slider.value.position(v);
            if (slider.scale.value() - position).abs() > 1e-6
                && slider.value.value(slider.scale.value()) != v
            {
                slider.scale.set_value(position);
            }
            slider.shown.set_text(&slider.value.text(v));
            slider.scale.set_sensitive(custom);
        }
        if custom {
            values_row.remove_css_class("hk-preset-values");
        } else {
            values_row.add_css_class("hk-preset-values");
        }
        if (lookahead.value() - compressor.lookahead_ms).abs() > 0.25 {
            lookahead.set_value(compressor.lookahead_ms);
        }
        hint.set_visible(!settings.dsp.limiter.enabled);
        hint_detail.set_text(&hint_line(&compressor));
        updating.set(false);
    });
    refresh();
    (column.upcast(), refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_as_drawn() {
        let night = CompressorSettings::default();
        assert_eq!(summary(&night), "Night");
        let custom = CompressorSettings {
            preset: CompressorPreset::Custom,
            ratio: 4.0,
            threshold_db: -18.0,
            ..night
        };
        assert_eq!(summary(&custom), "4 : 1 at −18 dB");
        assert_eq!(Value::Makeup.text(12.0), "+12 dB");
        assert_eq!(Value::Attack.text(5.0), "5 ms");
        assert_eq!(Value::Attack.text(0.5), "0.5 ms");
        // Attack and release are logarithmic and round as stored.
        let at = Value::Release.position(250.0);
        assert_eq!(Value::Release.value(at), 250.0);
        assert_eq!(Value::Attack.value(Value::Attack.position(0.1)), 0.1);
        assert!(hint_line(&night).starts_with("Night adds up to 12 dB"));
    }
}
