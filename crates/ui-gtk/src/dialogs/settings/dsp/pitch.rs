// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pitch editor: a shift in whole semitones that keeps the tempo.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use helikodu_model::settings::PitchSettings;

use super::{Live, info_note, lead};
use crate::dialogs::common::{compact_button, note, row};
use crate::i18n::{fill, ntr, tr};
use crate::widgets::{BipolarScale, label};

/// A shift in words: `+2 semitones`, `−1 semitone`, `0 semitones`.
pub(crate) fn semitones(value: f64) -> String {
    let whole = value.round();
    let count = whole.abs() as usize;
    let number = if whole > 0.0 {
        format!("+{}", super::trim_number(value))
    } else if whole < 0.0 {
        format!("−{}", super::trim_number(-value))
    } else {
        "0".to_owned()
    };
    fill(&ntr("{} semitone", "{} semitones", count), &[&number])
}

/// The pitch editor and what refreshes it.
pub(super) fn editor(live: &Rc<Live>) -> (gtk::Widget, Rc<dyn Fn()>) {
    let pitch: PitchSettings = live.settings().map(|s| s.dsp.pitch).unwrap_or_default();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&lead(&tr(
        "Moves the pitch up or down without changing the speed, for singing or playing along in another key.",
    )));

    let block = gtk::Box::new(gtk::Orientation::Vertical, 6);
    block.set_halign(gtk::Align::Start);
    block.set_size_request(420, -1);
    let words = label(
        &semitones(pitch.semitones),
        &["hk-mono", "hk-t2", "hk-secondary"],
    );
    words.set_halign(gtk::Align::End);
    words.set_widget_name("pitch-value");
    block.append(&words);
    let scale = BipolarScale::default();
    scale.set_step(1.0);
    scale.set_widget_name("pitch-shift");
    scale.update_property(&[gtk::accessible::Property::Label(&tr("Pitch shift"))]);
    scale.set_value_text(semitones);
    scale.set_value(pitch.semitones);
    block.append(&scale);
    let marks = gtk::CenterBox::new();
    marks.set_start_widget(Some(&label("−12", &["hk-mono-meta", "hk-t3"])));
    marks.set_center_widget(Some(&label("0", &["hk-mono-meta", "hk-t3"])));
    marks.set_end_widget(Some(&label("+12", &["hk-mono-meta", "hk-t3"])));
    block.append(&marks);
    block.append(&note(&tr(
        "In whole semitones; 12 are an octave. At 0 the stage does nothing.",
    )));
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    line.append(&block);
    let reset = compact_button(&tr("Reset to 0"));
    reset.set_widget_name("pitch-reset");
    reset.set_valign(gtk::Align::Start);
    reset.set_margin_top(14);
    reset.set_sensitive(pitch.semitones != 0.0);
    line.append(&reset);
    column.append(&row(&tr("Shift"), None, &line));
    column.append(&info_note(&tr(
        "Pitch shift adds about 33 ms of delay. It sounds best within a few semitones.",
    )));

    let updating = Rc::new(Cell::new(false));
    {
        let live = live.clone();
        let words = words.clone();
        let reset = reset.clone();
        let updating = updating.clone();
        scale.connect_changed(move |value| {
            words.set_text(&semitones(value));
            reset.set_sensitive(value != 0.0);
            if !updating.get() {
                live.apply_soon(move |s| s.dsp.pitch.semitones = value);
            }
        });
    }
    {
        let live = live.clone();
        reset.connect_clicked(move |_| {
            live.apply(|s| s.dsp.pitch.semitones = 0.0);
        });
    }
    let weak = Rc::downgrade(live);
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        let Some(pitch) = weak
            .upgrade()
            .and_then(|l| l.settings())
            .map(|s| s.dsp.pitch)
        else {
            return;
        };
        updating.set(true);
        scale.set_value(pitch.semitones);
        words.set_text(&semitones(pitch.semitones));
        reset.set_sensitive(pitch.semitones != 0.0);
        updating.set(false);
    });
    (column.upcast(), refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifts_read_in_words() {
        assert_eq!(semitones(2.0), "+2 semitones");
        assert_eq!(semitones(-1.0), "−1 semitone");
        assert_eq!(semitones(0.0), "0 semitones");
    }
}
