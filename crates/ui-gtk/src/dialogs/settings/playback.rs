// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Playback: the Previous button, the seek step, when a play
//! counts, what playing an album does to the queue, crossfades and fades, the
//! start-up queue, long tracks, power, and the decoders of synthesized
//! formats (the SoundFont for MIDI, the song lengths of SID tunes, how often a
//! looping tune repeats and its fade). The two power switches that a later
//! version applies are shown insensitive.

use std::rc::Rc;

use gtk::prelude::*;
use helikodu_model::settings::{ContextAction, StartAction};

use super::{Context, page_body};
use crate::backend::services::Settings;
use crate::dialogs::common::{
    compact_button, divider, dropdown, key_caps, note, on_selected, row, switch_row, tilde,
    with_note,
};
use crate::dialogs::decoder_files::{self, DecoderFile};
use crate::i18n::{fill, ntr, tr, trc};
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
    let share = value_dropdown(
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
    );
    let time = value_dropdown(
        context,
        &[1, 2, 4, 5, 10],
        settings.playback.play_count_minutes,
        minutes,
        140,
        |s, v| s.playback.play_count_minutes = v,
    );
    crate::widgets::sentence(
        &line,
        // Translators: when a play counts. {0} is a dropdown with a share of
        // the track ("half the track"), {1} one with a time ("4 minutes");
        // reorder them with {0} and {1} as needed.
        &tr("after {} or {} whichever comes first"),
        &[share.upcast_ref(), time.upcast_ref()],
        &["hk-t2"],
    );
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
        &tr(
            "The queue, the track and the position come back. Playback continues if it was playing when Helikodu closed.",
        ),
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
    for (title, value, name, set) in [
        (
            tr("Pause when the screen locks"),
            settings.playback.pause_on_lock,
            "pause-on-lock",
            (|s: &mut Settings, on| s.playback.pause_on_lock = on) as fn(&mut Settings, bool),
        ),
        (
            tr("Pause before the computer goes to sleep"),
            settings.playback.pause_before_sleep,
            "pause-before-sleep",
            |s: &mut Settings, on| s.playback.pause_before_sleep = on,
        ),
    ] {
        let (line, switch) = switch_row(&title, None);
        switch.set_active(value);
        switch.set_widget_name(name);
        let weak = Rc::downgrade(context);
        switch.connect_active_notify(move |switch| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            let on = switch.is_active();
            if !context.update(|s| set(s, on)) {
                switch.set_active(!on);
            } else if let Some(main) = context.main.upgrade() {
                main.apply_power_settings();
            }
        });
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
    body.append(&row(&tr("Crossfade"), None, &crossfade(context)));
    body.append(&row(&tr("Fades"), None, &fades(context)));
    body.append(&divider());
    body.append(&row(&tr("When Helikodu starts"), None, &start(context)));
    body.append(&row(&tr("Long tracks"), None, &long_tracks(context)));
    body.append(&divider());
    body.append(&row(&tr("Power"), None, &power(context)));
    body.append(&divider());
    body.append(&row(
        &tr("MIDI files"),
        None,
        &decoder_file(context, DecoderFile::SoundFont),
    ));
    body.append(&row(
        &tr("SID tunes"),
        None,
        &decoder_file(context, DecoderFile::SidSongLengths),
    ));
    body.append(&row(&tr("Looping tunes"), None, &looping(context)));
    scroller.upcast()
}

/// A length in milliseconds as the fade and crossfade lists show it: `Off`,
/// `50 ms`, `1 s`, `2.5 s`.
fn length(ms: u32) -> String {
    match ms {
        0 => tr("Off"),
        ms if ms < 1000 => fill(&tr("{} ms"), &[&ms.to_string()]),
        ms if ms % 1000 == 0 => fill(&tr("{} s"), &[&(ms / 1000).to_string()]),
        ms => fill(
            &tr("{} s"),
            &[&crate::format::decimal(f64::from(ms) / 1000.0, 1)],
        ),
    }
}

/// The crossfade between tracks and on a skip: "between tracks [Off]",
/// "when you skip [Off]". Off keeps the length for when it is on again.
fn crossfade(context: &Rc<Context>) -> gtk::Box {
    let transitions = context.settings().playback.transitions;
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.append(&label(&tr("between tracks"), &["hk-t2"]));
    let between = value_dropdown(
        context,
        &[0, 1000, 2000, 3000, 5000, 8000, 12_000],
        if transitions.crossfade {
            transitions.crossfade_ms
        } else {
            0
        },
        length,
        116,
        |s, ms| {
            let t = &mut s.playback.transitions;
            t.crossfade = ms > 0;
            if ms > 0 {
                t.crossfade_ms = ms;
            }
        },
    );
    between.set_widget_name("crossfade");
    line.append(&between);
    let skip_words = label(&tr("when you skip"), &["hk-t2"]);
    skip_words.set_margin_start(10);
    line.append(&skip_words);
    let skip = value_dropdown(
        context,
        &[0, 500, 1000, 2000, 3000, 5000],
        if transitions.skip_crossfade {
            transitions.skip_crossfade_ms
        } else {
            0
        },
        length,
        116,
        |s, ms| {
            let t = &mut s.playback.transitions;
            t.skip_crossfade = ms > 0;
            if ms > 0 {
                t.skip_crossfade_ms = ms;
            }
        },
    );
    skip.set_widget_name("skip-crossfade");
    line.append(&skip);
    with_note(
        &line,
        &tr(
            "Tracks that follow each other on an album always join without a crossfade. Nothing is crossfaded on a bit-perfect path.",
        ),
    )
}

/// The four fades, each with its label above.
fn fades(context: &Rc<Context>) -> gtk::Box {
    let transitions = context.settings().playback.transitions;
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.set_homogeneous(true);
    type Fade = (String, &'static str, u32, fn(&mut Settings, u32));
    let fades: [Fade; 4] = [
        (
            tr("Pause and resume"),
            "pause-fade",
            transitions.pause_fade_ms,
            |s, v| s.playback.transitions.pause_fade_ms = v,
        ),
        (tr("Stop"), "stop-fade", transitions.stop_fade_ms, |s, v| {
            s.playback.transitions.stop_fade_ms = v
        }),
        (
            // Translators: the heading of the fade used when seeking within a track.
            trc("fade", "Seek"),
            "seek-fade",
            transitions.seek_fade_ms,
            |s, v| s.playback.transitions.seek_fade_ms = v,
        ),
        (
            tr("Skip to another track"),
            "skip-fade",
            transitions.skip_fade_ms,
            |s, v| s.playback.transitions.skip_fade_ms = v,
        ),
    ];
    for (title, name, current, set) in fades {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = label(&title, &["hk-t2", "hk-secondary"]);
        column.append(&heading);
        let choice = value_dropdown(
            context,
            &[0, 50, 100, 200, 300, 500, 1000, 2000],
            current,
            length,
            -1,
            set,
        );
        choice.set_halign(gtk::Align::Fill);
        choice.set_widget_name(name);
        choice.add_css_class(crate::dialogs::common::OWN_LABEL);
        choice.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
            heading.upcast_ref()
        ])]);
        column.append(&choice);
        line.append(&column);
    }
    line
}

/// A file a decoder needs: where it is, or that none is chosen, with
/// "Choose…" and "Clear".
fn decoder_file(context: &Rc<Context>, which: DecoderFile) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let (title, unset, explanation) = match which {
        DecoderFile::SoundFont => (
            tr("SoundFont"),
            tr("None chosen: a SoundFont installed on the system is used, if there is one."),
            tr("MIDI files are played with the instruments of a SoundFont (.sf2 or .sf3)."),
        ),
        DecoderFile::SidSongLengths => (
            tr("Song lengths"),
            tr("Not set: tunes play for the default length, then fade out."),
            tr("Songlengths.md5 of the High Voltage SID Collection gives each tune its length."),
        ),
    };
    let current = which.get(&context.settings());
    text.append(&label(&title, &[]));
    let shown = label(
        &current.as_deref().map(tilde).unwrap_or(unset.clone()),
        &["hk-mono-small", "hk-t2"],
    );
    shown.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    shown.set_widget_name(match which {
        DecoderFile::SoundFont => "soundfont-path",
        DecoderFile::SidSongLengths => "sid-lengths-path",
    });
    text.append(&shown);
    text.append(&note(&explanation));
    line.append(&text);
    let choose = compact_button(&tr("Choose…"));
    choose.update_property(&[gtk::accessible::Property::Label(&which.title())]);
    line.append(&choose);
    let clear = compact_button(&tr("Clear"));
    clear.update_property(&[gtk::accessible::Property::Label(&fill(
        &tr("Clear {}"),
        &[&title],
    ))]);
    clear.set_visible(current.is_some());
    line.append(&clear);
    let weak = Rc::downgrade(context);
    let (weak_shown, weak_clear) = (shown.downgrade(), clear.downgrade());
    choose.connect_clicked(move |_| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let (weak, weak_shown, weak_clear) = (weak.clone(), weak_shown.clone(), weak_clear.clone());
        decoder_files::choose(
            &context.window,
            context.services.clone(),
            which,
            move |result| match result {
                Ok(path) => {
                    if let Some(shown) = weak_shown.upgrade() {
                        shown.set_text(&tilde(&path));
                    }
                    if let Some(clear) = weak_clear.upgrade() {
                        clear.set_visible(true);
                    }
                }
                Err(problem) => {
                    if let Some(context) = weak.upgrade() {
                        context.toast(&problem);
                    }
                }
            },
        );
    });
    let weak = Rc::downgrade(context);
    let weak_shown = shown.downgrade();
    clear.connect_clicked(move |clear| {
        if let Some(context) = weak.upgrade()
            && context.update(|s| which.set(s, None))
        {
            clear.set_visible(false);
            if let Some(shown) = weak_shown.upgrade() {
                shown.set_text(&unset);
            }
        }
    });
    line
}

/// How often a looping tune plays its loop, and its fade.
fn looping(context: &Rc<Context>) -> gtk::Box {
    let decoders = context.settings().playback.decoders.clone();
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let loops = value_dropdown(
        context,
        &[1, 2, 3, 4, 5],
        decoders.loops,
        |n| fill(&ntr("once", "{} times", n as usize), &[&n.to_string()]),
        110,
        |s, n| s.playback.decoders.loops = n,
    );
    loops.set_widget_name("loops");
    let fade = value_dropdown(
        context,
        &[0, 3000, 5000, 8000, 10_000, 15_000],
        decoders.fade_ms,
        length,
        110,
        |s, ms| s.playback.decoders.fade_ms = ms,
    );
    fade.set_widget_name("loop-fade");
    crate::widgets::sentence(
        &line,
        // Translators: how a looping tune ends. {0} is a dropdown with how
        // often ("once", "2 times"), {1} one with a time ("5 s"); reorder
        // them with {0} and {1} as needed.
        &tr("play the loop {} then fade out over {}"),
        &[loops.upcast_ref(), fade.upcast_ref()],
        &["hk-t2"],
    );
    let default = decoders.default_length_s;
    with_note(
        &line,
        &fill(
            &tr(
                "For tracker modules and game music that repeat a part. Tunes whose length nothing states play for {} before the fade.",
            ),
            &[&format!("{}:{:02}", default / 60, default % 60)],
        ),
    )
}
