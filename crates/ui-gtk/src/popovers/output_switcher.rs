// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The output quick switcher: a popover above the deck's output button with
//! the devices of each backend, what each can take (rates, sample formats,
//! whether it can be held exclusively), a check mark at the one playing, the
//! mode, the signal path right now, and a way to the Audio output settings.
//!
//! Choosing a device moves playback there at the same position. The device
//! lists are read on a worker thread each time the popover opens; a
//! remembered device that is not connected stays listed and cannot be chosen.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::services::{
    BackendSelection, DeviceInfo, DeviceSelection, OutputBackendKind, OutputMode, OutputSelection,
};
use crate::backend::{Backend, SignalPath, SignalState};
use crate::dialogs::common::{bind_segmented, segmented};
use crate::dialogs::volume_confirm;
use crate::i18n::{fill, ntr, tr};
use crate::playback::Playback;
use crate::widgets::signal_path_list::{device_format, device_name, source_text, state_label};
use crate::widgets::{SignalLamp, label};

/// The backend's name as people know it.
pub(crate) fn backend_name(kind: OutputBackendKind) -> String {
    match kind {
        OutputBackendKind::PipeWire => "PipeWire".into(),
        OutputBackendKind::PulseAudio => "PulseAudio".into(),
        OutputBackendKind::Alsa => "ALSA".into(),
        OutputBackendKind::Jack => "JACK".into(),
        OutputBackendKind::Null => tr("No sound"),
        OutputBackendKind::Wav => tr("WAV file"),
    }
}

/// A symbolic icon for a device, from what its name says.
pub(crate) fn device_icon(device: Option<&DeviceInfo>) -> &'static str {
    let Some(device) = device else {
        return "preferences-system-symbolic";
    };
    let text = format!("{} {}", device.id.as_str(), device.description).to_lowercase();
    if text.contains("hdmi") || text.contains("displayport") {
        "video-display-symbolic"
    } else if text.contains("headphone") || text.contains("bluez") || text.contains("bluetooth") {
        "audio-headphones-symbolic"
    } else if text.contains("usb") {
        "audio-card-symbolic"
    } else {
        "audio-speakers-symbolic"
    }
}

/// The kind of connection a device's name suggests, in capitals, if any.
fn device_kind(device: &DeviceInfo) -> Option<String> {
    let text = format!("{} {}", device.id.as_str(), device.description).to_lowercase();
    if text.contains("usb") {
        Some("USB".into())
    } else if text.contains("hdmi") || text.contains("displayport") {
        Some("HDMI".into())
    } else if text.contains("analog") || text.contains("pch") || text.contains("built-in") {
        Some(tr("Analogue stereo"))
    } else {
        None
    }
}

/// The highest rate a device takes, as `UP TO 192 kHz`.
pub(crate) fn rate_range(device: &DeviceInfo) -> Option<String> {
    let rates = &device.capabilities.sample_rates;
    let (low, high) = (rates.first()?.hz(), rates.last()?.hz());
    let khz = |hz: u32| crate::format::khz(hz);
    Some(if low == high {
        fill(&tr("{} kHz"), &[&khz(high)])
    } else {
        fill(&tr("up to {} kHz"), &[&khz(high)])
    })
}

/// Whether the device can be held exclusively with nothing between the
/// player and the hardware: an ALSA hardware device, or any PipeWire node
/// (the engine asks PipeWire for exclusive use).
pub(crate) fn supports_exclusive(device: &DeviceInfo) -> bool {
    match device.backend {
        OutputBackendKind::Alsa => device.id.as_str().starts_with("hw:"),
        OutputBackendKind::PipeWire => true,
        _ => false,
    }
}

/// The most bits a device takes, as `32-bit`, if the backend can tell.
fn most_bits(device: &DeviceInfo) -> Option<String> {
    let formats = &device.capabilities.formats;
    let integer = formats
        .iter()
        .filter(|f| f.is_integer())
        .map(|f| f.bits())
        .max();
    match integer {
        Some(bits) => Some(fill(&tr("{}-bit"), &[&bits.to_string()])),
        None if formats.iter().any(|f| f.is_float()) => Some(tr("float")),
        None => None,
    }
}

/// The short capability caption of a device that does not play: the
/// highest rate, the most bits, and whether it can be held exclusively.
fn capability_caption(device: &DeviceInfo) -> Vec<String> {
    let mut caption = Vec::new();
    match (rate_range(device), most_bits(device)) {
        (Some(rate), Some(bits)) => caption.push(fill(&tr("{} / {}"), &[&rate, &bits])),
        (Some(rate), None) => caption.push(rate),
        (None, Some(bits)) => caption.push(bits),
        (None, None) => {}
    }
    if supports_exclusive(device) {
        caption.push(tr("Exclusive"));
    }
    caption
}

/// Everything the backend says a device takes, for its tooltip and its
/// accessible description.
pub(crate) fn capability_text(device: &DeviceInfo) -> String {
    let caps = &device.capabilities;
    let unknown = tr("not reported");
    let rates = if caps.sample_rates.is_empty() {
        unknown.clone()
    } else {
        let list: Vec<String> = caps
            .sample_rates
            .iter()
            .map(|r| crate::format::khz(r.hz()))
            .collect();
        fill(&tr("{} kHz"), &[&list.join(", ")])
    };
    let formats = if caps.formats.is_empty() {
        unknown.clone()
    } else {
        caps.formats
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let channels = if caps.channel_counts.is_empty() {
        unknown
    } else {
        caps.channel_counts
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let exclusive = if supports_exclusive(device) {
        tr("Exclusive use: yes, bit-perfect")
    } else {
        tr("Exclusive use: no")
    };
    [
        fill(&tr("Sample rates: {}"), &[&rates]),
        fill(&tr("Sample formats: {}"), &[&formats]),
        fill(&tr("Channels: {}"), &[&channels]),
        exclusive,
    ]
    .join("\n")
}

/// The selection that plays to `device`, keeping `mode`.
fn selection_of(
    device: Option<&DeviceInfo>,
    backend: OutputBackendKind,
    mode: OutputMode,
) -> OutputSelection {
    OutputSelection {
        backend: BackendSelection::Backend(device.map_or(backend, |d| d.backend)),
        device: device.map_or(DeviceSelection::SystemDefault, |d| {
            DeviceSelection::Device(d.id.clone())
        }),
        mode,
    }
}

fn is_current(
    current: &OutputSelection,
    resolved: OutputBackendKind,
    device: Option<&DeviceInfo>,
) -> bool {
    match (device, &current.device) {
        (None, DeviceSelection::SystemDefault) => true,
        (Some(d), DeviceSelection::Device(id)) => d.backend == resolved && d.id == *id,
        _ => false,
    }
}

struct Switcher {
    popover: gtk::Popover,
    rows: gtk::Box,
    count: gtk::Label,
    badge: gtk::Label,
    readout: Readout,
    backend: Backend,
    playback: std::rc::Weak<Playback>,
    window: glib::WeakRef<gtk::Window>,
}

impl Switcher {
    fn reload(self: &Rc<Self>) {
        let services = self.backend.services();
        let current = services.output();
        let resolved = services.resolve_backend(current.backend);
        self.badge.set_text(&backend_name(resolved));
        let backends = services.output_backends();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let lists = gtk::gio::spawn_blocking(move || {
                backends
                    .into_iter()
                    .map(|b| (b, services.devices(b).unwrap_or_default()))
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            if let Some(switcher) = weak.upgrade() {
                switcher.fill(&current, resolved, &lists);
            }
        });
    }

    fn fill(
        self: &Rc<Self>,
        current: &OutputSelection,
        resolved: OutputBackendKind,
        lists: &[(OutputBackendKind, Vec<DeviceInfo>)],
    ) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        let view = self
            .playback
            .upgrade()
            .map(|p| p.view())
            .unwrap_or_default();
        self.readout.show(view.signal.as_deref(), &view.output);
        let output = view.output;
        let several = lists.iter().filter(|(_, d)| !d.is_empty()).count() > 1;
        let mut total = 1;
        self.add_row(None, current, resolved, &output);
        let mut found = matches!(current.device, DeviceSelection::SystemDefault);
        for (backend, devices) in lists {
            if devices.is_empty() {
                continue;
            }
            if several {
                let section = label(
                    &backend_name(*backend),
                    &["hk-micro-label", "hk-switcher-section"],
                );
                self.rows.append(&section);
            }
            for device in devices {
                found |= is_current(current, resolved, Some(device));
                total += 1;
                self.add_row(Some(device), current, resolved, &output);
            }
        }
        if !found && let DeviceSelection::Device(id) = &current.device {
            // A remembered device that is not connected.
            let row = self.row_widget(
                "audio-speakers-symbolic",
                id.as_str(),
                &tr("Not connected").to_uppercase(),
                false,
                false,
            );
            row.set_sensitive(false);
            row.add_css_class("missing");
            self.rows.append(&row);
            total += 1;
        }
        self.count.set_text(&fill(
            &ntr("{} device", "{} devices", total),
            &[&total.to_string()],
        ));
    }

    fn row_widget(
        &self,
        icon: &str,
        name: &str,
        caption: &str,
        active: bool,
        bit_perfect: bool,
    ) -> gtk::Button {
        let button = gtk::Button::new();
        button.add_css_class("hk-device-row");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
        tile.add_css_class("hk-device-tile");
        tile.set_valign(gtk::Align::Center);
        let image = gtk::Image::from_icon_name(icon);
        image.set_vexpand(true);
        image.set_valign(gtk::Align::Center);
        tile.append(&image);
        line.append(&tile);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let title = label(name, &["hk-device-name"]);
        text.append(&title);
        let note = label(caption, &["hk-mono-meta", "hk-device-caption"]);
        text.append(&note);
        line.append(&text);
        if active {
            button.add_css_class("active");
            let check = gtk::Image::from_icon_name("helikodu-check-symbolic");
            check.add_css_class("hk-accent-text");
            line.append(&check);
            if bit_perfect {
                button.add_css_class("bit-perfect");
            }
        }
        button.set_child(Some(&line));
        let state = if active { tr("current") } else { String::new() };
        button.update_property(&[gtk::accessible::Property::Label(&format!(
            "{name}, {caption} {state}"
        ))]);
        button
    }

    fn add_row(
        self: &Rc<Self>,
        device: Option<&DeviceInfo>,
        current: &OutputSelection,
        resolved: OutputBackendKind,
        output: &crate::backend::OutputInfo,
    ) {
        let active =
            is_current(current, resolved, device) && device.is_none_or(|d| d.backend == resolved);
        let name = device.map_or_else(|| tr("System default"), |d| d.description.clone());
        let mut caption: Vec<String> = Vec::new();
        match device {
            None => caption.push(tr("Follows the desktop")),
            Some(device) => {
                if let Some(kind) = device_kind(device) {
                    caption.push(kind);
                }
                if active {
                    if current.mode == OutputMode::Exclusive {
                        caption.push(tr("Exclusive"));
                    }
                    if let Some(rate) = output.sample_rate {
                        let mut text = fill(&tr("{} kHz"), &[&crate::format::khz(rate)]);
                        if let Some(bits) = output.bits {
                            text = fill(&tr("{} / {}-bit"), &[&text, &bits.to_string()]);
                        }
                        caption.push(text);
                    }
                } else {
                    let capabilities = capability_caption(device);
                    // What the backend reports wins over what the name
                    // suggests when both do not fit on the line.
                    let length: usize = caption
                        .iter()
                        .chain(&capabilities)
                        .map(|c| c.chars().count() + 3)
                        .sum();
                    if length > CAPTION_CHARS {
                        caption.clear();
                    }
                    caption.extend(capabilities);
                }
            }
        }
        // Capitals, as designed, but the unit keeps its case.
        let caption = caption.join(" · ").to_uppercase().replace("KHZ", "kHz");
        let bit_perfect = output.signal == crate::backend::SignalState::BitPerfect;
        let row = self.row_widget(device_icon(device), &name, &caption, active, bit_perfect);
        if let Some(device) = device {
            let capabilities = capability_text(device);
            row.set_tooltip_text(Some(&capabilities));
            row.update_property(&[gtk::accessible::Property::Description(&capabilities)]);
        }
        let target = selection_of(device, resolved, current.mode);
        let weak = Rc::downgrade(self);
        row.connect_clicked(move |_| {
            if let Some(switcher) = weak.upgrade() {
                switcher.choose(target.clone(), &name);
            }
        });
        self.rows.append(&row);
    }

    fn choose(self: &Rc<Self>, target: OutputSelection, name: &str) {
        self.popover.popdown();
        let services = self.backend.services();
        if services.output() == target {
            return;
        }
        let mut after = (*services.settings()).clone();
        after.output.backend = target.backend;
        after.output.device = target.device.clone();
        let backend = self.backend.clone();
        let window = self.window.upgrade();
        let playback = self.playback.upgrade();
        volume_confirm::guard_change(
            window.as_ref(),
            &self.backend,
            playback.as_ref(),
            volume_confirm::Proposal {
                target: &target,
                device: name,
                after: &after,
            },
            {
                let target = target.clone();
                move || backend.services().set_output(target)
            },
            || {},
        );
    }

    fn set_mode(&self, mode: OutputMode) {
        let services = self.backend.services();
        let mut target = services.output();
        if target.mode == mode {
            return;
        }
        target.mode = mode;
        services.set_output(target);
    }
}

/// The signal path right now: the lamp with its state, then the path in two
/// mono lines, from the file to the device.
#[derive(Debug)]
struct Readout {
    lamp: SignalLamp,
    state: gtk::Label,
    source: gtk::Label,
    device: gtk::Label,
}

impl Readout {
    fn new() -> (gtk::Box, Self) {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("hk-readout");
        root.add_css_class("hk-switcher-readout");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let lamp = SignalLamp::default();
        lamp.set_valign(gtk::Align::Center);
        head.append(&lamp);
        let state = label("", &["hk-signal", "idle"]);
        state.set_ellipsize(gtk::pango::EllipsizeMode::None);
        head.append(&state);
        root.append(&head);
        let source = label("", &["hk-mono", "hk-readout-line"]);
        source.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        root.append(&source);
        let device = label("", &["hk-mono", "hk-readout-line"]);
        device.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        root.append(&device);
        root.set_accessible_role(gtk::AccessibleRole::Group);
        root.update_property(&[gtk::accessible::Property::Label(&tr(
            "Signal path right now",
        ))]);
        (
            root,
            Self {
                lamp,
                state,
                source,
                device,
            },
        )
    }

    fn show(&self, path: Option<&SignalPath>, output: &crate::backend::OutputInfo) {
        let signal = match path {
            _ if output.signal == SignalState::NotConnected => SignalState::NotConnected,
            Some(path) => SignalState::of(path),
            None => SignalState::Idle,
        };
        self.lamp.set_state(signal);
        self.state
            .set_text(&state_label(signal, path).unwrap_or_default());
        for class in ["idle", "bit-perfect", "processed"] {
            self.state.remove_css_class(class);
        }
        self.state.add_css_class(match signal {
            SignalState::BitPerfect => "bit-perfect",
            SignalState::Processed { .. } => "processed",
            _ => "idle",
        });
        let Some(path) = path else {
            self.source.set_text(&tr("Nothing is playing."));
            self.device.set_text(&output.device);
            return;
        };
        self.source.set_text(&fill(
            &tr("{} → {}"),
            &[&source_text(path), &device_name(path, &output.device)],
        ));
        let active = path.active_stages().count();
        let work = if active == 0 {
            tr("no processing")
        } else {
            let names: Vec<&str> = path.active_stages().map(|s| s.detail.as_str()).collect();
            names.join(" · ")
        };
        self.device
            .set_text(&fill(&tr("{}, {}"), &[&device_format(path), &work]));
        self.device.set_tooltip_text(Some(&path.to_string()));
    }
}

/// The popover's width and greatest height, and the heights of its fixed
/// parts (the stylesheet gives the same).
const WIDTH: i32 = 400;
const MAX_HEIGHT: i32 = 520;
const HEAD: i32 = 50;
const MODE: i32 = 48;
/// About the characters a device row's caption holds.
const CAPTION_CHARS: usize = 46;
/// The readout box with its margin.
const READOUT: i32 = 92;
/// The footer row with its top border.
const FOOTER: i32 = 45;
/// Opens the switcher above `button` when it is clicked.
pub(crate) fn attach(
    window: &gtk::Window,
    button: &gtk::Button,
    backend: &Backend,
    playback: &Rc<Playback>,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Top);
    popover.add_css_class("hk-popover");
    popover.add_css_class("hk-glass");
    popover.set_parent(button);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("hk-switcher");
    // 400 × at most 520 with the popover's border, as designed.
    content.set_size_request(WIDTH - 2, -1);

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("hk-switcher-head");
    head.append(&label(&tr("Output"), &["hk-title"]));
    let badge = label("", &["hk-badge"]);
    badge.set_valign(gtk::Align::Center);
    head.append(&badge);
    let count = label("", &["hk-t3", "hk-secondary"]);
    count.set_hexpand(true);
    count.set_xalign(1.0);
    head.append(&count);
    content.append(&head);

    let rows = gtk::Box::new(gtk::Orientation::Vertical, 2);
    rows.add_css_class("hk-switcher-rows");
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_propagate_natural_height(true);
    scroller.set_max_content_height(MAX_HEIGHT - 2 - HEAD - MODE - READOUT - FOOTER - 1);
    scroller.set_child(Some(&rows));
    content.append(&scroller);

    let line = gtk::Separator::new(gtk::Orientation::Horizontal);
    line.add_css_class("hk-divider");
    content.append(&line);
    let mode_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    mode_row.add_css_class("hk-switcher-mode");
    let mode_label = label(&tr("Mode"), &["hk-medium"]);
    mode_label.set_hexpand(true);
    mode_row.append(&mode_label);
    let (modes, mode_buttons) = segmented(&[tr("Shared"), tr("Exclusive")]);
    mode_row.append(&modes);
    content.append(&mode_row);
    let (readout_box, readout) = Readout::new();
    content.append(&readout_box);

    let footer = gtk::Button::new();
    footer.add_css_class("hk-switcher-footer");
    let footer_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let footer_label = label(&tr("Audio output settings"), &[]);
    footer_label.set_hexpand(true);
    footer_line.append(&footer_label);
    footer_line.append(&label("Ctrl ,", &["hk-key", "hint"]));
    footer.set_child(Some(&footer_line));
    footer.set_action_name(Some("app.preferences-output"));
    content.append(&footer);
    popover.set_child(Some(&content));

    let switcher = Rc::new(Switcher {
        popover: popover.clone(),
        rows,
        count,
        badge,
        readout,
        backend: backend.clone(),
        playback: Rc::downgrade(playback),
        window: window.downgrade(),
    });
    let weak = Rc::downgrade(&switcher);
    let current_mode = backend.services().output().mode;
    bind_segmented(
        &mode_buttons,
        usize::from(current_mode == OutputMode::Exclusive),
        move |i| {
            if let Some(switcher) = weak.upgrade() {
                switcher.set_mode(if i == 1 {
                    OutputMode::Exclusive
                } else {
                    OutputMode::Shared
                });
            }
        },
    );
    let weak = Rc::downgrade(&switcher);
    let buttons = mode_buttons.clone();
    popover.connect_show(move |_| {
        if let Some(switcher) = weak.upgrade() {
            let mode = switcher.backend.services().output().mode;
            if let Some(button) = buttons.get(usize::from(mode == OutputMode::Exclusive)) {
                button.set_active(true);
            }
            switcher.reload();
        }
    });
    let weak = popover.downgrade();
    button.connect_clicked(move |_| {
        if let Some(popover) = weak.upgrade() {
            popover.popup();
        }
    });
    // The button's handler keeps the switcher alive; the popover goes with
    // the button.
    let keep = switcher;
    let weak = popover.downgrade();
    button.connect_destroy(move |_| {
        let _ = &keep;
        if let Some(popover) = weak.upgrade() {
            popover.unparent();
        }
    });
    popover
}
