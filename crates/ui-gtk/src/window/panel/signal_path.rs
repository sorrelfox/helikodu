// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Signal path tab of the panel: on a dark card, the lamp with its state
//! and one sentence, then the path from the file to the device; below, the
//! playback counters; at the bottom, the way to the audio settings and a
//! button that copies the path and the counters for a bug report.
//!
//! Everything shown is what the engine reported last. The counters change
//! only while the tab is visible.

use std::rc::{Rc, Weak};

use gtk::prelude::*;

use crate::backend::{PlaybackStats, SignalState};
use crate::format;
use crate::i18n::{fill, tr};
use crate::playback::{Change, Playback, PlayerView};
use crate::widgets::signal_path_list::{SignalPathList, diagnostics, sentence, state_label};
use crate::widgets::{SignalLamp, label};

/// The tab's widgets.
#[derive(Debug)]
pub(crate) struct SignalPathPage {
    root: gtk::Box,
    lamp: SignalLamp,
    state: gtk::Label,
    sentence: gtk::Label,
    list: Rc<SignalPathList>,
    buffer: gtk::Label,
    underruns: gtk::Label,
    silence: gtk::Label,
    playback: Weak<Playback>,
}

/// A row of the counters: the name in `t2`, the value in mono at the right.
fn stat_row(grid: &gtk::Box, name: &str) -> gtk::Label {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("hk-stat-line");
    let title = label(name, &["hk-t2"]);
    title.set_hexpand(true);
    row.append(&title);
    let value = label("", &["hk-mono", "hk-stat-value"]);
    value.set_xalign(1.0);
    row.append(&value);
    row.set_accessible_role(gtk::AccessibleRole::Group);
    grid.append(&row);
    value
}

/// The counters in words: `184 of 200 ms`, `0`, and the silence played in
/// place of missing samples, at `rate`.
pub(crate) fn stats_texts(stats: Option<&PlaybackStats>, rate: Option<u32>) -> [String; 3] {
    let Some(stats) = stats else {
        return [String::new(), String::new(), String::new()];
    };
    let silence = match rate {
        Some(rate) if rate > 0 => {
            let ms = stats.silent_frames.saturating_mul(1000) / u64::from(rate);
            fill(&tr("{} ms"), &[&format::count(ms)])
        }
        _ => format::count(stats.silent_frames),
    };
    [
        fill(
            &tr("{} of {} ms"),
            &[&stats.buffered_ms.to_string(), &stats.buffer_ms.to_string()],
        ),
        format::count(stats.underruns),
        silence,
    ]
}

impl SignalPathPage {
    /// The tab, following `playback`.
    pub(crate) fn new(playback: &Rc<Playback>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-signal-page");

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // Dark in both colour schemes, like the deck.
        card.add_css_class("hk-glass");
        card.add_css_class("hk-path-card");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let lamp = SignalLamp::default();
        lamp.set_valign(gtk::Align::Center);
        head.append(&lamp);
        let state = label("", &["hk-signal", "large", "idle"]);
        state.set_ellipsize(gtk::pango::EllipsizeMode::None);
        head.append(&state);
        card.append(&head);
        let sentence = gtk::Label::new(None);
        sentence.set_wrap(true);
        sentence.set_xalign(0.0);
        sentence.add_css_class("hk-path-sentence");
        card.append(&sentence);
        let list = SignalPathList::new();
        card.append(list.widget());
        content.append(&card);

        let stats = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stats.add_css_class("hk-path-stats");
        stats.append(&label(&tr("Playback"), &["hk-section-label"]));
        let buffer = stat_row(&stats, &tr("Buffer"));
        let underruns = stat_row(&stats, &tr("Underruns"));
        let silence = stat_row(&stats, &tr("Silence inserted"));
        content.append(&stats);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&content));
        root.append(&scroller);

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        footer.add_css_class("hk-panel-footer");
        footer.set_halign(gtk::Align::Fill);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);
        let settings = gtk::Button::with_label(&tr("Audio settings"));
        settings.add_css_class("hk-link");
        settings.set_valign(gtk::Align::Center);
        settings.set_action_name(Some("app.preferences-output"));
        footer.append(&settings);
        let copy = gtk::Button::with_label(&tr("Copy diagnostics"));
        copy.add_css_class("hk-link");
        copy.set_valign(gtk::Align::Center);
        copy.set_tooltip_text(Some(&tr(
            "Copy the signal path and the playback counters, for a bug report",
        )));
        footer.append(&copy);
        root.append(&footer);

        let page = Rc::new(Self {
            root,
            lamp,
            state,
            sentence,
            list,
            buffer,
            underruns,
            silence,
            playback: Rc::downgrade(playback),
        });
        let weak = Rc::downgrade(&page);
        copy.connect_clicked(move |button| {
            if let Some(page) = weak.upgrade() {
                page.copy_diagnostics(button);
            }
        });
        let weak = Rc::downgrade(&page);
        playback.connect(move |view, change| {
            let Some(page) = weak.upgrade() else { return };
            match change {
                Change::Signal | Change::Output | Change::State => page.show_path(view),
                // A few times a second: only while someone can see them.
                Change::Stats if page.root.is_mapped() => page.show_stats(view),
                _ => {}
            }
        });
        let weak = Rc::downgrade(&page);
        page.root.connect_map(move |_| {
            if let Some(page) = weak.upgrade()
                && let Some(playback) = page.playback.upgrade()
            {
                page.show_stats(&playback.view());
            }
        });
        page
    }

    /// The tab's widget.
    pub(crate) fn widget(&self) -> &gtk::Box {
        &self.root
    }

    fn show_path(&self, view: &PlayerView) {
        let path = view.signal.as_deref();
        // With nothing heard the lamp is idle, whatever the button says of the
        // device (not connected).
        let signal = match path {
            Some(path) if view.output.signal != SignalState::NotConnected => SignalState::of(path),
            Some(_) => SignalState::NotConnected,
            None if view.output.signal == SignalState::NotConnected => SignalState::NotConnected,
            None => SignalState::Idle,
        };
        self.lamp.set_state(signal);
        let text = state_label(signal, path).unwrap_or_default();
        self.state.set_text(&text);
        for class in ["idle", "bit-perfect", "processed"] {
            self.state.remove_css_class(class);
        }
        self.state.add_css_class(match signal {
            SignalState::BitPerfect => "bit-perfect",
            SignalState::Processed { .. } => "processed",
            _ => "idle",
        });
        self.sentence.set_text(&sentence(path));
        self.list.show(path, &view.output.device);
    }

    fn show_stats(&self, view: &PlayerView) {
        let rate = view.signal.as_ref().map(|p| p.device.format.rate.hz());
        let [buffer, underruns, silence] = stats_texts(view.stats.as_ref(), rate);
        self.buffer.set_text(&buffer);
        self.underruns.set_text(&underruns);
        self.silence.set_text(&silence);
    }

    fn copy_diagnostics(&self, button: &gtk::Button) {
        let Some(playback) = self.playback.upgrade() else {
            return;
        };
        let view = playback.view();
        let mut text = diagnostics(view.signal.as_deref());
        if let Some(stats) = view.stats {
            text.push_str(&format!(
                "\nbuffer {}/{} ms, underruns {} (total {}), frames {}, silent frames {}",
                stats.buffered_ms,
                stats.buffer_ms,
                stats.underruns,
                stats.total_underruns,
                stats.frames_delivered,
                stats.silent_frames
            ));
        }
        button.clipboard().set_text(&text);
        if let Some(window) = button
            .root()
            .and_then(|r| r.downcast::<crate::window::MainWindow>().ok())
        {
            window.toast(&tr("The signal path was copied."), false);
        }
    }

    /// What the tab shows, in words: the state, the sentence, each row as
    /// "stage: value", and the counters.
    pub(crate) fn texts(&self) -> Vec<String> {
        let mut texts = vec![
            self.state.text().to_string(),
            self.sentence.text().to_string(),
        ];
        texts.extend(
            self.list
                .texts()
                .into_iter()
                .map(|(name, value, node)| format!("{name}: {value} [{node:?}]")),
        );
        for (name, value) in [
            ("Buffer", &self.buffer),
            ("Underruns", &self.underruns),
            ("Silence", &self.silence),
        ] {
            texts.push(format!("{name}: {}", value.text()));
        }
        texts
    }
}
