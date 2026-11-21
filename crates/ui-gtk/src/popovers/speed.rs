// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The speed popover, above the deck's speed chip (or where it would be, while
//! the speed is normal): the head with the speed, a slider from half to twice
//! the speed with five quick choices, whether the pitch stays or follows the
//! speed, the pitch shift of the Pitch stage, "Start with this speed next
//! time", and a footer with the listening time until the track ends and "Back
//! to normal".
//!
//! The popover changes the speed of the running program. With "Start with this
//! speed next time" ticked, the speed and the choice for the pitch are also
//! written to the settings, now and at every later change; unticking writes the
//! normal speed there.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;

use crate::backend::{Backend, PlaybackSpeed, PlayerRequest};
use crate::dialogs::common::{bind_segmented, segmented};
use crate::i18n::{fill, tr};
use crate::playback::{Playback, PlayerView};
use crate::style::tokens::metric::sleep_timer as metric;
use crate::widgets::label;

/// The slowest and the fastest speed, and the step of the slider.
const MIN: f64 = 0.5;
const MAX: f64 = 2.0;
const STEP: f64 = 0.05;
/// The quick choices under the slider.
const CHOICES: [f64; 5] = [0.75, 1.0, 1.25, 1.5, 2.0];

/// A speed as the chip and the popover write it: `1.25×`, `2×`, `0.75×`.
pub(crate) fn speed_text(rate: f64) -> String {
    let text = format!("{rate:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.').to_owned();
    format!("{}×", crate::format::localized(text))
}

/// A speed for assistive technology: "1.25 times".
fn speed_words(rate: f64) -> String {
    let text = format!("{rate:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.').to_owned();
    fill(&tr("{} times"), &[&crate::format::localized(text)])
}

/// The accessible name of the speed chip: both settings, or why the speed
/// does not apply.
pub(crate) fn chip_name(view: &PlayerView) -> String {
    let speed = speed_words(view.speed.rate);
    if view.speed_not_applied() {
        fill(
            &tr("Speed: {}, not applied while playback is bit-perfect"),
            &[&speed],
        )
    } else if view.speed.keep_pitch {
        fill(&tr("Speed: {}, the pitch stays"), &[&speed])
    } else {
        fill(&tr("Speed: {}, the pitch follows the speed"), &[&speed])
    }
}

/// A pitch shift in semitones: "0 semitones", "+3 semitones".
fn semitones_text(value: f64) -> String {
    let n = value.round() as i64;
    let number = match n {
        n if n > 0 => format!("+{n}"),
        n if n < 0 => format!("−{}", -n),
        _ => "0".to_owned(),
    };
    fill(
        &crate::i18n::ntr(
            "{} semitone",
            "{} semitones",
            usize::try_from(n.unsigned_abs()).unwrap_or(usize::MAX),
        ),
        &[&number],
    )
}

/// The speed popover.
#[derive(Debug)]
pub(crate) struct SpeedPopover {
    popover: gtk::Popover,
    shown: gtk::Label,
    not_applied: gtk::Box,
    scale: gtk::Scale,
    choices: Vec<gtk::Button>,
    pitch: Vec<gtk::ToggleButton>,
    explanation: gtk::Label,
    shift: gtk::Scale,
    shift_text: gtk::Label,
    keep: gtk::CheckButton,
    keep_note: gtk::Label,
    ends: gtk::Label,
    backend: Backend,
    playback: Weak<Playback>,
    /// Set while the controls follow the player, so they do not echo.
    updating: Cell<bool>,
}

/// A row of three labels under a slider, the middle one at `middle` of the
/// way (in sixths), as the slider's scale.
fn scale_marks(texts: [&str; 3], middle: i32) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_column_homogeneous(true);
    grid.add_css_class("hk-speed-marks");
    let start = label(texts[0], &["hk-mono", "hk-speed-mark"]);
    start.set_halign(gtk::Align::Start);
    grid.attach(&start, 0, 0, 1, 1);
    let centre = label(texts[1], &["hk-mono", "hk-speed-mark"]);
    centre.set_halign(gtk::Align::Center);
    centre.set_xalign(0.5);
    grid.attach(&centre, middle - 1, 0, 2, 1);
    let end = label(texts[2], &["hk-mono", "hk-speed-mark"]);
    end.set_halign(gtk::Align::End);
    end.set_xalign(1.0);
    grid.attach(&end, 5, 0, 1, 1);
    // The empty columns keep their share of the width.
    for column in 0..6 {
        if grid.child_at(column, 0).is_none() {
            grid.attach(
                &gtk::Box::new(gtk::Orientation::Horizontal, 0),
                column,
                0,
                1,
                1,
            );
        }
    }
    grid.update_property(&[gtk::accessible::Property::Label("")]);
    grid.set_accessible_role(gtk::AccessibleRole::Presentation);
    grid
}

fn section(text: &str) -> gtk::Label {
    label(text, &["hk-section-label", "hk-speed-section"])
}

fn note(text: &str) -> gtk::Label {
    let note = label(text, &["hk-sleep-note"]);
    note.set_ellipsize(gtk::pango::EllipsizeMode::None);
    note.set_single_line_mode(false);
    note.set_wrap(true);
    note.set_xalign(0.0);
    note.set_max_width_chars(36);
    note
}

impl SpeedPopover {
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
        popover.set_widget_name("speed-popover");
        crate::style::glass_when_dark(&popover);
        popover.set_parent(parent);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("hk-sleep");
        content.add_css_class("hk-speed");
        content.set_size_request(metric::POPOVER_WIDTH - 2, -1);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.add_css_class("hk-sleep-head");
        head.set_size_request(-1, metric::HEADER);
        let title = label(&tr("Speed"), &["hk-sleep-title"]);
        title.set_hexpand(true);
        head.append(&title);
        let shown = label("", &["hk-mono", "hk-speed-value"]);
        head.append(&shown);
        content.append(&head);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("hk-sleep-body");
        let not_applied = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        not_applied.add_css_class("hk-speed-not-applied");
        let info = gtk::Image::from_icon_name("helikodu-info-symbolic");
        info.set_pixel_size(14);
        info.set_valign(gtk::Align::Start);
        not_applied.append(&info);
        not_applied.append(&note(&tr(
            "Not applied now. Playback is bit-perfect, so tracks play at their normal speed.",
        )));
        not_applied.set_accessible_role(gtk::AccessibleRole::Status);
        not_applied.set_visible(false);
        body.append(&not_applied);

        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, MIN, MAX, STEP);
        scale.set_draw_value(false);
        scale.set_round_digits(2);
        scale.add_mark(1.0, gtk::PositionType::Top, None);
        scale.add_css_class("hk-speed-scale");
        scale.update_property(&[gtk::accessible::Property::Label(&tr("Playback speed"))]);
        body.append(&scale);
        body.append(&scale_marks(["0.5×", "1×", "2×"], 2));
        let quick = gtk::Box::new(gtk::Orientation::Horizontal, metric::CHOICE_GAP);
        quick.set_homogeneous(true);
        quick.add_css_class("hk-speed-choices");
        quick.update_property(&[gtk::accessible::Property::Label(&tr(
            "Speed, quick choices",
        ))]);
        let mut choices = Vec::new();
        for rate in CHOICES {
            let button = gtk::Button::with_label(&speed_text(rate));
            button.add_css_class("hk-sleep-choice");
            button.add_css_class("hk-speed-choice");
            button.set_size_request(-1, metric::CHOICE_HEIGHT);
            quick.append(&button);
            choices.push(button);
        }
        body.append(&quick);

        body.append(&section(&tr("The pitch")));
        let (seg, pitch) = segmented(&[tr("Stays"), tr("Follows the speed")]);
        seg.set_halign(gtk::Align::Fill);
        seg.set_homogeneous(true);
        seg.add_css_class("hk-sleep-when");
        seg.update_property(&[gtk::accessible::Property::Label(&tr(
            "What the pitch does when the speed changes",
        ))]);
        body.append(&seg);
        let explanation = note("");
        explanation.add_css_class("hk-speed-explanation");
        body.append(&explanation);

        let shift_head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        shift_head.add_css_class("hk-speed-shift-head");
        let shift_title = section(&tr("Pitch shift"));
        shift_title.set_hexpand(true);
        shift_head.append(&shift_title);
        let shift_text = label(&semitones_text(0.0), &["hk-mono", "hk-speed-shift"]);
        shift_head.append(&shift_text);
        body.append(&shift_head);
        let shift = gtk::Scale::with_range(gtk::Orientation::Horizontal, -12.0, 12.0, 1.0);
        shift.set_draw_value(false);
        shift.set_round_digits(0);
        shift.set_has_origin(false);
        shift.add_mark(0.0, gtk::PositionType::Top, None);
        shift.add_css_class("hk-speed-scale");
        shift.update_property(&[gtk::accessible::Property::Label(&tr(
            "Pitch shift in semitones",
        ))]);
        body.append(&shift);
        body.append(&scale_marks(["−12", "0", "+12"], 3));

        // The box at the top of its two lines, which toggle it too.
        let keep_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        keep_row.add_css_class("hk-speed-keep");
        let keep = gtk::CheckButton::new();
        keep.set_valign(gtk::Align::Start);
        keep_row.append(&keep);
        let keep_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        keep_text.set_hexpand(true);
        let title_text = tr("Start with this speed next time");
        let keep_title = label(&title_text, &[]);
        keep_title.set_ellipsize(gtk::pango::EllipsizeMode::None);
        keep_title.set_wrap(true);
        keep_text.append(&keep_title);
        let keep_note = note("");
        keep_text.append(&keep_note);
        keep_row.append(&keep_text);
        keep.update_property(&[gtk::accessible::Property::Label(&title_text)]);
        let click = gtk::GestureClick::new();
        let toggle = keep.downgrade();
        click.connect_released(move |_, _, _, _| {
            if let Some(keep) = toggle.upgrade() {
                keep.set_active(!keep.is_active());
            }
        });
        keep_text.add_controller(click);
        body.append(&keep_row);
        content.append(&body);

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        footer.add_css_class("hk-sleep-footer");
        footer.set_size_request(-1, metric::FOOTER);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.set_hexpand(true);
        line.set_valign(gtk::Align::Center);
        let ends = label("", &["hk-sleep-left", "hk-mono"]);
        crate::widgets::sentence(
            &line,
            // Translators: the listening time until the playing track ends at
            // the speed chosen; {} is the time ("2:34"), shown larger.
            &tr("Ends in {}"),
            &[ends.upcast_ref()],
            &["hk-sleep-left-word"],
        );
        footer.append(&line);
        let normal = gtk::Button::with_label(&tr("Back to normal"));
        normal.add_css_class("hk-btn");
        normal.add_css_class("small");
        normal.set_valign(gtk::Align::Center);
        footer.append(&normal);
        content.append(&footer);
        popover.set_child(Some(&content));

        let this = Rc::new(Self {
            popover,
            shown,
            not_applied,
            scale,
            choices,
            pitch,
            explanation,
            shift,
            shift_text,
            keep,
            keep_note,
            ends,
            backend: backend.clone(),
            playback: Rc::downgrade(playback),
            updating: Cell::new(false),
        });

        let weak = Rc::downgrade(&this);
        this.scale.connect_value_changed(move |scale| {
            let Some(this) = weak.upgrade() else { return };
            if this.updating.get() {
                return;
            }
            let rate = ((scale.value() / STEP).round() * STEP).clamp(MIN, MAX);
            this.set_speed(rate, this.keep_pitch());
        });
        for (button, rate) in this.choices.iter().zip(CHOICES) {
            let weak = Rc::downgrade(&this);
            button.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.set_speed(rate, this.keep_pitch());
                }
            });
        }
        let weak = Rc::downgrade(&this);
        bind_segmented(&this.pitch, 0, move |_| {
            let Some(this) = weak.upgrade() else { return };
            this.explain();
            if this.updating.get() {
                return;
            }
            let rate = this.view().map_or(1.0, |v| v.speed.rate);
            this.set_speed(rate, this.keep_pitch());
        });
        let weak = Rc::downgrade(&this);
        this.shift.connect_value_changed(move |scale| {
            let Some(this) = weak.upgrade() else { return };
            let value = scale.value().round();
            this.shift_text.set_text(&semitones_text(value));
            if this.updating.get() {
                return;
            }
            this.set_pitch_shift(value);
        });
        let weak = Rc::downgrade(&this);
        this.keep.connect_toggled(move |keep| {
            let Some(this) = weak.upgrade() else { return };
            this.explain_keep();
            if this.updating.get() {
                return;
            }
            let speed = if keep.is_active() {
                this.view().map(|v| v.speed).unwrap_or_default()
            } else {
                PlaybackSpeed {
                    rate: 1.0,
                    keep_pitch: this.keep_pitch(),
                }
            };
            this.write_settings(speed);
        });
        let weak = Rc::downgrade(&this);
        normal.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.set_speed(1.0, this.keep_pitch());
            }
        });
        let weak = Rc::downgrade(&this);
        this.popover.connect_show(move |_| {
            if let Some(this) = weak.upgrade() {
                this.read_settings();
                if let Some(view) = this.view() {
                    this.show(&view);
                }
            }
        });
        this.explain();
        this.explain_keep();
        this
    }

    /// The popover.
    pub(crate) fn popover(&self) -> &gtk::Popover {
        &self.popover
    }

    fn view(&self) -> Option<PlayerView> {
        self.playback.upgrade().map(|p| p.view())
    }

    fn keep_pitch(&self) -> bool {
        !self.pitch.get(1).is_some_and(ToggleButtonExt::is_active)
    }

    /// Plays at `rate`, and writes it to the settings while "Start with this
    /// speed next time" is ticked.
    fn set_speed(&self, rate: f64, keep_pitch: bool) {
        self.backend
            .send(PlayerRequest::SetSpeed { rate, keep_pitch });
        if self.keep.is_active() {
            self.write_settings(PlaybackSpeed { rate, keep_pitch });
        }
    }

    fn write_settings(&self, speed: PlaybackSpeed) {
        let services = self.backend.services();
        let table = helikodu_model::settings::SpeedSettings::at(speed.rate, speed.keep_pitch);
        if let Err(error) = services.update_settings(&mut |s| s.dsp.speed = table) {
            log::warn!("the speed could not be saved: {}", error.0);
        }
    }

    fn set_pitch_shift(&self, semitones: f64) {
        let services = self.backend.services();
        let result = services.update_settings(&mut |s| {
            s.dsp.pitch.semitones = semitones;
            if semitones != 0.0 {
                s.dsp.pitch.enabled = true;
            }
        });
        if let Err(error) = result {
            log::warn!("the pitch shift could not be saved: {}", error.0);
        }
    }

    /// The pitch shift and the check box, from the settings.
    fn read_settings(&self) {
        let settings = self.backend.services().settings();
        self.updating.set(true);
        let semitones = if settings.dsp.pitch.enabled {
            settings.dsp.pitch.semitones.round()
        } else {
            0.0
        };
        self.shift.set_value(semitones);
        self.shift_text.set_text(&semitones_text(semitones));
        self.keep.set_active(settings.dsp.speed.is_active());
        self.updating.set(false);
        self.explain_keep();
    }

    fn explain(&self) {
        self.explanation.set_text(&if self.keep_pitch() {
            tr(
                "The music is faster but sounds as high as before. “Follows the speed” is like a tape: faster is higher.",
            )
        } else {
            tr("Like a tape: faster is higher, slower is lower. “Stays” keeps the pitch.")
        });
    }

    fn explain_keep(&self) {
        self.keep_note.set_text(&if self.keep.is_active() {
            tr("On: Helikodu starts with this speed.")
        } else {
            tr("Off: the speed is normal again when Helikodu closes.")
        });
    }

    /// Follows the player: the speed, the pitch's choice, whether the speed
    /// applies, and the listening time left.
    pub(crate) fn show(&self, view: &PlayerView) {
        let speed = view.speed;
        self.updating.set(true);
        self.shown.set_text(&speed_text(speed.rate));
        if (self.scale.value() - speed.rate).abs() > STEP / 4.0 {
            self.scale.set_value(speed.rate);
        }
        if let Some(button) = self.pitch.get(usize::from(!speed.keep_pitch)) {
            button.set_active(true);
        }
        self.updating.set(false);
        self.explain();
        for (button, rate) in self.choices.iter().zip(CHOICES) {
            if (rate - speed.rate).abs() < STEP / 4.0 {
                button.add_css_class("chosen");
            } else {
                button.remove_css_class("chosen");
            }
        }
        self.not_applied.set_visible(view.speed_not_applied());
        self.show_time(view);
    }

    /// The listening time until the track ends.
    pub(crate) fn show_time(&self, view: &PlayerView) {
        let text = view
            .now
            .as_ref()
            .and(view.listening_left())
            .map_or_else(|| "–:––".to_owned(), crate::format::duration);
        self.ends.set_text(&text);
    }

    /// Opens the popover pointing at `area` of its parent.
    pub(crate) fn open(&self, area: Option<gtk::gdk::Rectangle>) {
        self.popover.set_pointing_to(area.as_ref());
        self.popover.popup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_read_without_needless_zeros() {
        assert_eq!(speed_text(1.25), "1.25×");
        assert_eq!(speed_text(2.0), "2×");
        assert_eq!(speed_text(0.75), "0.75×");
        assert_eq!(speed_text(1.5), "1.5×");
        assert_eq!(speed_text(1.0), "1×");
        assert_eq!(semitones_text(0.0), "0 semitones");
        assert_eq!(semitones_text(3.0), "+3 semitones");
        assert_eq!(semitones_text(-1.0), "−1 semitone");
    }
}
