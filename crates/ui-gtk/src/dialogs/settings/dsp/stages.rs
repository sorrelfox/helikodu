// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The editors of crossfeed, the channel mixer and the limiter.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use helikodu_model::settings::{
    ChannelMixSettings, ChannelMode, CrossfeedPreset, CrossfeedSettings, LimiterSettings,
};

use super::{Live, info_note, lead};
use crate::dialogs::common::{bind_segmented, divider, row, segmented, switch_row};
use crate::i18n::{fill, tr};
use crate::widgets::{BipolarScale, label};

/// A balance in words: `Centre`, `2.0 dB to the left`.
pub(crate) fn balance_words(db: f64) -> String {
    if db.abs() < 0.05 {
        tr("Centre")
    } else if db < 0.0 {
        fill(&tr("{} dB to the left"), &[&format!("{:.1}", -db)])
    } else {
        fill(&tr("{} dB to the right"), &[&format!("{db:.1}")])
    }
}

/// A balance as the chain's summary gives it: `0`, `L 2.0`, `R 1.5`.
pub(crate) fn balance_short(db: f64) -> String {
    if db.abs() < 0.05 {
        "0".into()
    } else if db < 0.0 {
        fill(&tr("L {}"), &[&format!("{:.1}", -db)])
    } else {
        fill(&tr("R {}"), &[&format!("{db:.1}")])
    }
}

/// A slider with its title and value above and its ends below.
struct Slider {
    root: gtk::Box,
    scale: gtk::Scale,
    value: gtk::Label,
}

fn slider(title: &str, range: (f64, f64, f64), ends: (&str, &str), value: &str) -> Slider {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_hexpand(true);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = label(title, &[]);
    name.set_hexpand(true);
    head.append(&name);
    let shown = label(value, &["hk-mono", "hk-t2", "hk-secondary"]);
    head.append(&shown);
    root.append(&head);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, range.0, range.1, range.2);
    scale.add_css_class("hk-slider");
    scale.set_draw_value(false);
    scale.update_property(&[gtk::accessible::Property::Label(title)]);
    root.append(&scale);
    let marks = gtk::CenterBox::new();
    marks.set_start_widget(Some(&label(ends.0, &["hk-mono-meta", "hk-t3"])));
    marks.set_end_widget(Some(&label(ends.1, &["hk-mono-meta", "hk-t3"])));
    root.append(&marks);
    Slider {
        root,
        scale,
        value: shown,
    }
}

/// An option card with a value at the right of its title.
fn valued_card(
    title: &str,
    value: &str,
    description: &str,
    group: Option<&gtk::CheckButton>,
) -> (gtk::Box, gtk::CheckButton) {
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    card.add_css_class("hk-option-card");
    card.set_hexpand(true);
    let check = gtk::CheckButton::new();
    check.set_valign(gtk::Align::Start);
    if let Some(group) = group {
        check.set_group(Some(group));
    }
    check.update_property(&[
        gtk::accessible::Property::Label(title),
        gtk::accessible::Property::Description(&format!("{value}. {description}")),
    ]);
    card.append(&check);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = label(title, &["hk-strong"]);
    name.set_hexpand(true);
    head.append(&name);
    if !value.is_empty() {
        head.append(&label(value, &["hk-mono-meta", "hk-t3"]));
    }
    text.append(&head);
    let sentence = gtk::Label::new(Some(description));
    sentence.set_xalign(0.0);
    sentence.set_wrap(true);
    sentence.add_css_class("hk-t2");
    sentence.add_css_class("hk-secondary");
    text.append(&sentence);
    card.append(&text);
    let weak = card.downgrade();
    check.connect_toggled(move |check| {
        if let Some(card) = weak.upgrade() {
            if check.is_active() {
                card.add_css_class("selected");
            } else {
                card.remove_css_class("selected");
            }
        }
    });
    let click = gtk::GestureClick::new();
    let weak = check.downgrade();
    click.connect_released(move |_, _, _, _| {
        if let Some(check) = weak.upgrade() {
            check.set_active(true);
        }
    });
    card.add_controller(click);
    (card, check)
}

const PRESETS: [CrossfeedPreset; 4] = [
    CrossfeedPreset::Default,
    CrossfeedPreset::ChuMoy,
    CrossfeedPreset::JanMeier,
    CrossfeedPreset::Custom,
];

fn crossfeed_values(hz: u32, db: f64) -> String {
    fill(
        &tr("{} Hz · {} dB"),
        &[&hz.to_string(), &format!("{db:.1}")],
    )
}

/// Applies `change` to the custom values; moving a slider chooses Custom,
/// starting from the values heard so far.
fn set_custom(live: &Rc<Live>, change: impl Fn(&mut CrossfeedSettings) + 'static) {
    live.apply_soon(move |s| {
        let crossfeed = &mut s.dsp.crossfeed;
        if crossfeed.preset != CrossfeedPreset::Custom {
            let (hz, db) = crossfeed.effective();
            crossfeed.cutoff_hz = hz;
            crossfeed.level_db = db;
            crossfeed.preset = CrossfeedPreset::Custom;
        }
        change(crossfeed);
    });
}

/// The crossfeed editor and what refreshes it.
pub(super) fn crossfeed(live: &Rc<Live>) -> (gtk::Widget, Rc<dyn Fn()>) {
    let settings = live.settings().map(|s| s.dsp.crossfeed).unwrap_or_default();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&lead(&tr(
        "For headphones. A little of each channel is mixed into the other, as happens with loudspeakers in a room. The sound moves from inside the head towards the front.",
    )));

    let grid = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let lines = [
        gtk::Box::new(gtk::Orientation::Horizontal, 12),
        gtk::Box::new(gtk::Orientation::Horizontal, 12),
    ];
    for line in &lines {
        line.set_homogeneous(true);
        grid.append(line);
    }
    let mut checks: Vec<gtk::CheckButton> = Vec::new();
    for (i, preset) in PRESETS.iter().enumerate() {
        let (title, description) = match preset {
            CrossfeedPreset::Default => (tr("Default"), tr("The strongest of the three.")),
            CrossfeedPreset::ChuMoy => (tr("Chu Moy"), tr("In the middle.")),
            CrossfeedPreset::JanMeier => (
                tr("Jan Meier"),
                tr("The mildest. It changes the recording least."),
            ),
            CrossfeedPreset::Custom => (tr("Custom"), tr("Set both values yourself.")),
        };
        let value = preset
            .values()
            .map(|(hz, db)| crossfeed_values(hz, db))
            .unwrap_or_default();
        let (card, check) = valued_card(&title, &value, &description, checks.first());
        check.set_widget_name(&format!("crossfeed-{i}"));
        lines[i / 2].append(&card);
        checks.push(check);
    }
    column.append(&row(&tr("Strength"), None, &grid));

    let custom = gtk::Box::new(gtk::Orientation::Horizontal, 28);
    custom.set_homogeneous(true);
    let (hz, db) = settings.effective();
    let cutoff = slider(
        &tr("Cut-off frequency"),
        (
            f64::from(*CrossfeedSettings::CUTOFF_RANGE.start()),
            f64::from(*CrossfeedSettings::CUTOFF_RANGE.end()),
            10.0,
        ),
        ("300 Hz", "2 kHz"),
        &fill(&tr("{} Hz"), &[&hz.to_string()]),
    );
    cutoff.scale.set_value(f64::from(hz));
    cutoff.scale.set_widget_name("crossfeed-cutoff");
    custom.append(&cutoff.root);
    let level = slider(
        &tr("Level of the mixed-in channel"),
        (
            *CrossfeedSettings::LEVEL_RANGE.start(),
            *CrossfeedSettings::LEVEL_RANGE.end(),
            0.5,
        ),
        ("1 dB", "15 dB"),
        &fill(&tr("{} dB"), &[&format!("{db:.1}")]),
    );
    level.scale.set_value(db);
    level.scale.set_widget_name("crossfeed-level");
    custom.append(&level.root);
    column.append(&row(&tr("Custom values"), None, &custom));
    column.append(&info_note(&tr(
        "Works on stereo tracks. Mono and surround tracks pass through unchanged.",
    )));

    let updating = Rc::new(Cell::new(false));
    let index = PRESETS
        .iter()
        .position(|p| *p == settings.preset)
        .unwrap_or(0);
    checks[index].set_active(true);
    for (i, check) in checks.iter().enumerate() {
        let live = live.clone();
        let updating = updating.clone();
        check.connect_toggled(move |check| {
            if !check.is_active() || updating.get() {
                return;
            }
            let preset = PRESETS[i];
            live.apply(|s| {
                let crossfeed = &mut s.dsp.crossfeed;
                if preset == CrossfeedPreset::Custom {
                    // Start from the values heard so far.
                    let (hz, db) = crossfeed.effective();
                    crossfeed.cutoff_hz = hz;
                    crossfeed.level_db = db;
                }
                crossfeed.preset = preset;
            });
        });
    }
    {
        let live = live.clone();
        let updating = updating.clone();
        let shown = cutoff.value.clone();
        cutoff.scale.connect_value_changed(move |scale| {
            let hz = ((scale.value() / 10.0).round() * 10.0) as u32;
            shown.set_text(&fill(&tr("{} Hz"), &[&hz.to_string()]));
            if !updating.get() {
                set_custom(&live, move |c| c.cutoff_hz = hz);
            }
        });
    }
    {
        let live = live.clone();
        let updating = updating.clone();
        let shown = level.value.clone();
        level.scale.connect_value_changed(move |scale| {
            let db = (scale.value() * 2.0).round() / 2.0;
            shown.set_text(&fill(&tr("{} dB"), &[&format!("{db:.1}")]));
            if !updating.get() {
                set_custom(&live, move |c| c.level_db = db);
            }
        });
    }
    let weak_live = Rc::downgrade(live);
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        let Some(crossfeed) = weak_live
            .upgrade()
            .and_then(|l| l.settings())
            .map(|s| s.dsp.crossfeed)
        else {
            return;
        };
        updating.set(true);
        if let Some(i) = PRESETS.iter().position(|p| *p == crossfeed.preset)
            && !checks[i].is_active()
        {
            checks[i].set_active(true);
        }
        let (hz, db) = crossfeed.effective();
        if (cutoff.scale.value() - f64::from(hz)).abs() > 5.0 {
            cutoff.scale.set_value(f64::from(hz));
        }
        if (level.scale.value() - db).abs() > 0.25 {
            level.scale.set_value(db);
        }
        updating.set(false);
    });
    (column.upcast(), refresh)
}

const CHANNELS: [ChannelMode; 3] = [ChannelMode::Stereo, ChannelMode::Mono, ChannelMode::Swap];

/// The channel mixer's editor and what refreshes it.
pub(super) fn channel_mix(live: &Rc<Live>) -> (gtk::Widget, Rc<dyn Fn()>) {
    let mix: ChannelMixSettings = live
        .settings()
        .map(|s| s.dsp.channel_mix)
        .unwrap_or_default();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&lead(&tr(
        "Changes how the channels reach the device. Mono, swap and balance apply to every track. The surround setting applies to files with more channels than the device has.",
    )));

    let (group, buttons) = segmented(&[tr("Stereo"), tr("Mono"), tr("Swap left and right")]);
    group.update_property(&[gtk::accessible::Property::Label(&tr("Channels"))]);
    let updating = Rc::new(Cell::new(false));
    {
        let live = live.clone();
        let updating = updating.clone();
        bind_segmented(
            &buttons,
            CHANNELS
                .iter()
                .position(|c| *c == mix.channels)
                .unwrap_or(0),
            move |i| {
                if !updating.get() {
                    let channels = CHANNELS[i];
                    live.apply(|s| s.dsp.channel_mix.channels = channels);
                }
            },
        );
    }
    column.append(&row(&tr("Channels"), None, &group));

    let balance = gtk::Box::new(gtk::Orientation::Vertical, 6);
    balance.set_halign(gtk::Align::Start);
    balance.set_size_request(420, -1);
    let words = label(
        &balance_words(mix.balance_db),
        &["hk-mono", "hk-t2", "hk-secondary"],
    );
    words.set_halign(gtk::Align::End);
    balance.append(&words);
    let scale = BipolarScale::default();
    scale.set_widget_name("balance");
    scale.update_property(&[gtk::accessible::Property::Label(&tr("Balance"))]);
    scale.set_value_text(balance_words);
    scale.set_value(mix.balance_db);
    balance.append(&scale);
    let marks = gtk::CenterBox::new();
    marks.set_start_widget(Some(&label(&tr("Left"), &["hk-mono-meta", "hk-t3"])));
    marks.set_center_widget(Some(&label(&tr("Centre"), &["hk-mono-meta", "hk-t3"])));
    marks.set_end_widget(Some(&label(&tr("Right"), &["hk-mono-meta", "hk-t3"])));
    balance.append(&marks);
    {
        let live = live.clone();
        let words = words.clone();
        scale.connect_changed(move |db| {
            words.set_text(&balance_words(db));
            live.apply_soon(move |s| s.dsp.channel_mix.balance_db = db);
        });
    }
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.append(&balance);
    column.append(&row(&tr("Balance"), None, &line));
    column.append(&divider());

    let (surround, lfe) = switch_row(
        &tr("Add the LFE channel to the mix"),
        Some(&tr(
            "Surround is mixed down after ITU-R BS.775. Off: the subwoofer channel is left out.",
        )),
    );
    lfe.set_active(mix.include_lfe);
    lfe.set_widget_name("include-lfe");
    {
        let live = live.clone();
        let updating = updating.clone();
        lfe.connect_active_notify(move |switch| {
            if updating.get() {
                return;
            }
            let on = switch.is_active();
            if !live.apply(|s| s.dsp.channel_mix.include_lfe = on) {
                switch.set_active(!on);
            }
        });
    }
    column.append(&row(&tr("Surround files"), None, &surround));

    let weak_live = Rc::downgrade(live);
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        let Some(mix) = weak_live
            .upgrade()
            .and_then(|l| l.settings())
            .map(|s| s.dsp.channel_mix)
        else {
            return;
        };
        updating.set(true);
        if let Some(i) = CHANNELS.iter().position(|c| *c == mix.channels)
            && !buttons[i].is_active()
        {
            buttons[i].set_active(true);
        }
        if lfe.is_active() != mix.include_lfe {
            lfe.set_active(mix.include_lfe);
        }
        updating.set(false);
    });
    (column.upcast(), refresh)
}

/// One column of the limiter's activity readout.
fn activity_column(title: &str) -> (gtk::Box, gtk::Label) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.set_hexpand(true);
    column.append(&label(title, &["hk-micro-label", "hk-readout-label"]));
    let value = label("—", &["hk-mono", "hk-strong", "hk-secondary"]);
    column.append(&value);
    (column, value)
}

/// The limiter's editor. The activity readout follows the player while the
/// page is shown.
pub(super) fn limiter(live: &Rc<Live>) -> gtk::Widget {
    let limiter: LimiterSettings = live.settings().map(|s| s.dsp.limiter).unwrap_or_default();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&lead(&tr(
        "A safety net against clipping. When a peak would go above full scale, the limiter turns the level down for a moment. The rest of the time it leaves the sound alone.",
    )));

    let ms = |v: f64| fill(&tr("{} ms"), &[&format!("{v:.1}")]);
    let block = gtk::Box::new(gtk::Orientation::Vertical, 8);
    block.set_halign(gtk::Align::Start);
    block.set_size_request(420, -1);
    let value = label(
        &ms(limiter.lookahead_ms),
        &["hk-mono", "hk-t2", "hk-secondary"],
    );
    value.set_halign(gtk::Align::End);
    block.append(&value);
    let scale = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        *LimiterSettings::LOOKAHEAD_RANGE.start(),
        *LimiterSettings::LOOKAHEAD_RANGE.end(),
        0.5,
    );
    scale.add_css_class("hk-slider");
    scale.set_draw_value(false);
    scale.set_value(limiter.lookahead_ms);
    scale.set_widget_name("lookahead");
    scale.update_property(&[gtk::accessible::Property::Label(&tr("Lookahead"))]);
    block.append(&scale);
    let marks = gtk::CenterBox::new();
    marks.set_start_widget(Some(&label(
        &fill(&tr("{} ms"), &["1"]),
        &["hk-mono-meta", "hk-t3"],
    )));
    marks.set_end_widget(Some(&label(
        &fill(&tr("{} ms"), &["5"]),
        &["hk-mono-meta", "hk-t3"],
    )));
    block.append(&marks);
    let explanation = crate::dialogs::common::note(&tr(
        "Longer is gentler. The sound is delayed by the same time.",
    ));
    block.append(&explanation);
    {
        let live = live.clone();
        scale.connect_value_changed(move |scale| {
            let v = (scale.value() * 2.0).round() / 2.0;
            value.set_text(&ms(v));
            live.apply_soon(move |s| s.dsp.limiter.lookahead_ms = v);
        });
    }
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.append(&block);
    column.append(&row(&tr("Lookahead"), None, &line));

    // Activity.
    let readout = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    readout.add_css_class("hk-readout");
    readout.add_css_class("hk-dsp-readout");
    let (now, now_value) = activity_column(&tr("Gain reduction now"));
    let meter_line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let meter = gtk::LevelBar::for_interval(0.0, 6.0);
    meter.add_css_class("hk-meter");
    meter.set_hexpand(true);
    meter.set_valign(gtk::Align::Center);
    meter.update_property(&[gtk::accessible::Property::Label(&tr("Gain reduction now"))]);
    now.remove(&now_value);
    meter_line.append(&meter);
    meter_line.append(&now_value);
    now.append(&meter_line);
    readout.append(&now);
    let (largest, largest_value) = activity_column(&tr("Largest in this track"));
    readout.append(&largest);
    let (times, times_value) = activity_column(&tr("Times limited"));
    readout.append(&times);
    now.set_size_request(260, -1);
    column.append(&row(&tr("Activity"), None, &readout));
    column.append(&info_note(&tr(
        "The automatic preamp of the equalizer already prevents most clipping, so the limiter rarely has to act.",
    )));

    // Read the activity a few times a second while the editor is shown.
    let timer: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
    let weak_live = Rc::downgrade(live);
    let poll = move || {
        let reading = weak_live
            .upgrade()
            .and_then(|l| l.context())
            .and_then(|c| c.services.limiter_activity());
        match reading {
            Some(r) => {
                meter.set_value(r.reduction_db.clamp(0.0, 6.0));
                now_value.set_text(&fill(&tr("{} dB"), &[&format!("{:.1}", r.reduction_db)]));
                largest_value.set_text(&fill(
                    &tr("{} dB"),
                    &[&format!("−{:.1}", r.largest_db.abs())],
                ));
                times_value.set_text(&r.times.to_string());
            }
            None => {
                meter.set_value(0.0);
                for value in [&now_value, &largest_value, &times_value] {
                    value.set_text("—");
                }
            }
        }
    };
    {
        let timer = timer.clone();
        column.connect_map(move |_| {
            poll();
            let poll = poll.clone();
            if let Some(old) = timer.replace(Some(glib::timeout_add_local(
                Duration::from_millis(250),
                move || {
                    poll();
                    glib::ControlFlow::Continue
                },
            ))) {
                old.remove();
            }
        });
    }
    column.connect_unmap(move |_| {
        if let Some(id) = timer.take() {
            id.remove();
        }
    });
    column.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_reads_in_words() {
        assert_eq!(balance_words(0.0), "Centre");
        assert_eq!(balance_words(-2.0), "2.0 dB to the left");
        assert_eq!(balance_words(1.5), "1.5 dB to the right");
        assert_eq!(balance_short(0.0), "0");
    }
}
