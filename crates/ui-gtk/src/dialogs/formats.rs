// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Supported formats: what this computer can play, read from the decoders of
//! this build when the window opens. The decoders at the left filter the
//! table; a text filter and All, Available and Missing narrow it further.
//! Status is an icon and words, never colour alone. "Copy as text" puts the
//! table on the clipboard for bug reports.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::common::{bind_segmented, copy_text, dialog_window, find_open, header_strip, segmented};
use crate::backend::services::{
    Availability, Decoder, DecoderStatus, FamilyStatus, FormatFamily, FormatReport,
};
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "formats";

/// The family's name as people know it.
pub(crate) fn family_name(family: FormatFamily) -> String {
    match family {
        FormatFamily::MainstreamLossy => tr("Mainstream lossy"),
        FormatFamily::Lossless => tr("Lossless and PCM"),
        FormatFamily::ExtendedLossless => tr("Extended lossless"),
        FormatFamily::LegacyAndSpeech => tr("Legacy and speech"),
        FormatFamily::Speex => "Speex".into(),
        FormatFamily::LargePcmContainers => tr("Large PCM containers"),
        FormatFamily::VideoAudio => tr("Audio of video files"),
        FormatFamily::Dsd => "DSD".into(),
        FormatFamily::TrackerModules => tr("Tracker modules"),
        FormatFamily::GameMusic => tr("Game music"),
        FormatFamily::Sid => tr("C64 SID"),
        FormatFamily::GameAudioStreams => tr("Game audio streams"),
        FormatFamily::Midi => "MIDI".into(),
        _ => tr("Other formats"),
    }
}

fn available(availability: Availability) -> bool {
    availability == Availability::Available
}

/// Why a decoder or family is missing, in plain words.
fn missing_reason(decoder: Decoder) -> String {
    fill(&tr("This package was built without {}."), &[decoder.name()])
}

fn decoder_line(status: &DecoderStatus) -> String {
    if available(status.availability) {
        match &status.version {
            Some(version) => fill(&tr("{} · built in"), &[version]),
            None => tr("built in"),
        }
    } else {
        tr("not in this package")
    }
}

/// The table as plain text, for bug reports.
pub(crate) fn as_text(report: &FormatReport) -> String {
    let mut text = String::new();
    for status in &report.decoders {
        text.push_str(&format!(
            "{}: {}\n",
            status.decoder.name(),
            decoder_line(status)
        ));
    }
    text.push('\n');
    for family in &report.families {
        let state = if available(family.availability) {
            tr("Available")
        } else {
            tr("Not available")
        };
        text.push_str(&format!(
            "{} ({}): {} · {}\n",
            family_name(family.family),
            family.family.formats().join(", "),
            family.decoder.name(),
            state
        ));
    }
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Show {
    All,
    Available,
    Missing,
}

struct Formats {
    report: FormatReport,
    rows: Vec<(FamilyStatus, gtk::ListBoxRow)>,
    decoder: Cell<Option<Decoder>>,
    show: Cell<Show>,
    text: RefCell<String>,
    title: gtk::Label,
}

impl Formats {
    fn matches(&self, family: &FamilyStatus) -> bool {
        if let Some(decoder) = self.decoder.get()
            && !family.family.decoders().contains(&decoder)
        {
            return false;
        }
        match self.show.get() {
            Show::Available if !available(family.availability) => return false,
            Show::Missing if available(family.availability) => return false,
            _ => {}
        }
        let text = self.text.borrow().to_lowercase();
        if text.is_empty() {
            return true;
        }
        family_name(family.family).to_lowercase().contains(&text)
            || family
                .family
                .formats()
                .iter()
                .any(|f| f.to_lowercase().contains(&text))
    }

    fn refilter(&self) {
        for (family, row) in &self.rows {
            row.set_visible(self.matches(family));
        }
        self.title.set_text(&match self.decoder.get() {
            Some(decoder) => decoder.name().to_owned(),
            None => tr("All formats"),
        });
    }
}

fn status_cell(family: &FamilyStatus) -> gtk::Box {
    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    cell.set_size_request(150, -1);
    let ok = available(family.availability);
    let icon = gtk::Image::from_icon_name(if ok {
        "helikodu-check-symbolic"
    } else {
        "action-unavailable-symbolic"
    });
    icon.add_css_class(if ok { "hk-t2" } else { "hk-t1" });
    cell.append(&icon);
    let text = if ok {
        label(&tr("Available"), &["hk-t2"])
    } else {
        label(&tr("Not available"), &["hk-strong"])
    };
    cell.append(&text);
    cell
}

fn family_row(family: &FamilyStatus) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.add_css_class("hk-format-row");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let name = label(&family_name(family.family), &["hk-medium"]);
    name.set_size_request(148, -1);
    line.append(&name);
    let formats = label(
        &family.family.formats().join(", "),
        &["hk-mono", "hk-t2", "hk-formats"],
    );
    formats.set_hexpand(true);
    line.append(&formats);
    let decoder = label(family.decoder.name(), &["hk-t2"]);
    decoder.set_size_request(100, -1);
    line.append(&decoder);
    line.append(&status_cell(family));
    column.append(&line);
    if !available(family.availability) {
        let reason = label(&missing_reason(family.decoder), &["hk-t2", "hk-secondary"]);
        reason.set_margin_start(160);
        reason.set_wrap(true);
        column.append(&reason);
    }
    row.set_child(Some(&column));
    row
}

fn decoder_row(title: &str, line: &str, icon: Option<(&str, bool)>, dim: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&label(title, if dim { &["hk-t3"][..] } else { &[] }));
    text.append(&label(line, &["hk-mono-meta", "hk-t3"]));
    content.append(&text);
    if let Some((name, strong)) = icon {
        let image = gtk::Image::from_icon_name(name);
        image.add_css_class(if strong { "hk-t1" } else { "hk-t3" });
        content.append(&image);
    }
    row.set_child(Some(&content));
    row.add_css_class("two-line");
    row
}

/// Opens Supported formats over `parent`, or brings it to the front.
pub(crate) fn open(parent: &gtk::Window, report: FormatReport) -> gtk::Window {
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        return window;
    }
    let window = dialog_window(
        Some(parent),
        NAME,
        &tr("Supported formats"),
        (1080, 720),
        false,
    );
    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);

    // Decoders.
    let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side.add_css_class("hk-settings-nav");
    side.set_size_request(232, -1);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    head.add_css_class("hk-strip");
    head.add_css_class("plain");
    head.append(&label(&tr("Supported formats"), &["hk-title"]));
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&head));
    side.append(&handle);
    side.append(&label(
        &tr("Decoders"),
        &["hk-section-label", "hk-list-section", "first"],
    ));
    let decoders = gtk::ListBox::new();
    decoders.add_css_class("hk-list");
    decoders.set_margin_start(8);
    decoders.set_margin_end(8);
    decoders.update_property(&[gtk::accessible::Property::Label(&tr("Decoders"))]);
    let families = report.families.len();
    decoders.append(&decoder_row(
        &tr("All decoders"),
        &fill(
            &ntr("{} format family", "{} format families", families),
            &[&families.to_string()],
        ),
        None,
        false,
    ));
    for status in &report.decoders {
        let ok = available(status.availability);
        decoders.append(&decoder_row(
            status.decoder.name(),
            &decoder_line(status),
            Some(if ok {
                ("helikodu-check-symbolic", false)
            } else {
                ("action-unavailable-symbolic", true)
            }),
            !ok,
        ));
    }
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&decoders));
    side.append(&scroller);
    let when = glib::DateTime::now_local()
        .ok()
        .and_then(|t| t.format("%X").ok())
        .map(|t| t.to_string())
        .unwrap_or_default();
    let checked = label(
        &fill(&tr("Checked when this window opened, at {}."), &[&when]),
        &["hk-t3", "hk-secondary", "hk-side-foot"],
    );
    checked.set_wrap(true);
    side.append(&checked);
    layout.append(&side);

    // The table.
    let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.set_hexpand(true);
    main.add_css_class("hk-settings-main");
    let (header, strip) = header_strip(
        &window,
        &tr("All formats"),
        Some(&tr("What this computer can play")),
    );
    main.append(&header);
    let title = strip
        .first_child()
        .and_downcast::<gtk::Label>()
        .unwrap_or_default();
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    toolbar.add_css_class("hk-toolbar");
    let filter = gtk::SearchEntry::new();
    filter.add_css_class("hk-input");
    filter.set_hexpand(true);
    filter.set_valign(gtk::Align::Center);
    filter.set_placeholder_text(Some(&tr(
        "Find a format or file extension, for example opus",
    )));
    toolbar.append(&filter);
    let total = report.families.len();
    let ok = report.available();
    let (group, buttons) = segmented(&[
        fill(&tr("All {}"), &[&total.to_string()]),
        fill(&tr("Available {}"), &[&ok.to_string()]),
        fill(&tr("Missing {}"), &[&(total - ok).to_string()]),
    ]);
    toolbar.append(&group);
    main.append(&toolbar);
    let table_head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    table_head.add_css_class("hk-table-header");
    table_head.add_css_class("hk-format-head");
    for (text, width, expand) in [
        (tr("Family"), 148, false),
        (tr("Formats"), -1, true),
        (tr("Decoder"), 100, false),
        (tr("Status"), 150, false),
    ] {
        let head = label(&text, &["hk-section-label"]);
        head.set_size_request(width, -1);
        head.set_hexpand(expand);
        table_head.append(&head);
    }
    main.append(&table_head);
    let table = gtk::ListBox::new();
    table.add_css_class("hk-format-table");
    table.set_selection_mode(gtk::SelectionMode::None);
    table.update_property(&[gtk::accessible::Property::Label(&tr("Format families"))]);
    let mut rows = Vec::new();
    for family in &report.families {
        let row = family_row(family);
        table.append(&row);
        rows.push((family.clone(), row));
    }
    let table_scroller = gtk::ScrolledWindow::new();
    table_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    table_scroller.set_vexpand(true);
    table_scroller.set_child(Some(&table));
    main.append(&table_scroller);
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    footer.add_css_class("hk-format-footer");
    let summary = label(
        &fill(
            &tr("{} families available · {} not available on this computer"),
            &[&ok.to_string(), &(total - ok).to_string()],
        ),
        &["hk-t3", "hk-secondary"],
    );
    summary.set_hexpand(true);
    footer.append(&summary);
    let copy = super::common::compact_button(&tr("Copy as text"));
    footer.append(&copy);
    main.append(&footer);
    layout.append(&main);
    window.set_child(Some(&layout));

    let state = Rc::new(Formats {
        report,
        rows,
        decoder: Cell::new(None),
        show: Cell::new(Show::All),
        text: RefCell::new(String::new()),
        title,
    });
    let weak = Rc::downgrade(&state);
    copy.connect_clicked(move |button| {
        if let Some(state) = weak.upgrade() {
            copy_text(button, &as_text(&state.report));
        }
    });
    let weak = Rc::downgrade(&state);
    bind_segmented(&buttons, 0, move |i| {
        if let Some(state) = weak.upgrade() {
            state
                .show
                .set([Show::All, Show::Available, Show::Missing][i]);
            state.refilter();
        }
    });
    let weak = Rc::downgrade(&state);
    filter.connect_search_changed(move |entry| {
        if let Some(state) = weak.upgrade() {
            state.text.replace(entry.text().to_string());
            state.refilter();
        }
    });
    let weak = Rc::downgrade(&state);
    decoders.connect_row_selected(move |_, row| {
        if let (Some(state), Some(row)) = (weak.upgrade(), row) {
            let index = usize::try_from(row.index()).unwrap_or(0);
            state.decoder.set(
                index
                    .checked_sub(1)
                    .and_then(|i| state.report.decoders.get(i))
                    .map(|d| d.decoder),
            );
            state.refilter();
        }
    });
    if let Some(row) = decoders.row_at_index(0) {
        decoders.select_row(Some(&row));
    }
    let keep = RefCell::new(Some(state));
    window.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    window.present();
    window
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_lists_every_family() {
        let report = helikodu_service::formats::supported_formats();
        let text = as_text(&report);
        assert!(text.contains("FFmpeg"));
        assert_eq!(
            text.lines().filter(|l| l.contains(" · ")).count(),
            report.families.len()
                + report
                    .decoders
                    .iter()
                    .filter(|d| d.version.is_some())
                    .count()
        );
    }
}
