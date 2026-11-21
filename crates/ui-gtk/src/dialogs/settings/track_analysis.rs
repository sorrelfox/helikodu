// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Library, Track analysis: the switch of the analysis in the
//! background, and how far it is: the tracks analysed with a neutral meter,
//! and a line with what the analysis does now and how many tracks could not
//! be read. The counts come from the library when the page is built and then
//! follow the library service, which reports them when the state changes and
//! every two seconds or so while the analysis works. Nothing else in the
//! window shows the analysis.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::Context;
use crate::backend::Update;
use crate::backend::services::{AnalysisProgress, AnalysisState, PauseReason};
use crate::dialogs::common::{note, switch_row};
use crate::format::count;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;

/// The widget name of the group, for links that open the page at it.
pub(crate) const ROW: &str = "track-analysis";

/// "14,950 of 18,402 tracks analysed".
fn count_text(progress: &AnalysisProgress) -> String {
    fill(
        &ntr(
            // Translators: {0} is the number of tracks analysed, {1} the
            // number of all tracks, which chooses the form.
            "{} of {} track analysed",
            "{} of {} tracks analysed",
            progress.total,
        ),
        &[&count(progress.analyzed), &count(progress.total)],
    )
}

/// What the analysis does now, and after it how many tracks could not be
/// read.
pub(crate) fn state_text(progress: &AnalysisProgress) -> String {
    let state = match progress.state {
        AnalysisState::Off => tr("Crossfades use their fixed length, and silence is not left out."),
        AnalysisState::Idle if progress.remaining() == 0 => tr("Every track is analysed."),
        // Waiting for the delay after the start or a scan.
        AnalysisState::Idle => tr("Starts in a moment."),
        AnalysisState::Yielding => tr("Working slowly while music plays from the same drive."),
        AnalysisState::Paused(PauseReason::Scanning) => tr("Waits for the library scan to finish."),
        AnalysisState::Paused(PauseReason::OnBattery) => {
            tr("Paused while the computer runs on battery.")
        }
        _ => tr("Working in the background."),
    };
    if progress.failed == 0 {
        return state;
    }
    let failed = fill(
        &ntr(
            "{} track could not be read.",
            "{} tracks could not be read.",
            progress.failed,
        ),
        &[&count(progress.failed)],
    );
    // Translators: the state of the track analysis, then how many tracks
    // could not be read.
    fill(&tr("{} {}"), &[&state, &failed])
}

/// The share of the meter: the tracks analysed, full once every track is
/// done (those that could not be read included).
fn meter_value(progress: &AnalysisProgress) -> f64 {
    if progress.total == 0 || progress.remaining() == 0 {
        1.0
    } else {
        progress.analyzed as f64 / progress.total as f64
    }
}

/// The parts of the progress row.
struct ProgressRow {
    row: gtk::Box,
    count: gtk::Label,
    meter: gtk::LevelBar,
    state: gtk::Label,
}

impl ProgressRow {
    fn new() -> Self {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        row.set_widget_name("analysis-progress");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let count = label("", &[]);
        count.set_widget_name("analysis-count");
        head.append(&count);
        let meter = gtk::LevelBar::for_interval(0.0, 1.0);
        meter.add_css_class("hk-meter");
        meter.set_size_request(160, -1);
        meter.set_valign(gtk::Align::Center);
        meter.set_widget_name("analysis-meter");
        meter.update_property(&[gtk::accessible::Property::Label(&tr("Tracks analysed"))]);
        head.append(&meter);
        row.append(&head);
        let state = note("");
        state.set_widget_name("analysis-state");
        row.append(&state);
        // Shown once the counts are known, and only with tracks to count.
        row.set_visible(false);
        Self {
            row,
            count,
            meter,
            state,
        }
    }

    fn show(&self, progress: &AnalysisProgress) {
        self.row.set_visible(progress.total > 0);
        self.count.set_text(&count_text(progress));
        self.meter.set_value(meter_value(progress));
        // Off: the meter keeps its value, drawn without its fill colour.
        if progress.state == AnalysisState::Off {
            self.meter.add_css_class("off");
        } else {
            self.meter.remove_css_class("off");
        }
        self.state.set_text(&state_text(progress));
    }
}

/// The Track analysis group: the switch and the progress row.
pub(super) fn group(context: &Rc<Context>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.set_widget_name(ROW);
    let (line, switch) = switch_row(
        &tr("Analyse tracks in the background"),
        Some(&tr(
            "Finds the silence and the loudness of each track once, for crossfades, silence and the leveller, and notes how the track sounds, so that Helikodu can find tracks that sound alike. Tracks analysed by an earlier version are analysed once more. Nothing is written to the files.",
        )),
    );
    switch.set_widget_name("analyze-in-background");
    switch.set_active(context.settings().library.analyze_in_background);
    column.append(&line);
    let progress = Rc::new(ProgressRow::new());
    column.append(&progress.row);
    let last: Rc<Cell<Option<AnalysisProgress>>> = Rc::new(Cell::new(None));

    let weak = Rc::downgrade(context);
    {
        let (progress, last) = (Rc::downgrade(&progress), last.clone());
        switch.connect_active_notify(move |switch| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            let on = switch.is_active();
            if context.settings().library.analyze_in_background == on {
                return;
            }
            if !context.update(|s| s.library.analyze_in_background = on) {
                switch.set_active(!on);
                return;
            }
            // Turned off, it stops at once; turned on, the library service
            // says what it does.
            if !on && let (Some(progress), Some(mut shown)) = (progress.upgrade(), last.get()) {
                shown.state = AnalysisState::Off;
                last.set(Some(shown));
                progress.show(&shown);
            }
        });
    }

    let services = context.services.clone();
    {
        let (weak_progress, last) = (Rc::downgrade(&progress), last.clone());
        glib::spawn_future_local(async move {
            let found = gio::spawn_blocking(move || services.analysis_progress())
                .await
                .ok()
                .flatten();
            if let (Some(progress), Some(found)) = (weak_progress.upgrade(), found) {
                last.set(Some(found));
                progress.show(&found);
            }
        });
    }
    if let Some(playback) = &context.playback {
        let weak_progress = Rc::downgrade(&progress);
        playback.connect_event(move |update| {
            if let (Update::Analysis(found), Some(progress)) = (update, weak_progress.upgrade()) {
                last.set(Some(*found));
                progress.show(found);
            }
        });
    }
    // The row lives as long as its widgets.
    let keep = std::cell::RefCell::new(Some(progress));
    column.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    column
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(
        total: usize,
        analyzed: usize,
        failed: usize,
        state: AnalysisState,
    ) -> AnalysisProgress {
        AnalysisProgress {
            total,
            analyzed,
            failed,
            state,
        }
    }

    #[test]
    fn each_state_has_its_line() {
        let yielding = progress(18_402, 14_950, 12, AnalysisState::Yielding);
        assert_eq!(count_text(&yielding), "14,950 of 18,402 tracks analysed");
        assert_eq!(
            state_text(&yielding),
            "Working slowly while music plays from the same drive. 12 tracks could not be read."
        );
        assert_eq!(
            state_text(&progress(10, 4, 0, AnalysisState::Running)),
            "Working in the background."
        );
        assert_eq!(
            state_text(&progress(
                10,
                4,
                0,
                AnalysisState::Paused(PauseReason::Scanning)
            )),
            "Waits for the library scan to finish."
        );
        assert_eq!(
            state_text(&progress(
                10,
                4,
                1,
                AnalysisState::Paused(PauseReason::OnBattery)
            )),
            "Paused while the computer runs on battery. 1 track could not be read."
        );
        assert_eq!(
            state_text(&progress(10, 4, 0, AnalysisState::Off)),
            "Crossfades use their fixed length, and silence is not left out."
        );
        assert_eq!(
            state_text(&progress(10, 9, 1, AnalysisState::Idle)),
            "Every track is analysed. 1 track could not be read."
        );
        assert_eq!(
            state_text(&progress(10, 4, 0, AnalysisState::Idle)),
            "Starts in a moment."
        );
    }

    #[test]
    fn the_meter_is_full_once_every_track_is_done() {
        assert!((meter_value(&progress(10, 4, 0, AnalysisState::Running)) - 0.4).abs() < 1e-9);
        assert!((meter_value(&progress(10, 9, 1, AnalysisState::Idle)) - 1.0).abs() < 1e-9);
        // Off keeps the value.
        assert!((meter_value(&progress(10, 4, 0, AnalysisState::Off)) - 0.4).abs() < 1e-9);
        assert!((meter_value(&progress(0, 0, 0, AnalysisState::Idle)) - 1.0).abs() < 1e-9);
    }
}
