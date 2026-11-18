// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Loudness: the mode, the two preamps and how clipping is
//! prevented. The limiter, the measurement of the rest of the library and
//! the readout of the signal path come in a later version.

use std::rc::Rc;

use gtk::prelude::*;
use helikodu_model::settings::{ClippingPrevention, LoudnessMode};

use super::{Context, page_body};
use crate::dialogs::common::{bind_segmented, note, option_card, row, segmented};
use crate::i18n::tr;
use crate::widgets::BipolarScale;
use crate::widgets::bipolar_scale::decibels;
use crate::widgets::label;

const MODES: [LoudnessMode; 4] = [
    LoudnessMode::Off,
    LoudnessMode::Track,
    LoudnessMode::Album,
    LoudnessMode::Auto,
];

fn mode_note(mode: LoudnessMode) -> String {
    match mode {
        LoudnessMode::Off => tr("Files play at the level they were made at."),
        LoudnessMode::Track => tr("Every track plays at the same loudness."),
        LoudnessMode::Album => tr(
            "Albums play at the same loudness, and the tracks of an album keep their differences.",
        ),
        LoudnessMode::Auto => tr(
            "Album loudness while an album plays in order, track loudness otherwise, for example in shuffle.",
        ),
    }
}

fn mode(context: &Rc<Context>) -> gtk::Box {
    let (group, buttons) = segmented(&[tr("Off"), tr("Track"), tr("Album"), tr("Automatic")]);
    let current = context.settings().loudness.mode;
    let explanation = note(&mode_note(current));
    let weak = Rc::downgrade(context);
    let text = explanation.downgrade();
    bind_segmented(
        &buttons,
        MODES.iter().position(|m| *m == current).unwrap_or(3),
        move |i| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            let mode = MODES[i];
            if context.update(|s| s.loudness.mode = mode)
                && let Some(text) = text.upgrade()
            {
                text.set_text(&mode_note(mode));
            }
        },
    );
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.append(&group);
    column.append(&explanation);
    column
}

fn preamp(
    context: &Rc<Context>,
    title: &str,
    value: f64,
    set: fn(&mut crate::backend::services::Settings, f64),
) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.set_hexpand(true);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = label(title, &[]);
    name.set_hexpand(true);
    head.append(&name);
    let shown = label(&decibels(value), &["hk-mono", "hk-t2", "hk-secondary"]);
    head.append(&shown);
    column.append(&head);
    let scale = BipolarScale::default();
    scale.set_value(value);
    scale.update_property(&[gtk::accessible::Property::Label(title)]);
    let weak = Rc::downgrade(context);
    scale.connect_changed(move |v| {
        shown.set_text(&decibels(v));
        if let Some(context) = weak.upgrade() {
            context.update(|s| set(s, v));
        }
    });
    column.append(&scale);
    let scale_labels = gtk::CenterBox::new();
    scale_labels.set_start_widget(Some(&label("−12", &["hk-mono-meta", "hk-t3"])));
    scale_labels.set_center_widget(Some(&label(&decibels(0.0), &["hk-mono-meta", "hk-t3"])));
    scale_labels.set_end_widget(Some(&label("+12", &["hk-mono-meta", "hk-t3"])));
    column.append(&scale_labels);
    column
}

fn preamps(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 28);
    line.set_homogeneous(true);
    line.append(&preamp(
        context,
        &tr("Files with loudness values"),
        settings.loudness.preamp_db,
        |s, v| s.loudness.preamp_db = v,
    ));
    line.append(&preamp(
        context,
        &tr("Files without loudness values"),
        settings.loudness.untagged_preamp_db,
        |s, v| s.loudness.untagged_preamp_db = v,
    ));
    line
}

fn clipping(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.set_homogeneous(true);
    let (lower, lower_check) = option_card(
        &tr("Lower the gain"),
        &tr("The gain is reduced until the loudest peak fits. Nothing else is changed."),
        None,
    );
    let (limit, limit_check) = option_card(
        &tr("Use the limiter"),
        &tr("The full gain is kept. Peaks above full scale are limited."),
        Some(&lower_check),
    );
    if context.settings().loudness.clipping == ClippingPrevention::Limiter {
        limit_check.set_active(true);
    } else {
        lower_check.set_active(true);
    }
    // The limiter comes in a later version.
    limit.set_sensitive(false);
    let weak = Rc::downgrade(context);
    lower_check.connect_toggled(move |check| {
        if let Some(context) = weak.upgrade()
            && check.is_active()
        {
            context.update(|s| s.loudness.clipping = ClippingPrevention::LowerGain);
        }
    });
    line.append(&lower);
    line.append(&limit);
    line
}

/// The Loudness page.
pub(super) fn page(context: &Rc<Context>) -> gtk::Widget {
    let (scroller, body) = page_body();
    body.append(&row(&tr("Even out loudness"), None, &mode(context)));
    body.append(&row(
        &tr("Preamp"),
        Some(&tr(
            "At 0 dB, files play at the ReplayGain 2.0 level of −18 LUFS.",
        )),
        &preamps(context),
    ));
    body.append(&row(&tr("Prevent clipping"), None, &clipping(context)));
    scroller.upcast()
}
