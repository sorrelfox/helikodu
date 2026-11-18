// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Loudness: the mode, the two preamps, how clipping is prevented
//! (lowering the gain or the limiter), how many tracks have loudness values
//! with "Measure the rest" (and "Stop" while a measurement runs; its progress
//! shows in the sidebar footer), and whether measured values also go into the
//! tags of the files.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::settings::{ClippingPrevention, LoudnessMode};

use super::{Context, page_body};
use crate::backend::Update;
use crate::backend::services::LoudnessScope;
use crate::dialogs::common::{
    bind_segmented, compact_button, divider, dropdown, note, on_selected, option_card, row,
    segmented, with_note,
};
use crate::dialogs::loudness_footer::{measure_in_background, stop_in_background};
use crate::i18n::{fill, ntr, tr};
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
    limit_check.set_widget_name("clipping-limiter");
    let weak = Rc::downgrade(context);
    limit_check.connect_toggled(move |check| {
        if let Some(context) = weak.upgrade()
            && check.is_active()
        {
            context.update(|s| s.loudness.clipping = ClippingPrevention::Limiter);
        }
    });
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
    body.append(&divider());
    body.append(&row(&tr("Loudness data"), None, &loudness_data(context)));
    body.append(&row(
        &tr("Measured values"),
        None,
        &measured_values(context),
    ));
    scroller.upcast()
}

/// How many tracks have loudness values, and "Measure the rest" (or "Stop"
/// while a measurement runs).
fn loudness_data(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let count = label(&tr("Counting the tracks…"), &[]);
    count.set_widget_name("loudness-count");
    head.append(&count);
    let meter = gtk::LevelBar::for_interval(0.0, 1.0);
    meter.add_css_class("hk-meter");
    meter.add_css_class("hk-measure-bar");
    meter.set_valign(gtk::Align::Center);
    meter.update_property(&[gtk::accessible::Property::Label(&tr(
        "Tracks with loudness values",
    ))]);
    head.append(&meter);
    text.append(&head);
    let rest = note("");
    text.append(&rest);
    line.append(&text);
    let button = compact_button(&tr("Measure the rest"));
    button.set_widget_name("measure-loudness");
    line.append(&button);

    let measuring = Rc::new(Cell::new(context.services.is_measuring_loudness()));
    let show_state = {
        let button = button.downgrade();
        let measuring = measuring.clone();
        move || {
            if let Some(button) = button.upgrade() {
                button.set_label(&if measuring.get() {
                    tr("Stop")
                } else {
                    tr("Measure the rest")
                });
                button.set_sensitive(true);
            }
        }
    };
    show_state();
    // The counts come from the library, on a worker thread.
    let load = {
        let services = context.services.clone();
        let (count, meter, rest) = (count.downgrade(), meter.downgrade(), rest.downgrade());
        move || {
            let services = services.clone();
            let (count, meter, rest) = (count.clone(), meter.clone(), rest.clone());
            glib::spawn_future_local(async move {
                let coverage = gio::spawn_blocking(move || services.loudness_coverage())
                    .await
                    .ok()
                    .flatten();
                let (Some(count), Some(meter), Some(rest)) =
                    (count.upgrade(), meter.upgrade(), rest.upgrade())
                else {
                    return;
                };
                let Some(coverage) = coverage else {
                    count.set_text(&tr("The library is not available."));
                    meter.set_visible(false);
                    rest.set_text("");
                    return;
                };
                count.set_text(&fill(
                    &ntr(
                        // Translators: {0} is the number of tracks with values,
                        // which chooses the form, {1} the number of all tracks.
                        "{} of {} tracks has ReplayGain or R128 values",
                        "{} of {} tracks have ReplayGain or R128 values",
                        coverage.with_values,
                    ),
                    &[
                        &group_digits(coverage.with_values),
                        &group_digits(coverage.tracks),
                    ],
                ));
                meter.set_visible(true);
                meter.set_value(if coverage.tracks == 0 {
                    0.0
                } else {
                    coverage.with_values as f64 / coverage.tracks as f64
                });
                let missing = coverage.tracks.saturating_sub(coverage.with_values);
                rest.set_text(&if missing == 0 {
                    tr("Every track has loudness values.")
                } else {
                    fill(
                        &ntr(
                            "The other {} uses the preamp for files without loudness values.",
                            "The other {} use the preamp for files without loudness values.",
                            missing,
                        ),
                        &[&group_digits(missing)],
                    )
                });
            });
        }
    };
    load();
    let weak = Rc::downgrade(context);
    {
        let measuring = measuring.clone();
        let show_state = show_state.clone();
        button.connect_clicked(move |button| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            button.set_sensitive(false);
            let show_state = show_state.clone();
            if measuring.get() {
                let measuring = measuring.clone();
                stop_in_background(context.services.clone(), move || {
                    measuring.set(false);
                    show_state();
                });
            } else {
                let measuring = measuring.clone();
                let weak = Rc::downgrade(&context);
                measuring.set(true);
                measure_in_background(
                    context.services.clone(),
                    LoudnessScope::Rest,
                    move |problem| {
                        if let Some(problem) = problem {
                            measuring.set(false);
                            if let Some(context) = weak.upgrade() {
                                context.toast(&problem);
                            }
                        }
                        show_state();
                    },
                );
            }
        });
    }
    // Follow the measurements, whoever started them.
    if let Some(playback) = &context.playback {
        let weak_line = line.downgrade();
        playback.connect_event(move |update| {
            let Update::Loudness(status) = update else {
                return;
            };
            if weak_line.upgrade().is_none() {
                return;
            }
            let running = !status.finished;
            if measuring.replace(running) != running {
                show_state();
            }
            if status.finished {
                load();
            }
        });
    }
    line
}

/// A count with thousands grouped: `18,402`.
fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Whether measured values also go into the tags of the files.
fn measured_values(context: &Rc<Context>) -> gtk::Box {
    let current = context.settings().loudness.write_measured_to_tags;
    let choice = dropdown(
        &[
            tr("Keep them in the library only"),
            tr("Also write them to the tags of the files"),
        ],
        usize::from(current),
        320,
    );
    choice.set_widget_name("write-measured");
    let weak = Rc::downgrade(context);
    on_selected(&choice, move |i| {
        if let Some(context) = weak.upgrade() {
            context.update(|s| s.loudness.write_measured_to_tags = i == 1);
        }
    });
    with_note(
        &choice,
        &tr("Tags are written as ReplayGain 2.0, and as R128 in Opus files."),
    )
}

#[cfg(test)]
mod tests {
    use super::group_digits;

    #[test]
    fn counts_group_their_thousands() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(1462), "1,462");
        assert_eq!(group_digits(18_402), "18,402");
        assert_eq!(group_digits(1_000_000), "1,000,000");
    }
}
