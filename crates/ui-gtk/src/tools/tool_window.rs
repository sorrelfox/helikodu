// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The frame the tool windows share: the left pane (232 px on `side`, the
//! window title in a 46 px strip, a section label, two-line rows with a count,
//! a note at the bottom), the header with the name of what is chosen and a
//! summary, the table header, and the 52 px footer with a summary and the
//! primary button. Also the cells of the duplicate tables.

use std::path::Path;
use std::time::SystemTime;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::AudioFormat;
use crate::backend::services::DuplicateEntry;
use crate::dialogs::common::{dialog_window, header_strip, tilde};
use crate::widgets::label;

/// The width of the Remove and Main columns in English; see [`check_width`].
pub(crate) const CHECK: i32 = 52;
/// The width of the Format column.
pub(crate) const FORMAT: i32 = 96;
/// The width of the Size column.
pub(crate) const SIZE: i32 = 60;
/// The width of the Plays column.
pub(crate) const PLAYS: i32 = 40;
/// The width of the Added column.
pub(crate) const ADDED: i32 = 86;
/// The gap between the columns.
pub(crate) const GAP: i32 = 14;

/// The width of the Remove and Main columns: 52 px, or what their headers
/// need in a language where they are longer, so that the check marks stay
/// under their headers.
pub(crate) fn check_width() -> i32 {
    thread_local! {
        static WIDTH: std::cell::OnceCell<i32> = const { std::cell::OnceCell::new() };
    }
    WIDTH.with(|width| {
        *width.get_or_init(|| {
            check_headers()
                .iter()
                .map(|text| {
                    let cell = label(text, &["hk-section-label"]);
                    cell.set_ellipsize(gtk::pango::EllipsizeMode::None);
                    cell.measure(gtk::Orientation::Horizontal, -1).1
                })
                .fold(CHECK, i32::max)
        })
    })
}

/// The headers of the Remove and Main columns.
pub(crate) fn check_headers() -> [String; 2] {
    [
        // Translators: a narrow column header of the identical copies: the
        // copy that is kept and plays.
        crate::i18n::trc("column", "Main"),
        // Translators: a narrow column header of the duplicate tables: the
        // copies to move to the trash.
        crate::i18n::trc("column", "Remove"),
    ]
}

/// The parts of a tool window that the tool fills.
pub(crate) struct Frame {
    pub(crate) window: gtk::Window,
    /// The rows of the left pane.
    pub(crate) list: gtk::ListBox,
    /// The note at the bottom of the left pane.
    pub(crate) side_note: gtk::Label,
    /// The name of what is chosen, in the header.
    pub(crate) title: gtk::Label,
    /// The summary at the right of the header.
    pub(crate) summary: gtk::Label,
    /// The right side under the header: options, table and footer go here.
    pub(crate) main: gtk::Box,
}

/// A tool window over `parent`, 1080 × 720, not modal.
pub(crate) fn frame(parent: &gtk::Window, name: &str, title: &str, section: &str) -> Frame {
    let window = dialog_window(Some(parent), name, title, (1080, 720), false);
    window.add_css_class("hk-tool");
    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);

    let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side.add_css_class("hk-settings-nav");
    side.set_size_request(232, -1);
    side.set_hexpand(false);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    head.add_css_class("hk-strip");
    head.add_css_class("plain");
    head.append(&label(title, &["hk-title"]));
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&head));
    side.append(&handle);
    side.append(&label(
        section,
        &["hk-section-label", "hk-list-section", "first"],
    ));
    let list = gtk::ListBox::new();
    list.add_css_class("hk-list");
    list.set_margin_start(8);
    list.set_margin_end(8);
    list.set_vexpand(true);
    list.set_valign(gtk::Align::Start);
    list.update_property(&[gtk::accessible::Property::Label(section)]);
    side.append(&list);
    let side_note = label("", &["hk-t3", "hk-secondary", "hk-side-foot"]);
    side_note.set_wrap(true);
    side_note.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    // A wrapped label asks for its whole text on one line otherwise.
    side_note.set_max_width_chars(24);
    side_note.set_single_line_mode(false);
    side_note.set_ellipsize(gtk::pango::EllipsizeMode::None);
    side.append(&side_note);
    layout.append(&side);

    let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.set_hexpand(true);
    main.add_css_class("hk-settings-main");
    let (header, strip) = header_strip(&window, "", Some(""));
    main.append(&header);
    let title = strip
        .first_child()
        .and_downcast::<gtk::Label>()
        .unwrap_or_default();
    title.set_accessible_role(gtk::AccessibleRole::Heading);
    let summary = title
        .next_sibling()
        .and_downcast::<gtk::Label>()
        .unwrap_or_default();
    layout.append(&main);
    window.set_child(Some(&layout));
    Frame {
        window,
        list,
        side_note,
        title,
        summary,
        main,
    }
}

/// A row of the left pane: the name over a line in 11 px `t3`, and a count at
/// the right. Returns the row and its count label.
pub(crate) fn side_row(name: &str, line: &str) -> (gtk::ListBoxRow, gtk::Label) {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("two-line");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&label(name, &["hk-side-name"]));
    text.append(&label(line, &["hk-t3", "hk-side-line"]));
    content.append(&text);
    let count = label("", &["hk-count"]);
    count.set_valign(gtk::Align::Center);
    content.append(&count);
    row.set_child(Some(&content));
    row.update_property(&[gtk::accessible::Property::Label(name)]);
    (row, count)
}

/// The table header: `columns` of (name, width or -1 to expand, right-aligned).
pub(crate) fn table_header(columns: &[(String, i32, bool)]) -> gtk::Box {
    let head = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    head.add_css_class("hk-table-header");
    head.add_css_class("hk-tool-head");
    for (text, width, right) in columns {
        let cell = label(text, &["hk-section-label"]);
        cell.set_size_request(*width, -1);
        cell.set_hexpand(*width < 0);
        if *right {
            cell.set_xalign(1.0);
        }
        head.append(&cell);
    }
    head
}

/// The footer: a one-line summary and room for buttons at the right. Returns
/// the footer and its summary.
pub(crate) fn footer() -> (gtk::Box, gtk::Label) {
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("hk-tool-footer");
    let summary = label("", &["hk-t3", "hk-secondary"]);
    summary.set_hexpand(true);
    footer.append(&summary);
    (footer, summary)
}

/// The primary button of a tool, 30 px high; its label carries the number.
pub(crate) fn primary_button(text: &str) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("hk-btn");
    button.add_css_class("primary");
    button.add_css_class("compact");
    button.set_valign(gtk::Align::Center);
    button
}

/// A cell of a table row: `text` at `width` (or expanding for -1).
pub(crate) fn cell(width: i32, classes: &[&str], right: bool) -> gtk::Label {
    let cell = label("", classes);
    cell.set_size_request(width, -1);
    cell.set_hexpand(width < 0);
    if right {
        cell.set_xalign(1.0);
    }
    cell
}

/// A path cell: mono, shortened at the start so the file name stays.
pub(crate) fn path_cell() -> gtk::Label {
    let path = cell(-1, &["hk-mono", "hk-tool-path"], false);
    path.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    path.set_max_width_chars(1);
    path
}

/// A path as shown: the home folder as `~`.
pub(crate) fn shown_path(path: &Path) -> String {
    tilde(path)
}

/// A date as the tables show it: `12 Oct 2026`.
pub(crate) fn date(time: Option<SystemTime>) -> String {
    let Some(seconds) = time
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
    else {
        return String::new();
    };
    glib::DateTime::from_unix_local(seconds)
        .ok()
        .and_then(|t| t.format("%-d %b %Y").ok())
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// The format of a duplicate: `FLAC 24/96`, `MP3 192`.
pub(crate) fn entry_format(entry: &DuplicateEntry) -> String {
    crate::format::short(&AudioFormat {
        codec: entry.codec.as_ref().map(|c| c.to_uppercase()),
        sample_rate: entry.sample_rate,
        bits: entry.bit_depth.and_then(|b| u16::try_from(b).ok()),
        bitrate_kbps: entry.bitrate.map(|b| b / 1000),
        channels: None,
    })
}

/// The list of the left pane of the tool window `window`.
pub(crate) fn side_list(window: &gtk::Window) -> Option<gtk::ListBox> {
    let mut pending: Vec<gtk::Widget> = window.child().into_iter().collect();
    while let Some(widget) = pending.pop() {
        if let Some(list) = widget.downcast_ref::<gtk::ListBox>()
            && list.has_css_class("hk-list")
        {
            return Some(list.clone());
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c);
        }
    }
    None
}
