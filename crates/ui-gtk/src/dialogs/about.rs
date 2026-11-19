// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! About Helikodu: the version, the libraries in use, how the program was
//! installed, the offline promise, the licence, and "Copy diagnostics" for
//! bug reports. The project address is shown as text; the program opens no
//! web page itself.
//!
//! The diagnostics are the versions and the build, the system, the output
//! settings and devices, the loudness settings, the decoders and the state of
//! the library database. They hold no file paths but the settings file's
//! (with the home folder as `~`).

use gtk::prelude::*;
use gtk::{gdk, glib};

use super::common::{
    copy_text, dialog_window, find_open, header_strip, in_flatpak, note, sandbox_permissions, tilde,
};
use crate::backend::Backend;
use crate::backend::services::{Availability, DeviceSelection};
use crate::i18n::tr;
use crate::widgets::{label, wrapped_label};

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "about";

/// The project's address, shown as text.
const ADDRESS: &str = "github.com/sorrelfox/helikodu";

/// The program's version.
pub(crate) fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The GTK version in use and the windowing system.
fn gtk_line() -> String {
    let windowing = gdk::Display::default()
        .map(|d| d.type_().name().to_owned())
        .map(|name| {
            if name.contains("Wayland") {
                "Wayland".to_owned()
            } else if name.contains("X11") {
                "X11".to_owned()
            } else {
                name
            }
        })
        .unwrap_or_default();
    let version = format!(
        "{}.{}.{}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version()
    );
    if windowing.is_empty() {
        version
    } else {
        format!("{version} · {windowing}")
    }
}

/// How the program was installed.
fn installed_as() -> String {
    if in_flatpak() {
        return "Flatpak".into();
    }
    // The AppImage runtime names the image file it started from.
    if std::env::var_os("APPIMAGE").is_some() {
        return "AppImage".into();
    }
    match std::env::current_exe() {
        Ok(path) if path.starts_with("/usr") || path.starts_with("/app") => tr("system package"),
        Ok(_) => tr("built from source"),
        Err(_) => String::new(),
    }
}

/// The operating system's name, from `/etc/os-release`.
fn system_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_owned())
        })
        .unwrap_or_else(|| "Linux".into())
}

/// Facts for a bug report, as plain text.
pub(crate) fn diagnostics(backend: &Backend) -> String {
    let services = backend.services();
    let settings = services.settings();
    let versions = services.versions();
    let output = services.output();
    let mut lines = vec![
        format!(
            "Helikodu {} ({}, {} build)",
            version(),
            std::env::consts::ARCH,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        ),
        format!("GTK {}", gtk_line()),
        format!("System: {}", system_name()),
        format!(
            "Desktop: {} ({})",
            std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            std::env::var("XDG_SESSION_TYPE").unwrap_or_default()
        ),
        format!("Installed as: {}", installed_as()),
    ];
    for (name, value) in [
        ("FFmpeg", versions.ffmpeg),
        ("PipeWire", versions.pipewire),
        ("SQLite", versions.sqlite),
    ] {
        if let Some(value) = value {
            lines.push(format!("{name}: {value}"));
        }
    }
    let device = match &output.device {
        DeviceSelection::SystemDefault => "default".to_owned(),
        DeviceSelection::Device(id) => id.as_str().to_owned(),
    };
    lines.push(format!(
        "Output: {} {} {} (settings: {:?}, {:?}, {:?}, buffer {} ms)",
        services.resolve_backend(output.backend).as_str(),
        device,
        output.mode.as_str(),
        settings.output.sample_rate,
        settings.output.format,
        settings.output.volume,
        settings.output.buffer_ms
    ));
    lines.push(format!(
        "Loudness: {:?}, preamp {} dB, untagged {} dB",
        settings.loudness.mode, settings.loudness.preamp_db, settings.loudness.untagged_preamp_db
    ));
    lines.push(format!(
        "Library folders: {}",
        settings.library.folders.len()
    ));
    lines.push(format!(
        "Log: {}",
        match settings.advanced.log_detail {
            helikodu_model::settings::LogDetail::Off => "none kept",
            helikodu_model::settings::LogDetail::Normal => "normal detail",
            helikodu_model::settings::LogDetail::Full => "full detail",
        }
    ));
    if let Some(file) = services.settings_file() {
        lines.push(format!("Settings: {}", tilde(&file)));
    }
    let report = services.formats();
    let decoders: Vec<String> = report
        .decoders
        .iter()
        .map(|d| {
            format!(
                "{} {}",
                d.decoder.name(),
                if d.availability == Availability::Available {
                    d.version.clone().unwrap_or_else(|| "yes".into())
                } else {
                    "no".into()
                }
            )
        })
        .collect();
    lines.push(format!("Decoders: {}", decoders.join(", ")));
    let backend = services.resolve_backend(output.backend);
    match services.devices(backend) {
        Ok(devices) => {
            let names: Vec<String> = devices
                .iter()
                .map(|d| {
                    if d.is_default {
                        format!("{} (default)", d.description)
                    } else {
                        d.description.clone()
                    }
                })
                .collect();
            lines.push(format!(
                "Devices on {}: {}",
                backend.as_str(),
                names.join(", ")
            ));
        }
        Err(error) => lines.push(format!("Devices on {}: {}", backend.as_str(), error.0)),
    }
    if let Some(backups) = services.backups() {
        lines.push(format!(
            "Library database: {}{}, last check {}",
            if backups.read_only { "read-only, " } else { "" },
            match backups.latest {
                Some(at) => format!("latest backup {}", crate::format::local_time(at)),
                None => "no backup yet".to_owned(),
            },
            match &backups.check {
                Some((_, None)) => "found no problems".to_owned(),
                Some((_, Some(problem))) => format!("found: {problem}"),
                None => "not made yet".to_owned(),
            }
        ));
    }
    // File paths stay out: the library folders are counted above, and the
    // logs and crash reports are files the user can look at and attach.
    lines.join("\n") + "\n"
}

fn version_row(name: &str, value: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("hk-about-row");
    let key = label(name, &["hk-t2"]);
    key.set_size_request(132, -1);
    row.append(&key);
    row.append(&label(value, &["hk-mono"]));
    row
}

fn licences(parent: &gtk::Window) {
    let window = dialog_window(Some(parent), "licences", &tr("Licences"), (520, 420), true);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let (header, _) = header_strip(&window, &tr("Licences"), None);
    root.append(&header);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.add_css_class("hk-dialog-body");
    for text in [
        tr(
            "Helikodu is free software: you can redistribute it and change it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.",
        ),
        tr(
            "It is distributed in the hope that it will be useful, but without any warranty; without even the implied warranty of merchantability or fitness for a particular purpose. See the GNU General Public License for more details.",
        ),
        tr(
            "The libraries Helikodu uses keep their own licences; their texts are installed with the program.",
        ),
    ] {
        let paragraph = note(&text);
        paragraph.remove_css_class("hk-note");
        paragraph.add_css_class("hk-t2");
        body.append(&paragraph);
    }
    body.append(&label("SPDX: GPL-3.0-or-later", &["hk-mono", "hk-t3"]));
    root.append(&body);
    window.set_child(Some(&root));
    window.present();
}

/// What the Flatpak may do and why, the same text as the description in the
/// software centres.
fn permissions(parent: &gtk::Window) {
    let window = dialog_window(
        Some(parent),
        "permissions",
        &tr("Permissions"),
        (520, 420),
        true,
    );
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let (header, _) = header_strip(&window, &tr("Permissions"), None);
    root.append(&header);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.add_css_class("hk-dialog-body");
    body.append(&sandbox_permissions());
    root.append(&body);
    window.set_child(Some(&root));
    window.present();
}

/// Opens About over `parent`, or brings it to the front.
pub(crate) fn open(parent: &gtk::Window, backend: &Backend) -> gtk::Window {
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        return window;
    }
    let window = dialog_window(Some(parent), NAME, &tr("About Helikodu"), (520, 600), true);
    window.set_resizable(false);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let (header, _) = header_strip(&window, &tr("About"), None);
    root.append(&header);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("hk-about");

    let identity = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("hk-logo-tile");
    tile.add_css_class("hk-glass");
    tile.set_valign(gtk::Align::Start);
    let logo = gtk::Image::from_icon_name("helikodu-logo-symbolic");
    logo.set_pixel_size(36);
    logo.add_css_class("hk-accent-text");
    logo.set_vexpand(true);
    logo.set_valign(gtk::Align::Center);
    tile.append(&logo);
    identity.append(&tile);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.append(&label("Helikodu", &["hk-about-name"]));
    let mut facts = vec![version().to_owned()];
    if let Some(date) = option_env!("HELIKODU_BUILD_DATE") {
        facts.push(date.to_owned());
    }
    facts.push(std::env::consts::ARCH.to_owned());
    text.append(&label(
        &facts.join(" · "),
        &["hk-mono", "hk-t2", "hk-secondary"],
    ));
    text.append(&wrapped_label(
        &tr("A music player for Linux that works offline."),
        &["hk-t2"],
    ));
    identity.append(&text);
    body.append(&identity);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("hk-bordered-list");
    list.set_margin_top(22);
    let versions = backend.services().versions();
    list.append(&version_row("GTK", &gtk_line()));
    for (name, value) in [
        ("FFmpeg", versions.ffmpeg),
        ("PipeWire", versions.pipewire),
        ("SQLite", versions.sqlite),
    ] {
        if let Some(value) = value {
            list.append(&version_row(name, &value));
        }
    }
    let installed = installed_as();
    if !installed.is_empty() {
        list.append(&version_row(&tr("Installed as"), &installed));
    }
    let formats = gtk::Button::new();
    formats.add_css_class("hk-about-link");
    let formats_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let formats_label = label(&tr("Supported formats and decoders"), &[]);
    formats_label.set_hexpand(true);
    formats_line.append(&formats_label);
    formats_line.append(&gtk::Image::from_icon_name("go-next-symbolic"));
    formats.set_child(Some(&formats_line));
    // About is modal: a window opened beside it would take no clicks while
    // it stays open, hidden behind the larger Supported formats. It closes
    // first.
    let weak = window.downgrade();
    formats.connect_clicked(move |_| {
        let Some(about) = weak.upgrade() else { return };
        let application = about.application();
        about.close();
        if let Some(application) = application {
            application.activate_action("formats", None);
        }
    });
    list.append(&formats);
    body.append(&list);

    let privacy = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    privacy.set_margin_top(18);
    let lock = gtk::Image::from_icon_name("helikodu-lock-symbolic");
    lock.add_css_class("hk-t2");
    privacy.append(&lock);
    let promise = wrapped_label(
        &tr("No telemetry, no update checks. Helikodu never goes online."),
        &["hk-t2"],
    );
    promise.set_hexpand(true);
    privacy.append(&promise);
    body.append(&privacy);

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    body.append(&spacer);
    let legal = note(&tr(
        "© 2026 sorrelfox and Helikodu contributors. Free software under the GNU General Public License, version 3 or later. It comes with no warranty.",
    ));
    body.append(&legal);
    let address = label(ADDRESS, &["hk-mono", "hk-t2", "hk-secondary"]);
    address.set_selectable(true);
    address.set_margin_top(6);
    body.append(&address);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_margin_top(18);
    let copy = gtk::Button::with_label(&tr("Copy diagnostics"));
    copy.add_css_class("hk-btn");
    let backend_copy = backend.clone();
    let done = label("", &["hk-t3", "hk-secondary"]);
    let done_weak = done.downgrade();
    copy.connect_clicked(move |button| {
        copy_text(button, &diagnostics(&backend_copy));
        if let Some(done) = done_weak.upgrade() {
            done.set_text(&tr("Copied"));
            let weak = done.downgrade();
            glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
                if let Some(done) = weak.upgrade() {
                    done.set_text("");
                }
            });
        }
    });
    buttons.append(&copy);
    let licence = gtk::Button::with_label(&tr("Licences"));
    licence.add_css_class("hk-btn");
    let weak = window.downgrade();
    licence.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            licences(&window);
        }
    });
    buttons.append(&licence);
    // In the Flatpak: what it may do and why.
    if in_flatpak() {
        let sandbox = gtk::Button::with_label(&tr("Permissions"));
        sandbox.add_css_class("hk-btn");
        let weak = window.downgrade();
        sandbox.connect_clicked(move |_| {
            if let Some(window) = weak.upgrade() {
                permissions(&window);
            }
        });
        buttons.append(&sandbox);
    }
    done.set_hexpand(true);
    buttons.append(&done);
    let close = gtk::Button::with_label(&tr("Close"));
    close.add_css_class("hk-btn");
    let weak = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    buttons.append(&close);
    body.append(&buttons);
    root.append(&body);
    window.set_child(Some(&root));
    window.set_default_widget(Some(&close));
    window.present();
    window
}
