// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Loudness: the mode, the two preamps, the leveller for tracks
//! without loudness values, how clipping is prevented (lowering the gain or
//! the limiter), how many tracks have loudness values with "Measure the rest"
//! (and "Stop" while a measurement runs; its progress shows in the sidebar
//! footer), whether measured values also go into the tags of the files, and
//! at the bottom the signal path of what is heard, as the engine reports it.
//!
//! The leveller works only while a mode is on, and takes the place of the
//! preamp for files without loudness values: its row is insensitive while the
//! mode is Off, and that preamp while the leveller is on (both keep their
//! values).

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::settings::{ClippingPrevention, LoudnessMode};

use super::{Context, page_body};
use crate::backend::services::{LoudnessCoverage, LoudnessScope, Settings};
use crate::backend::{SignalState, StageKind, Update};
use crate::dialogs::common::{
    bind_segmented, compact_button, divider, dropdown, note, on_selected, option_card, row,
    segmented, switch_row, with_note,
};
use crate::dialogs::loudness_footer::{measure_in_background, stop_in_background};
use crate::i18n::{fill, ntr, tr};
use crate::playback::Change;
use crate::widgets::bipolar_scale::decibels;
use crate::widgets::signal_path_list::{
    device_format, device_name, source_text, stages, uses_leveller,
};
use crate::widgets::{BipolarScale, SignalLamp, label};

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

fn mode(context: &Rc<Context>, levelling: &Rc<Levelling>) -> gtk::Box {
    let (group, buttons) = segmented(&[tr("Off"), tr("Track"), tr("Album"), tr("Automatic")]);
    for (button, name) in buttons.iter().zip(["off", "track", "album", "auto"]) {
        button.set_widget_name(&format!("loudness-{name}"));
    }
    let current = context.settings().loudness.mode;
    let explanation = note(&mode_note(current));
    let weak = Rc::downgrade(context);
    let text = explanation.downgrade();
    let levelling = Rc::downgrade(levelling);
    bind_segmented(
        &buttons,
        MODES.iter().position(|m| *m == current).unwrap_or(3),
        move |i| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            let mode = MODES[i];
            if context.update(|s| s.loudness.mode = mode) {
                if let Some(text) = text.upgrade() {
                    text.set_text(&mode_note(mode));
                }
                if let Some(levelling) = levelling.upgrade() {
                    levelling.refresh(&context.settings());
                }
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
    scale.add_css_class("hk-preamp-scale");
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

fn preamps(context: &Rc<Context>, levelling: &Levelling) -> gtk::Box {
    let settings = context.settings();
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 28);
    line.set_homogeneous(true);
    line.set_valign(gtk::Align::Start);
    line.append(&preamp(
        context,
        &tr("Files with loudness values"),
        settings.loudness.preamp_db,
        |s, v| s.loudness.preamp_db = v,
    ));
    let untagged = preamp(
        context,
        &tr("Files without loudness values"),
        settings.loudness.untagged_preamp_db,
        |s, v| s.loudness.untagged_preamp_db = v,
    );
    untagged.set_widget_name("untagged-preamp");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.append(&untagged);
    let unused = note(&tr("Not used while the leveller is on."));
    unused.set_widget_name("untagged-preamp-note");
    column.append(&unused);
    line.append(&column);
    levelling.untagged.set(Some(&untagged));
    levelling.untagged_note.set(Some(&unused));
    line
}

/// The parts of the page that follow the mode and the leveller.
#[derive(Default)]
struct Levelling {
    row: glib::WeakRef<gtk::Box>,
    explanation: glib::WeakRef<gtk::Label>,
    untagged: glib::WeakRef<gtk::Box>,
    untagged_note: glib::WeakRef<gtk::Label>,
    rest: glib::WeakRef<gtk::Label>,
    /// How many tracks have loudness values, once counted.
    coverage: Cell<Option<LoudnessCoverage>>,
}

impl Levelling {
    /// Shows the rules of `settings`: the leveller row only while a mode is on,
    /// the preamp for files without values only while the leveller is off.
    fn refresh(&self, settings: &Settings) {
        let mode_on = settings.loudness.mode != LoudnessMode::Off;
        let leveller = settings.loudness.leveller;
        if let Some(row) = self.row.upgrade() {
            row.set_sensitive(mode_on);
        }
        if let Some(explanation) = self.explanation.upgrade() {
            explanation.set_text(&leveller_note(mode_on));
        }
        if let Some(untagged) = self.untagged.upgrade() {
            untagged.set_sensitive(!leveller);
        }
        if let Some(note) = self.untagged_note.upgrade() {
            note.set_visible(leveller);
        }
        if let Some(rest) = self.rest.upgrade() {
            rest.set_text(
                &self
                    .coverage
                    .get()
                    .map(|c| rest_text(c, mode_on && leveller))
                    .unwrap_or_default(),
            );
        }
    }
}

/// The explanation of the leveller: what it does, or when it works.
fn leveller_note(mode_on: bool) -> String {
    if mode_on {
        tr("Instead of a fixed preamp. The level changes gradually, by at most 12 dB.")
    } else {
        tr("Works while “Even out loudness” is on.")
    }
}

/// The leveller's switch row.
fn leveller(context: &Rc<Context>, levelling: &Levelling) -> gtk::Box {
    let (line, switch) = switch_row(
        &tr("Even out tracks without loudness values as they play"),
        Some(&leveller_note(true)),
    );
    line.set_widget_name("leveller-row");
    switch.set_widget_name("leveller");
    switch.set_active(context.settings().loudness.leveller);
    let explanation = line
        .first_child()
        .and_then(|text| text.last_child())
        .and_downcast::<gtk::Label>();
    if let Some(explanation) = &explanation {
        explanation.set_widget_name("leveller-note");
        levelling.explanation.set(Some(explanation));
    }
    levelling.row.set(Some(&line));
    let weak = Rc::downgrade(context);
    switch.connect_active_notify(move |switch| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let on = switch.is_active();
        if !context.update(|s| s.loudness.leveller = on) {
            switch.set_active(!on);
        }
    });
    line
}

/// The line about the tracks without loudness values: the preamp or the
/// leveller evens them out.
fn rest_text(coverage: LoudnessCoverage, leveller: bool) -> String {
    let missing = coverage.tracks.saturating_sub(coverage.with_values);
    if missing == 0 {
        tr("Every track has loudness values.")
    } else if leveller {
        fill(
            &ntr(
                "The other {} is evened out by the leveller as it plays.",
                "The other {} are evened out by the leveller as they play.",
                missing,
            ),
            &[&group_digits(missing)],
        )
    } else {
        fill(
            &ntr(
                "The other {} uses the preamp for files without loudness values.",
                "The other {} use the preamp for files without loudness values.",
                missing,
            ),
            &[&group_digits(missing)],
        )
    }
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
    let levelling = Rc::new(Levelling::default());
    body.append(&row(
        &tr("Even out loudness"),
        None,
        &mode(context, &levelling),
    ));
    body.append(&row(
        &tr("Preamp"),
        Some(&tr(
            "At 0 dB, files play at the ReplayGain 2.0 level of −18 LUFS.",
        )),
        &preamps(context, &levelling),
    ));
    body.append(&row(&tr("Leveller"), None, &leveller(context, &levelling)));
    body.append(&row(&tr("Prevent clipping"), None, &clipping(context)));
    body.append(&divider());
    body.append(&row(
        &tr("Loudness data"),
        None,
        &loudness_data(context, &levelling),
    ));
    body.append(&row(
        &tr("Measured values"),
        None,
        &measured_values(context),
    ));
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    body.append(&spacer);
    body.append(&readout(context));
    levelling.refresh(&context.settings());
    // The leveller's switch changes what the page shows.
    if let Some(switch) = levelling
        .row
        .upgrade()
        .and_then(|row| row.last_child())
        .and_downcast::<gtk::Switch>()
    {
        let weak = Rc::downgrade(context);
        let levelling_weak = Rc::downgrade(&levelling);
        switch.connect_active_notify(move |_| {
            if let (Some(context), Some(levelling)) = (weak.upgrade(), levelling_weak.upgrade()) {
                levelling.refresh(&context.settings());
            }
        });
    }
    // The parts live as long as the page.
    let keep = std::cell::RefCell::new(Some(levelling));
    scroller.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    scroller.upcast()
}

/// The signal path of what is heard, at the bottom of the page: the lamp and
/// its state, the source, the loudness stage when it works and the device,
/// and a sentence on what that means. It follows the engine, so it changes as
/// the controls change.
fn readout(context: &Rc<Context>) -> gtk::Box {
    let readout = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    readout.add_css_class("hk-readout");
    readout.add_css_class("hk-dsp-readout");
    readout.set_widget_name("loudness-readout");
    readout.update_property(&[gtk::accessible::Property::Label(&tr(
        "Signal path right now",
    ))]);
    let lamp = SignalLamp::default();
    lamp.set_valign(gtk::Align::Center);
    readout.append(&lamp);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    head.append(&label(&tr("Signal path right now"), &["hk-micro-label"]));
    let state = label("", &["hk-signal"]);
    state.set_widget_name("loudness-readout-state");
    head.append(&state);
    text.append(&head);
    let path = gtk::Label::new(None);
    path.set_xalign(0.0);
    path.set_wrap(true);
    path.add_css_class("hk-mono");
    path.add_css_class("hk-dsp-path");
    path.set_widget_name("loudness-readout-path");
    text.append(&path);
    let sentence = note("");
    sentence.add_css_class("hk-readout-note");
    sentence.set_widget_name("loudness-readout-note");
    text.append(&sentence);
    readout.append(&text);
    if let Some(playback) = &context.playback {
        let (lamp, state, path, sentence) = (
            lamp.downgrade(),
            state.downgrade(),
            path.downgrade(),
            sentence.downgrade(),
        );
        playback.connect(move |view, change| {
            if !matches!(change, Change::Signal | Change::Output) {
                return;
            }
            let (Some(lamp), Some(state), Some(path), Some(sentence)) = (
                lamp.upgrade(),
                state.upgrade(),
                path.upgrade(),
                sentence.upgrade(),
            ) else {
                return;
            };
            let shown = readout_text(view.signal.as_deref(), &view.output.device);
            lamp.set_state(shown.signal);
            state.set_text(&shown.state);
            for class in ["idle", "bit-perfect", "processed"] {
                state.remove_css_class(class);
            }
            state.add_css_class(match shown.signal {
                SignalState::BitPerfect => "bit-perfect",
                SignalState::Processed { .. } => "processed",
                _ => "idle",
            });
            path.set_text(&shown.path);
            sentence.set_text(&shown.sentence);
            sentence.set_visible(!shown.sentence.is_empty());
        });
    }
    readout
}

/// What the readout shows.
#[derive(Debug, PartialEq)]
struct Readout {
    signal: SignalState,
    state: String,
    path: String,
    sentence: String,
}

/// The readout for the path the engine reported (`None` when nothing is
/// heard), with `device` as the output button names it.
fn readout_text(path: Option<&crate::backend::SignalPath>, device: &str) -> Readout {
    use crate::widgets::signal_path_list::{sentence, state_label};
    let Some(path) = path else {
        return Readout {
            signal: SignalState::Idle,
            state: state_label(SignalState::Idle, None).unwrap_or_default(),
            path: tr("Nothing is playing."),
            sentence: String::new(),
        };
    };
    let (loudness, node) = stages(path, &[StageKind::Loudness]);
    let levels = node == crate::widgets::signal_path_list::Node::Active;
    // A gain changes the samples: the processed lamp, as the design draws it.
    let signal = if levels {
        SignalState::Processed { dsp: true }
    } else {
        SignalState::of(path)
    };
    let mode = match path.device.mode {
        crate::backend::services::OutputMode::Shared => tr("shared"),
        crate::backend::services::OutputMode::Exclusive => tr("exclusive"),
    };
    let mut parts = vec![source_text(path)];
    if levels {
        parts.push(loudness);
    }
    parts.push(fill(
        &tr("{} {} ({})"),
        &[&device_name(path, device), &device_format(path), &mode],
    ));
    // Wrap only between parts: the arrow starts the new line.
    let parts: Vec<String> = parts.iter().map(|p| p.replace(' ', "\u{00A0}")).collect();
    let sentence = if levels && uses_leveller(path) {
        tr(
            "This track has no loudness values, so its level is evened out as it plays. Playback is not bit-perfect while this is on.",
        )
    } else if levels {
        tr(
            "Changing the level changes the samples, so playback is not bit-perfect while this is on.",
        )
    } else {
        sentence(Some(path))
    };
    Readout {
        signal,
        state: state_label(signal, Some(path)).unwrap_or_default(),
        path: parts.join(" →\u{00A0}"),
        sentence,
    }
}

/// How many tracks have loudness values, and "Measure the rest" (or "Stop"
/// while a measurement runs).
fn loudness_data(context: &Rc<Context>, levelling: &Rc<Levelling>) -> gtk::Box {
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
    rest.set_widget_name("loudness-rest");
    levelling.rest.set(Some(&rest));
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
        let (count, meter) = (count.downgrade(), meter.downgrade());
        let levelling = Rc::downgrade(levelling);
        move || {
            let services = services.clone();
            let (count, meter, levelling) = (count.clone(), meter.clone(), levelling.clone());
            glib::spawn_future_local(async move {
                let settings = services.settings();
                let coverage = gio::spawn_blocking(move || services.loudness_coverage())
                    .await
                    .ok()
                    .flatten();
                let (Some(count), Some(meter), Some(levelling)) =
                    (count.upgrade(), meter.upgrade(), levelling.upgrade())
                else {
                    return;
                };
                levelling.coverage.set(coverage);
                levelling.refresh(&settings);
                let Some(coverage) = coverage else {
                    count.set_text(&tr("The library is not available."));
                    meter.set_visible(false);
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
    use super::*;
    use crate::backend::sample::signal_path;
    use crate::backend::{SignalLamp, StageKind};

    #[test]
    fn the_rest_is_evened_out_by_the_preamp_or_the_leveller() {
        let coverage = LoudnessCoverage {
            tracks: 18_402,
            with_values: 16_940,
        };
        assert_eq!(
            rest_text(coverage, false),
            "The other 1,462 use the preamp for files without loudness values."
        );
        assert_eq!(
            rest_text(coverage, true),
            "The other 1,462 are evened out by the leveller as they play."
        );
        let all = LoudnessCoverage {
            tracks: 3,
            with_values: 3,
        };
        assert_eq!(rest_text(all, true), "Every track has loudness values.");
    }

    #[test]
    fn the_readout_names_the_leveller() {
        let idle = readout_text(None, "Studio Interface");
        assert_eq!(idle.signal, SignalState::Idle);
        assert_eq!(idle.path, "Nothing is playing.");

        let bit_perfect = readout_text(Some(&signal_path(SignalLamp::BitPerfect)), "");
        assert_eq!(bit_perfect.signal, SignalState::BitPerfect);
        assert_eq!(
            bit_perfect.sentence,
            "The samples reach the device unchanged."
        );
        assert!(!bit_perfect.path.contains("ReplayGain"));

        let mut path = signal_path(SignalLamp::LosslessProcessed);
        let replaygain = readout_text(Some(&path), "");
        assert_eq!(replaygain.signal, SignalState::Processed { dsp: true });
        assert!(
            replaygain.path.contains("ReplayGain\u{a0}-6.30"),
            "{}",
            replaygain.path
        );
        assert!(
            replaygain
                .sentence
                .starts_with("Changing the level changes the samples")
        );

        if let Some(stage) = path
            .stages
            .iter_mut()
            .find(|s| s.kind == StageKind::Loudness)
        {
            stage.detail = "leveller (no loudness values)".into();
        }
        let leveller = readout_text(Some(&path), "");
        assert_eq!(leveller.state, "DSP active");
        assert_eq!(
            leveller.path.replace('\u{a0}', " "),
            "FLAC · 96 kHz · 24-bit → Leveller (no loudness values) → Studio Interface 96 kHz / 32-bit (exclusive)"
        );
        assert_eq!(
            leveller.sentence,
            "This track has no loudness values, so its level is evened out as it plays. Playback is not bit-perfect while this is on."
        );
    }

    #[test]
    fn counts_group_their_thousands() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(1462), "1,462");
        assert_eq!(group_digits(18_402), "18,402");
        assert_eq!(group_digits(1_000_000), "1,000,000");
    }
}
