// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The toast that tells when the engine leaves convolution out because the
//! impulse response for the playing rate cannot be read. It shows once for a
//! file and rate (again only when the file or the choice changes), stays until
//! it is dismissed or the file works, and its "Details…" opens Settings › DSP
//! and EQ at the Convolution stage.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::SystemTime;

use gtk::prelude::*;

use super::{MainWindow, ToastLead};
use crate::backend::dsp::entry_for;
use crate::backend::{Backend, SignalPath, StageKind, StageState};
use crate::i18n::{fill, tr};
use crate::playback::{Change, Playback};

/// A file that cannot be read for a rate: the rate of the file, the file and
/// when it was last changed.
type Problem = (u32, Option<PathBuf>, Option<SystemTime>);

#[derive(Default)]
struct State {
    /// The problems told of.
    told: HashSet<Problem>,
    /// The text of the toast and the playing rate it was shown at.
    showing: Option<(String, u32)>,
}

/// Whether the engine leaves convolution out in `path`.
fn left_out(path: &SignalPath) -> bool {
    path.stage(StageKind::Convolution)
        .is_some_and(|s| s.state == StageState::Bypassed && s.detail.starts_with("left out"))
}

impl MainWindow {
    /// Shows the toast when convolution is left out. Called once from `setup`.
    pub(super) fn watch_convolution(&self, backend: &Backend, playback: &Rc<Playback>) {
        let services = backend.services();
        let state = Rc::new(RefCell::new(State::default()));
        let weak = self.downgrade();
        playback.connect(move |view, change| {
            if change != Change::Signal {
                return;
            }
            let Some(path) = view.signal.as_deref() else {
                return;
            };
            let Some(window) = weak.upgrade() else {
                return;
            };
            let rate = path.device.format.rate.hz();
            if !left_out(path) {
                // Working again at the rate the toast told of: the file was fixed.
                let mut state = state.borrow_mut();
                if let Some((text, at)) = state.showing.take() {
                    if at == rate {
                        window.dismiss_toast_if(&text);
                    } else {
                        state.showing = Some((text, at));
                    }
                }
                return;
            }
            // The file convolution would use at this rate.
            let settings = services.settings();
            let entries = services.dsp().impulse_files(&settings.dsp.convolution);
            let entry = entry_for(&entries, rate);
            let file = entry.map(|e| e.path.clone());
            let changed = file
                .as_ref()
                .and_then(|f| std::fs::metadata(f).ok())
                .and_then(|m| m.modified().ok());
            let file_rate = entry.map_or(rate, |e| e.rate_hz);
            let problem = (file_rate, file, changed);
            if !state.borrow_mut().told.insert(problem) {
                return;
            }
            let text = fill(
                &tr("The impulse response for {} cannot be read"),
                &[&fill(&tr("{} kHz"), &[&crate::format::khz(file_rate)])],
            );
            state.borrow_mut().showing = Some((text.clone(), rate));
            let weak = window.downgrade();
            window.toast_marked(
                &text,
                ToastLead::Warning,
                Some(&tr("Details…")),
                true,
                move || {
                    if let Some(window) = weak.upgrade() {
                        use crate::dialogs::settings::dsp::{Stage, open_stage};
                        open_stage(&window, Stage::Convolution);
                    }
                },
            );
        });
    }
}
