// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Playback: the Previous button, the seek step, when a play
//! counts, what playing an album does to the queue, the start-up queue, long
//! tracks and power. Crossfades and fades come in a later version; the two
//! power switches that a later version applies are shown insensitive.

use std::rc::Rc;

use gtk::prelude::*;
use helikodu_model::settings::{ContextAction, StartAction};

use super::{Context, page_body};
use crate::backend::services::Settings;
use crate::dialogs::common::{
    divider, dropdown, key_caps, note, on_selected, row, switch_row, with_note,
};
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;

/// A dropdown over `values` showing `text` for each; a stored value that is
/// not offered is added so that it shows as it is.
fn value_dropdown<T: Copy + PartialEq + Ord + 'static>(
    context: &Rc<Context>,
    values: &[T],
    current: T,
    text: impl Fn(T) -> String,
    width: i32,
    set: impl Fn(&mut Settings, T) + 'static,
) -> gtk::DropDown {
    let mut values = values.to_vec();
    if !values.contains(&current) {
        values.push(current);
    }
    let items: Vec<String> = values.iter().map(|&v| text(v)).collect();
    let index = values.iter().position(|&v| v == current).unwrap_or(0);
    let choice = dropdown(&items, index, width);
    let weak = Rc::downgrade(context);
    on_selected(&choice, move |i| {
        if let (Some(context), Some(&v)) = (weak.upgrade(), values.get(i)) {
            context.update(|s| set(s, v));
        }
    });
    choice
}

fn seconds(n: u32) -> String {
    fill(
        &ntr("{} second", "{} seconds", n as usize),
        &[&n.to_string()],
    )
}

fn minutes(n: u32) -> String {
    fill(
        &ntr("{} minute", "{} minutes", n as usize),
        &[&n.to_string()],
    )
}

fn previous(context: &Rc<Context>) -> gtk::Box {
    let current = context.settings().playback.previous_restarts_after_s;
    let explanation = note(&previous_note(current));
    let weak = explanation.downgrade();
    let choice = value_dropdown(
        context,
        &[3, 5, 0],
        current,
        |s| {
            if s == 0 {
                tr("Always go to the previous track")
            } else {
                fill(
                    &ntr(
                        "Restart the track after {} second",
                        "Restart the track after {} seconds",
                        s as usize,
                    ),
                    &[&s.to_string()],
                )
            }
        },
        320,
        move |s, v| {
            s.playback.previous_restarts_after_s = v;
            if let Some(explanation) = weak.upgrade() {
                explanation.set_text(&previous_note(v));
            }
        },
    );
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.append(&choice);
    column.append(&explanation);
    column
}

fn previous_note(seconds: u32) -> String {
    if seconds == 0 {
        tr("Previous always goes to the track before, wherever the current one is.")
    } else {
        fill(
            &ntr(
                "In the first {} second of a track, Previous goes to the track before it.",
                "In the first {} seconds of a track, Previous goes to the track before it.",
                seconds as usize,
            ),
            &[&seconds.to_string()],
        )
    }
}

fn seek_step(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let step = value_dropdown(
        context,
        &[2, 5, 10, 15, 30],
        context.settings().playback.seek_step_s,
        seconds,
        140,
        |s, v| s.playback.seek_step_s = v,
    );
    step.set_widget_name("seek-step");
    step.update_property(&[gtk::accessible::Property::Label(&tr("Seek step"))]);
    line.append(&step);
    line.append(&key_caps(&["Ctrl", "Shift", "←"]));
    line.append(&label(&tr("and"), &["hk-t2"]));
    line.append(&key_caps(&["Ctrl", "Shift", "→"]));
    line
}

fn count_play(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    // The sentence reads "after <share> or <time>, whichever comes first".
    line.append(&label(&tr("after"), &["hk-t2"]));
    line.append(&value_dropdown(
        context,
        &[25, 50, 75, 100],
        settings.playback.play_count_percent,
        |p| match p {
            25 => tr("a quarter of the track"),
            50 => tr("half the track"),
            75 => tr("three quarters of the track"),
            100 => tr("the whole track"),
            p => fill(&tr("{} % of the track"), &[&p.to_string()]),
        },
        170,
        |s, v| s.playback.play_count_percent = v,
    ));
    line.append(&label(&tr("or"), &["hk-t2"]));
    line.append(&value_dropdown(
        context,
        &[1, 2, 4, 5, 10],
        settings.playback.play_count_minutes,
        minutes,
        140,
        |s, v| s.playback.play_count_minutes = v,
    ));
    line.append(&label(&tr("whichever comes first"), &["hk-t2"]));
    with_note(
        &line,
        &tr("Only time actually heard is counted. Seeking forward does not add to it."),
    )
}

fn album(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let current = context.settings().playback.play_context;
    let order = |a: ContextAction| usize::from(a == ContextAction::Append);
    let choice = dropdown(
        &[tr("Replaces the queue"), tr("Adds to the end of the queue")],
        order(current),
        240,
    );
    let weak = Rc::downgrade(context);
    on_selected(&choice, move |i| {
        if let Some(context) = weak.upgrade() {
            context.update(|s| {
                s.playback.play_context = if i == 1 {
                    ContextAction::Append
                } else {
                    ContextAction::Replace
                };
            });
        }
    });
    line.append(&choice);
    let hint = note(&tr("“Play next” and “Add to queue” always keep the queue."));
    hint.set_hexpand(true);
    line.append(&hint);
    line
}

fn start(context: &Rc<Context>) -> gtk::Box {
    let current = context.settings().playback.on_start;
    let choice = dropdown(
        &[
            tr("Restore the last session"),
            tr("Start with an empty queue"),
        ],
        usize::from(current == StartAction::Empty),
        320,
    );
    let weak = Rc::downgrade(context);
    on_selected(&choice, move |i| {
        if let Some(context) = weak.upgrade() {
            context.update(|s| {
                s.playback.on_start = if i == 1 {
                    StartAction::Empty
                } else {
                    StartAction::Restore
                };
            });
        }
    });
    with_note(
        &choice,
        &tr("The queue, the track and the position come back. Playback stays paused."),
    )
}

fn long_tracks(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let (line, switch) = switch_row(
        &tr("Resume long tracks where they stopped"),
        Some(&tr(
            "Applies to audiobooks and to tracks longer than the time on the right.",
        )),
    );
    let length = value_dropdown(
        context,
        &[10, 20, 30, 60],
        settings.playback.long_track_minutes,
        minutes,
        124,
        |s, v| s.playback.long_track_minutes = v,
    );
    length.update_property(&[gtk::accessible::Property::Label(&tr("Long tracks"))]);
    line.insert_child_after(&length, line.first_child().as_ref());
    switch.set_active(settings.playback.resume_long_tracks);
    let weak = Rc::downgrade(context);
    switch.connect_active_notify(move |switch| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let on = switch.is_active();
        if !context.update(|s| s.playback.resume_long_tracks = on) {
            switch.set_active(!on);
        }
    });
    line
}

fn power(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let (awake, switch) = switch_row(
        &tr("Keep the computer awake while music plays"),
        Some(&tr(
            "The computer can sleep again as soon as playback pauses or stops.",
        )),
    );
    switch.set_active(settings.playback.keep_awake);
    switch.set_widget_name("keep-awake");
    let weak = Rc::downgrade(context);
    switch.connect_active_notify(move |switch| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let on = switch.is_active();
        if !context.update(|s| s.playback.keep_awake = on) {
            switch.set_active(!on);
        }
    });
    column.append(&awake);
    // Applied from a later version: shown with their values, not sensitive.
    for (title, value) in [
        (
            tr("Pause when the screen locks"),
            settings.playback.pause_on_lock,
        ),
        (
            tr("Pause before the computer goes to sleep"),
            settings.playback.pause_before_sleep,
        ),
    ] {
        let (line, switch) = switch_row(&title, None);
        switch.set_active(value);
        line.set_sensitive(false);
        column.append(&line);
    }
    column
}

/// The Playback page.
pub(super) fn page(context: &Rc<Context>) -> gtk::Widget {
    let (scroller, body) = page_body();
    body.append(&row(&tr("Previous button"), None, &previous(context)));
    body.append(&row(&tr("Seek step"), None, &seek_step(context)));
    body.append(&row(&tr("Count a play"), None, &count_play(context)));
    body.append(&row(&tr("Playing an album"), None, &album(context)));
    body.append(&divider());
    body.append(&row(&tr("When Helikodu starts"), None, &start(context)));
    body.append(&row(&tr("Long tracks"), None, &long_tracks(context)));
    body.append(&divider());
    body.append(&row(&tr("Power"), None, &power(context)));
    scroller.upcast()
}
