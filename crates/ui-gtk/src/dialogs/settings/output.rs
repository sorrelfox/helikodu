// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Audio output: the backend, the device (updated on hot-plug when
//! the page is shown again), shared or exclusive use, and the format, volume,
//! buffer and warm-up settings, the resampler quality, how DSD files reach
//! the device (converted to PCM, or DoP where the device and the path allow),
//! and when an exclusive device is released.
//!
//! A change that would play at a fixed full volume on a device the user has
//! not confirmed asks first (the full-output confirmation).

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::audio::SampleRate;
use helikodu_model::settings::{
    DeviceLostPolicy, DsdOutput, OutputFormat, ResamplerQuality, SampleRatePolicy, VolumeControl,
};

use super::{Context, page_body};
use crate::backend::services::{
    BackendSelection, DeviceInfo, DeviceSelection, OutputBackendKind, OutputMode, Settings,
};
use crate::dialogs::common::{
    bind_segmented, divider, dropdown, note, on_selected, option_card, row, segmented, switch_row,
};
use crate::dialogs::volume_confirm;
use crate::i18n::{fill, tr};
use crate::popovers::output_switcher::{backend_name, rate_range};
use crate::widgets::label;

const BACKENDS: [Option<OutputBackendKind>; 5] = [
    None,
    Some(OutputBackendKind::PipeWire),
    Some(OutputBackendKind::PulseAudio),
    Some(OutputBackendKind::Alsa),
    Some(OutputBackendKind::Jack),
];

const RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// Applies `change`, asking first when it would move playing output to an
/// unconfirmed fixed full volume. `revert` puts the control back when the
/// user says no or the store refuses.
fn guarded(
    context: &Rc<Context>,
    device: &str,
    change: impl Fn(&mut Settings) + 'static,
    revert: impl Fn() + 'static,
) {
    let mut after = (*context.settings()).clone();
    change(&mut after);
    let target = after.output.selection();
    let revert = Rc::new(revert);
    let weak = Rc::downgrade(context);
    let undo = revert.clone();
    volume_confirm::guard_change(
        Some(&context.window),
        &context.backend,
        context.playback.as_ref(),
        volume_confirm::Proposal {
            target: &target,
            device,
            after: &after,
        },
        move || {
            if let Some(context) = weak.upgrade()
                && !context.update(|s| change(s))
            {
                undo();
            }
        },
        move || revert(),
    );
}

/// Builds the pages again once the current signal is handled.
fn later_rebuild(context: &Rc<Context>) {
    let weak = Rc::downgrade(context);
    glib::idle_add_local_once(move || {
        if let Some(context) = weak.upgrade() {
            context.rebuild();
        }
    });
}

fn backend_control(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let built = context.services.output_backends();
    let labels: Vec<String> = BACKENDS
        .iter()
        .map(|b| b.map_or_else(|| tr("Automatic"), backend_name))
        .collect();
    let (group, buttons) = segmented(&labels);
    for (button, backend) in buttons.iter().zip(BACKENDS) {
        if let Some(kind) = backend
            && !built.contains(&kind)
        {
            button.set_sensitive(false);
            button.set_tooltip_text(Some(&tr("Not in this build")));
        }
    }
    let index = match settings.output.backend {
        BackendSelection::Automatic => 0,
        BackendSelection::Backend(kind) => {
            BACKENDS.iter().position(|b| *b == Some(kind)).unwrap_or(0)
        }
    };
    let weak = Rc::downgrade(context);
    bind_segmented(&buttons, index, move |i| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let backend = match BACKENDS[i] {
            None => BackendSelection::Automatic,
            Some(kind) => BackendSelection::Backend(kind),
        };
        if context.settings().output.backend == backend {
            return;
        }
        let weak = Rc::downgrade(&context);
        guarded(
            &context,
            &tr("System default"),
            move |s| {
                s.output.backend = backend;
                s.output.device = DeviceSelection::SystemDefault;
            },
            move || {
                if let Some(context) = weak.upgrade() {
                    context.rebuild();
                }
            },
        );
        later_rebuild(&context);
    });
    group
}

fn device_row(
    context: &Rc<Context>,
    device: Option<&DeviceInfo>,
    chosen: bool,
    group: &gtk::CheckButton,
) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.add_css_class("hk-device-choice");
    if chosen {
        line.add_css_class("selected");
    }
    let radio = gtk::CheckButton::new();
    radio.set_group(Some(group));
    radio.set_active(chosen);
    line.append(&radio);
    let name = device.map_or_else(|| tr("System default"), |d| d.description.clone());
    let title = label(&name, if chosen { &["hk-strong"][..] } else { &[] });
    line.append(&title);
    let caption = match device {
        None => tr("follows the desktop"),
        Some(d) => rate_range(d).unwrap_or_default(),
    };
    let caption = label(&caption, &["hk-t3", "hk-secondary"]);
    caption.set_hexpand(true);
    line.append(&caption);
    let id = device.map_or("default", |d| d.id.as_str());
    line.append(&label(id, &["hk-mono-meta", "hk-t3"]));
    radio.update_property(&[gtk::accessible::Property::Label(&name)]);
    let selection = device.map_or(DeviceSelection::SystemDefault, |d| {
        DeviceSelection::Device(d.id.clone())
    });
    let weak = Rc::downgrade(context);
    radio.connect_toggled(move |radio| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        if !radio.is_active() || context.settings().output.device == selection {
            return;
        }
        let device = selection.clone();
        let weak = Rc::downgrade(&context);
        guarded(
            &context,
            &name,
            move |s| s.output.device = device.clone(),
            move || {
                if let Some(context) = weak.upgrade() {
                    context.rebuild();
                }
            },
        );
        later_rebuild(&context);
    });
    let click = gtk::GestureClick::new();
    let weak = radio.downgrade();
    click.connect_released(move |_, _, _, _| {
        if let Some(radio) = weak.upgrade() {
            radio.set_active(true);
        }
    });
    line.add_controller(click);
    line
}

fn device_list(context: &Rc<Context>) -> gtk::Box {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("hk-bordered-list");
    let settings = context.settings();
    let backend = context.services.resolve_backend(settings.output.backend);
    let group = gtk::CheckButton::new();
    let chosen_default = settings.output.device == DeviceSelection::SystemDefault;
    list.append(&device_row(context, None, chosen_default, &group));
    let services = context.services.clone();
    let weak = Rc::downgrade(context);
    let list_weak = list.downgrade();
    glib::spawn_future_local(async move {
        let devices = gio::spawn_blocking(move || services.devices(backend))
            .await
            .unwrap_or_else(|_| Ok(Vec::new()));
        let (Some(context), Some(list)) = (weak.upgrade(), list_weak.upgrade()) else {
            return;
        };
        let current = context.settings().output.device.clone();
        match devices {
            Ok(devices) => {
                let mut found = current == DeviceSelection::SystemDefault;
                for device in &devices {
                    let chosen = current == DeviceSelection::Device(device.id.clone());
                    found |= chosen;
                    list.append(&device_row(&context, Some(device), chosen, &group));
                }
                if !found && let DeviceSelection::Device(id) = &current {
                    let missing = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                    missing.add_css_class("hk-device-choice");
                    missing.add_css_class("selected");
                    missing.append(&label(id.as_str(), &["hk-t2"]));
                    let state = label(&tr("Not connected"), &["hk-t3", "hk-secondary"]);
                    state.set_hexpand(true);
                    missing.append(&state);
                    list.append(&missing);
                }
            }
            Err(error) => {
                let problem = label(
                    &fill(&tr("The devices cannot be listed: {}"), &[&error.0]),
                    &["hk-t3", "hk-device-choice"],
                );
                list.append(&problem);
            }
        }
    });
    list
}

fn mode(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.set_homogeneous(true);
    let (shared, shared_check) = option_card(
        &tr("Shared"),
        &tr("Other apps can play at the same time."),
        None,
    );
    let (exclusive, exclusive_check) = option_card(
        &tr("Exclusive, bit-perfect"),
        &tr("Helikodu alone uses the device. Samples reach the DAC untouched."),
        Some(&shared_check),
    );
    if context.settings().output.mode == OutputMode::Exclusive {
        exclusive_check.set_active(true);
    } else {
        shared_check.set_active(true);
    }
    for (check, mode) in [
        (&shared_check, OutputMode::Shared),
        (&exclusive_check, OutputMode::Exclusive),
    ] {
        let weak = Rc::downgrade(context);
        check.connect_toggled(move |check| {
            if let Some(context) = weak.upgrade()
                && check.is_active()
                && context.settings().output.mode != mode
            {
                context.update(|s| s.output.mode = mode);
            }
        });
    }
    line.append(&shared);
    line.append(&exclusive);
    line
}

/// A labelled control in the grid.
fn cell(title: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let cell = gtk::Box::new(gtk::Orientation::Vertical, 8);
    cell.set_hexpand(true);
    let name = label(title, &["hk-row-label"]);
    cell.append(&name);
    let control = control.as_ref();
    control.set_halign(gtk::Align::Fill);
    cell.append(control);
    control.update_property(&[gtk::accessible::Property::Label(title)]);
    cell
}

fn rate_text(policy: &SampleRatePolicy) -> String {
    match policy {
        SampleRatePolicy::MatchTrack => tr("Match the track when possible"),
        SampleRatePolicy::DeviceDefault => tr("The device's default rate"),
        SampleRatePolicy::Fixed(rate) => {
            fill(&tr("Always {} kHz"), &[&crate::format::khz(rate.hz())])
        }
    }
}

fn format_text(format: OutputFormat) -> String {
    match format {
        OutputFormat::Auto => tr("Automatic"),
        OutputFormat::S16 => tr("16-bit"),
        OutputFormat::S24 => tr("24-bit in 32-bit words"),
        OutputFormat::S24Packed => tr("24-bit packed"),
        OutputFormat::S32 => tr("32-bit"),
        OutputFormat::F32 => tr("32-bit float"),
    }
}

fn volume_text(control: &VolumeControl) -> String {
    match control {
        VolumeControl::System => tr("System volume"),
        VolumeControl::Software => tr("Software volume in Helikodu"),
        VolumeControl::Hardware(element) => fill(&tr("Hardware mixer ({})"), &[element]),
        VolumeControl::Fixed => tr("Fixed at 100 % (bit-perfect)"),
    }
}

fn lost_text(policy: DeviceLostPolicy) -> String {
    match policy {
        DeviceLostPolicy::Pause => tr("Pause"),
        DeviceLostPolicy::FallbackDefault => tr("Continue on the default device"),
        DeviceLostPolicy::Stop => tr("Stop"),
    }
}

fn grid(context: &Rc<Context>) -> gtk::Grid {
    let settings = context.settings();
    let output = &settings.output;
    let grid = gtk::Grid::new();
    grid.set_row_spacing(16);
    grid.set_column_spacing(24);
    grid.set_column_homogeneous(true);

    // Sample rate.
    let mut rates: Vec<SampleRatePolicy> = vec![
        SampleRatePolicy::MatchTrack,
        SampleRatePolicy::DeviceDefault,
    ];
    rates.extend(
        RATES
            .iter()
            .filter_map(|&hz| SampleRate::new(hz))
            .map(SampleRatePolicy::Fixed),
    );
    if !rates.contains(&output.sample_rate) {
        rates.push(output.sample_rate);
    }
    let items: Vec<String> = rates.iter().map(rate_text).collect();
    let index = rates
        .iter()
        .position(|r| *r == output.sample_rate)
        .unwrap_or(0);
    let rate = dropdown(&items, index, -1);
    let weak = Rc::downgrade(context);
    on_selected(&rate, move |i| {
        if let (Some(context), Some(&policy)) = (weak.upgrade(), rates.get(i)) {
            context.update(|s| s.output.sample_rate = policy);
        }
    });
    let rate_cell = cell(&tr("Sample rate"), &rate);
    if context.services.resolve_backend(output.backend) == OutputBackendKind::PipeWire
        && output.sample_rate == SampleRatePolicy::MatchTrack
    {
        rate_cell.append(&note(&tr(
            "PipeWire switches only to the rates allowed for the device in its configuration.",
        )));
    }
    grid.attach(&rate_cell, 0, 0, 1, 1);

    // Sample format.
    let formats = [
        OutputFormat::Auto,
        OutputFormat::S16,
        OutputFormat::S24,
        OutputFormat::S24Packed,
        OutputFormat::S32,
        OutputFormat::F32,
    ];
    let items: Vec<String> = formats.iter().map(|&f| format_text(f)).collect();
    let index = formats
        .iter()
        .position(|f| *f == output.format)
        .unwrap_or(0);
    let format = dropdown(&items, index, -1);
    let weak = Rc::downgrade(context);
    on_selected(&format, move |i| {
        if let (Some(context), Some(&format)) = (weak.upgrade(), formats.get(i)) {
            context.update(|s| s.output.format = format);
        }
    });
    grid.attach(&cell(&tr("Sample format"), &format), 1, 0, 1, 1);

    // Resampler quality.
    let mut qualities = vec![
        ResamplerQuality::High,
        ResamplerQuality::Medium,
        ResamplerQuality::Fast,
    ];
    let stored = context.settings().dsp.resampler;
    if !qualities.contains(&stored) {
        qualities.push(stored);
    }
    let items: Vec<String> = qualities
        .iter()
        .map(|q| match q {
            ResamplerQuality::Fast => tr("Fast"),
            ResamplerQuality::Medium => tr("Medium"),
            ResamplerQuality::High => tr("High"),
            ResamplerQuality::Maximum => tr("Maximum"),
        })
        .collect();
    let index = qualities.iter().position(|q| *q == stored).unwrap_or(0);
    let resampler = dropdown(&items, index, -1);
    resampler.set_widget_name("resampler");
    let weak = Rc::downgrade(context);
    on_selected(&resampler, move |i| {
        if let (Some(context), Some(&quality)) = (weak.upgrade(), qualities.get(i)) {
            context.update(|s| s.dsp.resampler = quality);
        }
    });
    grid.attach(&cell(&tr("Resampler quality"), &resampler), 2, 0, 1, 1);

    // Volume control.
    let mut controls = vec![
        VolumeControl::System,
        VolumeControl::Software,
        VolumeControl::Fixed,
    ];
    if !controls.contains(&output.volume) {
        controls.push(output.volume.clone());
    }
    let items: Vec<String> = controls.iter().map(volume_text).collect();
    let index = controls
        .iter()
        .position(|c| *c == output.volume)
        .unwrap_or(0);
    let volume = dropdown(&items, index, -1);
    volume.set_widget_name("volume-control");
    let weak = Rc::downgrade(context);
    let reverting = Rc::new(Cell::new(false));
    let previous = Rc::new(Cell::new(index));
    let weak_volume = volume.downgrade();
    on_selected(&volume, move |i| {
        if reverting.get() {
            return;
        }
        let (Some(context), Some(control)) = (weak.upgrade(), controls.get(i).cloned()) else {
            return;
        };
        let device = context
            .playback
            .as_ref()
            .map(|p| p.view().output.device)
            .unwrap_or_default();
        let back = previous.get();
        previous.set(i);
        let reverting = reverting.clone();
        let previous = previous.clone();
        let weak_volume = weak_volume.clone();
        guarded(
            &context,
            &device,
            move |s| s.output.volume = control.clone(),
            move || {
                if let Some(volume) = weak_volume.upgrade() {
                    reverting.set(true);
                    volume.set_selected(u32::try_from(back).unwrap_or(0));
                    previous.set(back);
                    reverting.set(false);
                }
            },
        );
    });
    grid.attach(&cell(&tr("Volume control"), &volume), 0, 1, 1, 1);

    // DSD files.
    let dsd = dropdown(
        &[tr("Convert to PCM"), tr("DoP when the device can")],
        usize::from(output.dsd == DsdOutput::Dop),
        -1,
    );
    dsd.set_widget_name("dsd-output");
    let weak = Rc::downgrade(context);
    on_selected(&dsd, move |i| {
        if let Some(context) = weak.upgrade() {
            context.update(|s| {
                s.output.dsd = if i == 1 {
                    DsdOutput::Dop
                } else {
                    DsdOutput::Pcm
                };
            });
        }
    });
    grid.attach(&cell(&tr("DSD files"), &dsd), 1, 1, 1, 1);

    // If the device disappears.
    let policies = [
        DeviceLostPolicy::Pause,
        DeviceLostPolicy::FallbackDefault,
        DeviceLostPolicy::Stop,
    ];
    let items: Vec<String> = policies.iter().map(|&p| lost_text(p)).collect();
    let index = policies
        .iter()
        .position(|p| *p == output.on_device_lost)
        .unwrap_or(0);
    let lost = dropdown(&items, index, -1);
    let weak = Rc::downgrade(context);
    on_selected(&lost, move |i| {
        if let (Some(context), Some(&policy)) = (weak.upgrade(), policies.get(i)) {
            context.update(|s| s.output.on_device_lost = policy);
        }
    });
    grid.attach(&cell(&tr("If the device disappears"), &lost), 2, 1, 1, 1);

    // Buffer.
    let buffer_cell = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = label(&tr("Buffer"), &["hk-row-label"]);
    title.set_hexpand(true);
    head.append(&title);
    let value = label(
        &fill(&tr("{} ms"), &[&output.buffer_ms.to_string()]),
        &["hk-mono", "hk-t2", "hk-secondary"],
    );
    head.append(&value);
    buffer_cell.append(&head);
    let buffer = gtk::Scale::with_range(gtk::Orientation::Horizontal, 50.0, 2000.0, 10.0);
    buffer.add_css_class("hk-slider");
    buffer.set_value(f64::from(output.buffer_ms));
    buffer.update_property(&[gtk::accessible::Property::Label(&tr("Buffer"))]);
    let weak = Rc::downgrade(context);
    let pending = Rc::new(Cell::new(None::<glib::SourceId>));
    buffer.connect_value_changed(move |scale| {
        let ms = (scale.value() / 10.0).round() as u32 * 10;
        value.set_text(&fill(&tr("{} ms"), &[&ms.to_string()]));
        // Apply once the slider rests, not on every step of a drag.
        if let Some(source) = pending.take() {
            source.remove();
        }
        let weak = weak.clone();
        let done = pending.clone();
        pending.set(Some(glib::timeout_add_local_once(
            std::time::Duration::from_millis(250),
            move || {
                done.take();
                if let Some(context) = weak.upgrade() {
                    context.update(|s| s.output.buffer_ms = ms);
                }
            },
        )));
    });
    buffer_cell.append(&buffer);
    grid.attach(&buffer_cell, 0, 2, 1, 1);

    // DAC warm-up.
    let warm = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let preroll = gtk::Entry::new();
    preroll.add_css_class("hk-input");
    preroll.add_css_class("hk-numeric");
    preroll.set_input_purpose(gtk::InputPurpose::Digits);
    preroll.set_text(&output.preroll_ms.to_string());
    preroll.set_width_chars(5);
    preroll.set_max_width_chars(5);
    gtk::prelude::EditableExt::set_alignment(&preroll, 1.0);
    preroll.set_hexpand(false);
    preroll.update_property(&[gtk::accessible::Property::Label(&tr("DAC warm-up"))]);
    let save = {
        let weak = Rc::downgrade(context);
        move |entry: &gtk::Entry| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            let stored = context.settings().output.preroll_ms;
            match entry.text().trim().parse::<u32>() {
                Ok(ms) if ms != stored => {
                    if !context.update(|s| s.output.preroll_ms = ms) {
                        entry.set_text(&stored.to_string());
                    }
                }
                Ok(_) => {}
                Err(_) => entry.set_text(&stored.to_string()),
            }
        }
    };
    let on_activate = save.clone();
    preroll.connect_activate(move |entry| on_activate(entry));
    let focus = gtk::EventControllerFocus::new();
    let weak = preroll.downgrade();
    focus.connect_leave(move |_| {
        if let Some(entry) = weak.upgrade() {
            save(&entry);
        }
    });
    preroll.add_controller(focus);
    warm.append(&preroll);
    warm.append(&label(&tr("ms of silence"), &["hk-t3"]));
    let warm_cell = cell(&tr("DAC warm-up"), &warm);
    preroll.set_halign(gtk::Align::Start);
    grid.attach(&warm_cell, 1, 2, 1, 1);

    // Release of an exclusive device: after a pause of so many seconds, or
    // only on stop (0).
    let mut pauses: Vec<u32> = vec![30, 0];
    if !pauses.contains(&output.exclusive_release_s) {
        pauses.insert(0, output.exclusive_release_s);
    }
    let items: Vec<String> = pauses
        .iter()
        .map(|&s| {
            if s == 0 {
                tr("On stop only")
            } else {
                fill(&tr("On stop and after a {} s pause"), &[&s.to_string()])
            }
        })
        .collect();
    let index = pauses
        .iter()
        .position(|s| *s == output.exclusive_release_s)
        .unwrap_or(0);
    let release = dropdown(&items, index, -1);
    release.set_widget_name("exclusive-release");
    let weak = Rc::downgrade(context);
    on_selected(&release, move |i| {
        if let (Some(context), Some(&seconds)) = (weak.upgrade(), pauses.get(i)) {
            context.update(|s| s.output.exclusive_release_s = seconds);
        }
    });
    grid.attach(&cell(&tr("Release the device"), &release), 2, 2, 1, 1);
    grid
}

/// The Audio output page.
pub(super) fn page(context: &Rc<Context>) -> gtk::Widget {
    let (scroller, body) = page_body();
    body.append(&row(&tr("Backend"), None, &backend_control(context)));
    body.append(&row(
        &tr("Device"),
        Some(&tr(
            "Remembered by its stable ID, so it survives reboots and replugging.",
        )),
        &device_list(context),
    ));
    body.append(&row(&tr("Mode"), None, &mode(context)));
    body.append(&divider());
    body.append(&grid(context));
    body.append(&divider());
    body.append(&unplug_switch(context));
    scroller.upcast()
}

/// Pausing when the device that the default output plays to is unplugged.
fn unplug_switch(context: &Rc<Context>) -> gtk::Box {
    let (line, switch) = switch_row(
        &tr("Pause when headphones are unplugged"),
        Some(&tr(
            "Instead of going on through the loudspeakers when the system default output moves to another device.",
        )),
    );
    switch.set_active(context.settings().output.pause_on_unplug);
    let weak = Rc::downgrade(context);
    switch.connect_active_notify(move |switch| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let on = switch.is_active();
        if !context.update(|s| s.output.pause_on_unplug = on) {
            switch.set_active(!on);
        }
    });
    line
}
