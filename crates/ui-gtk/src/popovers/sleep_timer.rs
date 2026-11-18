// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sleep timer popover, above the deck's sleep chip (or where it would
//! be, while no timer runs): the head with the clock time playback stops at,
//! a grid of lengths (15 minutes to an hour and a half, or a typed number of
//! minutes), what happens when the time is up, and a footer with the time
//! left, "+10 min" and "Turn off" — or only "Start" while no timer runs.
//!
//! While a timer runs, choosing another length or another "when" starts it
//! again with that choice.

use std::cell::Cell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::{Backend, PlayerRequest, SleepWhen};
use crate::dialogs::common::{bind_segmented, segmented};
use crate::i18n::{fill, tr};
use crate::playback::{Playback, PlayerView};
use crate::style::tokens::metric::sleep_timer as metric;
use crate::widgets::label;

/// The lengths of the grid, in minutes.
const LENGTHS: [u64; 5] = [15, 30, 45, 60, 90];
/// The length chosen before the user chooses one.
const DEFAULT_MINUTES: u64 = 30;
/// What "+10 min" adds.
const EXTEND: Duration = Duration::from_secs(600);

fn minutes(m: u64) -> Duration {
    Duration::from_secs(m * 60)
}

/// A length as its chip names it.
fn length_name(m: u64) -> String {
    match m {
        60 => tr("1 hour"),
        90 => tr("1.5 hours"),
        _ => fill(&tr("{} min"), &[&m.to_string()]),
    }
}

/// The sleep timer popover.
#[derive(Debug)]
pub(crate) struct SleepPopover {
    popover: gtk::Popover,
    stops_at: gtk::Label,
    choices: Vec<gtk::Button>,
    other_row: gtk::Revealer,
    other_minutes: gtk::SpinButton,
    when: Vec<gtk::ToggleButton>,
    explanation: gtk::Label,
    left: gtk::Label,
    running: gtk::Box,
    start: gtk::Button,
    backend: Backend,
    playback: Weak<Playback>,
    /// The length chosen, for "Start".
    chosen: Cell<Duration>,
    /// Set while the controls follow the player, so they do not echo.
    updating: Cell<bool>,
}

impl SleepPopover {
    /// A popover on `parent`; it follows `playback` and sends to `backend`.
    pub(crate) fn new(
        parent: &impl IsA<gtk::Widget>,
        backend: &Backend,
        playback: &Rc<Playback>,
    ) -> Rc<Self> {
        let popover = gtk::Popover::new();
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Top);
        popover.add_css_class("hk-popover");
        popover.add_css_class("hk-glass");
        popover.set_parent(parent);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("hk-sleep");
        content.set_size_request(metric::POPOVER_WIDTH - 2, -1);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.add_css_class("hk-sleep-head");
        head.set_size_request(-1, metric::HEADER);
        let title = label(&tr("Sleep timer"), &["hk-sleep-title"]);
        title.set_hexpand(true);
        head.append(&title);
        let stops_at = label("", &["hk-sleep-stops", "hk-mono"]);
        head.append(&stops_at);
        content.append(&head);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        body.add_css_class("hk-sleep-body");
        body.append(&label(&tr("Stop after"), &["hk-section-label"]));
        let grid = gtk::Grid::new();
        grid.set_row_spacing(metric::CHOICE_GAP as u32);
        grid.set_column_spacing(metric::CHOICE_GAP as u32);
        grid.set_column_homogeneous(true);
        let mut choices = Vec::new();
        let names: Vec<String> = LENGTHS
            .iter()
            .map(|m| length_name(*m))
            .chain([tr("Other…")])
            .collect();
        for (i, name) in names.iter().enumerate() {
            let button = gtk::Button::with_label(name);
            button.add_css_class("hk-sleep-choice");
            button.set_size_request(-1, metric::CHOICE_HEIGHT);
            grid.attach(&button, (i % 3) as i32, (i / 3) as i32, 1, 1);
            choices.push(button);
        }
        body.append(&grid);
        let other_row = gtk::Revealer::new();
        let other_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        other_line.add_css_class("hk-sleep-other");
        let other_minutes = gtk::SpinButton::with_range(1.0, 720.0, 5.0);
        other_minutes.set_value(DEFAULT_MINUTES as f64);
        other_minutes.update_property(&[gtk::accessible::Property::Label(&tr(
            "Minutes until playback stops",
        ))]);
        other_line.append(&other_minutes);
        let unit = label(&tr("minutes"), &["hk-t2"]);
        unit.set_hexpand(true);
        other_line.append(&unit);
        let set = gtk::Button::with_label(&tr("Set"));
        set.add_css_class("hk-btn");
        set.add_css_class("small");
        other_line.append(&set);
        other_row.set_child(Some(&other_line));
        body.append(&other_row);

        body.append(&label(&tr("When the time is up"), &["hk-section-label"]));
        let (seg, when) = segmented(&[tr("At once"), tr("After the track")]);
        seg.set_halign(gtk::Align::Fill);
        seg.set_homogeneous(true);
        seg.add_css_class("hk-sleep-when");
        body.append(&seg);
        let explanation = label("", &["hk-sleep-note"]);
        explanation.set_ellipsize(gtk::pango::EllipsizeMode::None);
        explanation.set_single_line_mode(false);
        explanation.set_wrap(true);
        explanation.set_xalign(0.0);
        explanation.set_max_width_chars(36);
        body.append(&explanation);
        content.append(&body);

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        footer.add_css_class("hk-sleep-footer");
        footer.set_size_request(-1, metric::FOOTER);
        let running = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        running.set_hexpand(true);
        let left = label("", &["hk-sleep-left", "hk-mono"]);
        let left_word = label(&tr("left"), &["hk-sleep-left-word", "hk-mono"]);
        left_word.set_hexpand(true);
        left_word.set_xalign(0.0);
        running.append(&left);
        running.append(&left_word);
        running.set_valign(gtk::Align::Center);
        let extend = gtk::Button::with_label(&tr("+10 min"));
        extend.add_css_class("hk-btn");
        extend.add_css_class("small");
        running.append(&extend);
        let turn_off = gtk::Button::with_label(&tr("Turn off"));
        turn_off.add_css_class("hk-btn");
        turn_off.add_css_class("small");
        running.append(&turn_off);
        footer.append(&running);
        let start = gtk::Button::with_label(&tr("Start"));
        start.add_css_class("hk-btn");
        start.add_css_class("primary");
        start.add_css_class("small");
        start.set_hexpand(true);
        start.set_halign(gtk::Align::End);
        start.set_valign(gtk::Align::Center);
        footer.append(&start);
        content.append(&footer);
        popover.set_child(Some(&content));

        let this = Rc::new(Self {
            popover,
            stops_at,
            choices,
            other_row,
            other_minutes,
            when,
            explanation,
            left,
            running,
            start,
            backend: backend.clone(),
            playback: Rc::downgrade(playback),
            chosen: Cell::new(minutes(DEFAULT_MINUTES)),
            updating: Cell::new(false),
        });
        for (i, button) in this.choices.iter().enumerate() {
            let weak = Rc::downgrade(&this);
            button.connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else { return };
                match LENGTHS.get(i) {
                    Some(m) => {
                        this.other_row.set_reveal_child(false);
                        this.choose(minutes(*m));
                    }
                    None => {
                        this.other_row.set_reveal_child(true);
                        this.mark(None);
                        this.other_minutes.grab_focus();
                    }
                }
            });
        }
        let weak = Rc::downgrade(&this);
        let apply_other = move || {
            if let Some(this) = weak.upgrade() {
                let m = this.other_minutes.value().round().max(1.0) as u64;
                this.choose(minutes(m));
            }
        };
        let apply = apply_other.clone();
        set.connect_clicked(move |_| apply());
        this.other_minutes.connect_activate(move |_| apply_other());
        let weak = Rc::downgrade(&this);
        bind_segmented(&this.when, 0, move |i| {
            let Some(this) = weak.upgrade() else { return };
            this.explain();
            if this.updating.get() {
                return;
            }
            // A running timer starts again with the time it has left.
            if let Some(left) = this.view().and_then(|v| v.sleep_left()) {
                let left = Duration::from_secs(left.as_secs().max(1));
                this.backend.send(PlayerRequest::StartSleepTimer {
                    length: left,
                    when: if i == 1 {
                        SleepWhen::AfterTrack
                    } else {
                        SleepWhen::AtOnce
                    },
                });
            }
        });
        let b = backend.clone();
        extend.connect_clicked(move |_| b.send(PlayerRequest::ExtendSleepTimer(EXTEND)));
        let b = backend.clone();
        turn_off.connect_clicked(move |_| b.send(PlayerRequest::CancelSleepTimer));
        let weak = Rc::downgrade(&this);
        this.start.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.backend.send(PlayerRequest::StartSleepTimer {
                    length: this.chosen.get(),
                    when: this.when_chosen(),
                });
            }
        });
        let weak = Rc::downgrade(&this);
        this.popover.connect_show(move |_| {
            if let Some(this) = weak.upgrade()
                && let Some(view) = this.view()
            {
                this.show(&view);
            }
        });
        this.explain();
        this.mark(Some(this.chosen.get()));
        this
    }

    /// The popover.
    pub(crate) fn popover(&self) -> &gtk::Popover {
        &self.popover
    }

    fn view(&self) -> Option<PlayerView> {
        self.playback.upgrade().map(|p| p.view())
    }

    fn when_chosen(&self) -> SleepWhen {
        if self.when.get(1).is_some_and(ToggleButtonExt::is_active) {
            SleepWhen::AfterTrack
        } else {
            SleepWhen::AtOnce
        }
    }

    /// Chooses a length: a running timer starts again with it.
    fn choose(&self, length: Duration) {
        self.chosen.set(length);
        self.mark(Some(length));
        if self.view().is_some_and(|v| v.sleep.is_some()) {
            self.backend.send(PlayerRequest::StartSleepTimer {
                length,
                when: self.when_chosen(),
            });
        }
    }

    /// Marks the chip of `length`: one of the grid, or "Other…".
    fn mark(&self, length: Option<Duration>) {
        let index = match length {
            Some(length) => LENGTHS
                .iter()
                .position(|m| minutes(*m) == length)
                .unwrap_or(LENGTHS.len()),
            None => LENGTHS.len(),
        };
        for (i, button) in self.choices.iter().enumerate() {
            if i == index {
                button.add_css_class("chosen");
            } else {
                button.remove_css_class("chosen");
            }
        }
    }

    fn explain(&self) {
        self.explanation.set_text(&match self.when_chosen() {
            SleepWhen::AfterTrack => {
                tr("Playback pauses when the track that plays then has ended.")
            }
            SleepWhen::AtOnce => tr(
                "The sound fades over the last 30 seconds, then playback pauses. On a bit-perfect path it pauses without a fade.",
            ),
        });
    }

    /// Follows the player: the timer, its time left and the clock time it
    /// stops at. Called on each change and once a second while it runs.
    pub(crate) fn show(&self, view: &PlayerView) {
        let running = view.sleep.map(|(t, _)| t);
        self.running.set_visible(running.is_some());
        self.start.set_visible(running.is_none());
        match (running, view.sleep_left()) {
            (Some(timer), Some(left)) => {
                self.left.set_text(&crate::format::duration(left));
                let stops = glib::DateTime::now_local()
                    .and_then(|now| now.add_seconds(left.as_secs_f64()))
                    .and_then(|at| at.format("%H:%M"))
                    .map(|t| fill(&tr("Stops at {}"), &[&t]))
                    .unwrap_or_default();
                self.stops_at.set_text(&stops);
                self.updating.set(true);
                let index = usize::from(timer.when == SleepWhen::AfterTrack);
                if let Some(button) = self.when.get(index) {
                    button.set_active(true);
                }
                self.updating.set(false);
                self.mark(Some(timer.length));
            }
            _ => {
                self.stops_at.set_text("");
                self.mark(Some(self.chosen.get()));
            }
        }
    }

    /// Opens the popover pointing at `area` of its parent.
    pub(crate) fn open(&self, area: Option<gtk::gdk::Rectangle>) {
        self.popover.set_pointing_to(area.as_ref());
        self.popover.popup();
    }
}
