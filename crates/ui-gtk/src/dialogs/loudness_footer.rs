// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A loudness measurement in the sidebar footer, shown like a library scan:
//! a title with the share done, a bar, the tracks measured, and a stop
//! button. A toast says how it ended. Measurements start from Settings ›
//! Loudness ("Measure the rest") and from the context menu of tracks and
//! albums ("Measure loudness").

use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::Update;
use crate::backend::services::{LoudnessScope, LoudnessStatus, Services};
use crate::i18n::{fill, ntr, tr};
use crate::playback::Playback;
use crate::widgets::{icon_button, label};
use crate::window::MainWindow;

/// The footer's widgets.
pub(crate) struct LoudnessFooter {
    pub(crate) root: gtk::Box,
    share: gtk::Label,
    bar: gtk::ProgressBar,
    tracks: gtk::Label,
}

impl LoudnessFooter {
    fn new(services: Arc<dyn Services>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("hk-scan-footer");
        root.set_widget_name("loudness-footer");
        root.set_visible(false);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = label(&tr("Measuring loudness"), &[]);
        title.set_hexpand(true);
        head.append(&title);
        let share = label("", &["hk-numeric", "hk-t2"]);
        head.append(&share);
        let stop = icon_button("process-stop-symbolic", &tr("Stop measuring"));
        stop.add_css_class("hk-small-icon");
        stop.connect_clicked(move |button| {
            button.set_sensitive(false);
            stop_in_background(services.clone(), || {});
        });
        head.append(&stop);
        root.append(&head);
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("hk-scan-bar");
        bar.update_property(&[gtk::accessible::Property::Label(&tr("Measuring loudness"))]);
        root.append(&bar);
        let tracks = label("", &["hk-mono-meta", "hk-t3"]);
        root.append(&tracks);
        Self {
            root,
            share,
            bar,
            tracks,
        }
    }

    /// Follows `status`; hides when the measurement is over.
    fn update(&self, status: &LoudnessStatus) {
        if status.finished {
            self.root.set_visible(false);
            return;
        }
        if !self.root.is_visible() {
            if let Some(stop) = self.root.first_child().and_then(|head| head.last_child()) {
                stop.set_sensitive(true);
            }
            self.root.set_visible(true);
        }
        match (status.fraction(), status.total) {
            (Some(fraction), Some(total)) => {
                self.bar.set_fraction(fraction);
                let percent = (fraction * 100.0).floor() as u32;
                self.share
                    .set_text(&fill(&tr("{}%"), &[&percent.to_string()]));
                self.tracks.set_text(&fill(
                    &ntr("{} of {} track", "{} of {} tracks", total),
                    &[&status.done.to_string(), &total.to_string()],
                ));
            }
            _ => {
                self.bar.pulse();
                self.share.set_text("");
                self.tracks.set_text("");
            }
        }
    }
}

/// What a finished measurement says.
pub(crate) fn finished_message(status: &LoudnessStatus) -> (String, bool) {
    if let Some(error) = &status.error {
        return (
            fill(&tr("The loudness measurement stopped: {}"), &[error]),
            true,
        );
    }
    let measured = status.done.saturating_sub(status.failed);
    let text = if status.cancelled {
        fill(
            &ntr(
                "Measuring stopped. {} track was measured.",
                "Measuring stopped. {} tracks were measured.",
                measured,
            ),
            &[&measured.to_string()],
        )
    } else {
        fill(
            &ntr(
                "Loudness measured for {} track.",
                "Loudness measured for {} tracks.",
                measured,
            ),
            &[&measured.to_string()],
        )
    };
    if status.failed > 0 {
        let failed = fill(
            &ntr(
                "{} track could not be read.",
                "{} tracks could not be read.",
                status.failed,
            ),
            &[&status.failed.to_string()],
        );
        (format!("{text} {failed}"), true)
    } else {
        (text, false)
    }
}

/// Starts measuring `scope` on a worker thread (an earlier measurement is
/// stopped first, which can take a moment). `done` gets the error, if any.
pub(crate) fn measure_in_background(
    services: Arc<dyn Services>,
    scope: LoudnessScope,
    done: impl FnOnce(Option<String>) + 'static,
) {
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || services.measure_loudness(scope)).await;
        done(match result {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e.0),
            Err(_) => Some(tr("The measurement could not start.")),
        });
    });
}

/// Stops the running measurement on a worker thread, then calls `done`.
pub(crate) fn stop_in_background(services: Arc<dyn Services>, done: impl FnOnce() + 'static) {
    glib::spawn_future_local(async move {
        let _ = gio::spawn_blocking(move || services.stop_loudness()).await;
        done();
    });
}

/// Puts the footer under the sidebar's other content of `window` and makes
/// it follow the measurements.
pub(crate) fn attach(
    window: &MainWindow,
    sidebar: &gtk::Box,
    services: Arc<dyn Services>,
    playback: &Rc<Playback>,
) {
    let footer = LoudnessFooter::new(services);
    sidebar.append(&footer.root);
    let weak = window.downgrade();
    playback.connect_event(move |update| {
        let Update::Loudness(status) = update else {
            return;
        };
        footer.update(status);
        if status.finished
            && let Some(window) = weak.upgrade()
        {
            let (message, error) = finished_message(status);
            window.toast(&message, error);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_is_told_in_words() {
        let done = LoudnessStatus {
            done: 12,
            total: Some(12),
            finished: true,
            ..LoudnessStatus::default()
        };
        assert_eq!(
            finished_message(&done),
            ("Loudness measured for 12 tracks.".into(), false)
        );
        let stopped = LoudnessStatus {
            done: 3,
            failed: 1,
            cancelled: true,
            finished: true,
            ..done
        };
        let (text, error) = finished_message(&stopped);
        assert!(text.starts_with("Measuring stopped. 2 tracks"), "{text}");
        assert!(error);
    }
}
