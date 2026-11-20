// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts of the Import window that show a plan or an earlier import: the
//! figures strip, the rows of "What changes" with the figure in 500 and the
//! rest in `t3`, the playlists and what each becomes, the boxes of the fields
//! that differ, the notice, and the popover that lists entries in words.

use std::time::SystemTime;

use gtk::glib;
use gtk::prelude::*;

use super::tool_window::date;
use crate::backend::interchange::{
    FieldConflict, PlanTotals, PlannedPlaylist, PlannedPlaylistKind,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use helikodu_model::user_fields::FieldDefinition;

/// The most entries an import lists by name.
pub(super) const LISTED: usize = 200;

/// One figure of the strip: the number, its label, and a caption with an
/// optional link.
pub(super) struct Figure {
    pub(super) value: gtk::Label,
    pub(super) caption: gtk::Label,
    pub(super) link: gtk::Button,
}

/// The strip of four figures under the options.
pub(super) struct Figures {
    pub(super) root: gtk::Box,
    pub(super) matched: Figure,
    pub(super) unmatched: Figure,
    pub(super) ambiguous: Figure,
    pub(super) unchanged: Figure,
}

fn figure(name: &str, widget: &str) -> (gtk::Box, Figure) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 1);
    column.set_widget_name(widget);
    let value = label("", &["hk-import-figure", "hk-numeric"]);
    column.append(&value);
    column.append(&label(name, &["hk-import-figure-label"]));
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let caption = label("", &["hk-import-figure-caption"]);
    line.append(&caption);
    let link = gtk::Button::with_label("");
    link.add_css_class("hk-import-link");
    link.set_widget_name(&format!("{widget}-show"));
    link.set_visible(false);
    line.append(&link);
    column.append(&line);
    (
        column,
        Figure {
            value,
            caption,
            link,
        },
    )
}

impl Figures {
    pub(super) fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        root.set_homogeneous(true);
        root.add_css_class("hk-import-figures");
        let (a, matched) = figure(&tr("Matched"), "figure-matched");
        let (b, unmatched) = figure(&tr("Not in your library"), "figure-unmatched");
        let (c, ambiguous) = figure(&tr("More than one fit"), "figure-ambiguous");
        let (d, unchanged) = figure(&tr("Unchanged"), "figure-unchanged");
        for column in [&a, &b, &c, &d] {
            root.append(column);
        }
        Self {
            root,
            matched,
            unmatched,
            ambiguous,
            unchanged,
        }
    }

    /// The figures while a library is read: "…" for each.
    pub(super) fn reading(&self) {
        for figure in [
            &self.matched,
            &self.unmatched,
            &self.ambiguous,
            &self.unchanged,
        ] {
            figure.value.set_text("…");
            figure.caption.set_text("");
            figure.link.set_visible(false);
        }
    }

    /// Shows `matching` and `unchanged`; `links` shows the links that list
    /// the entries left out.
    pub(super) fn show(
        &self,
        matching: &crate::backend::interchange::MatchCounts,
        unchanged: usize,
        links: bool,
    ) {
        self.matched
            .value
            .set_text(&format::count(matching.by_path + matching.by_tags));
        self.matched.caption.set_text(&fill(
            // Translators: the caption of the Matched figure of an import.
            &tr("{} by path · {} by tags"),
            &[
                &format::count(matching.by_path),
                &format::count(matching.by_tags),
            ],
        ));
        self.unmatched
            .value
            .set_text(&format::count(matching.unmatched));
        self.unmatched.caption.set_text("");
        self.unmatched.caption.set_visible(false);
        self.unmatched
            .link
            .set_label(&if matching.unmatched > LISTED {
                fill(&tr("Show the first {}"), &[&format::count(LISTED)])
            } else {
                tr("Show")
            });
        self.unmatched
            .link
            .set_visible(links && matching.unmatched > 0);
        self.ambiguous
            .value
            .set_text(&format::count(matching.ambiguous));
        self.ambiguous.caption.set_text(&tr("left out"));
        self.ambiguous.link.set_label(&tr("Show"));
        self.ambiguous
            .link
            .set_visible(links && matching.ambiguous > 0);
        self.unchanged.value.set_text(&format::count(unchanged));
        self.unchanged.caption.set_text(&tr("nothing new for them"));
    }
}

/// `template` with the number `n` in 500 and the words around it in `t3`:
/// "{} tracks" gives "1,240" and " tracks".
pub(super) fn counted(template: &str, n: usize) -> gtk::Box {
    counted_as(template, n, true)
}

/// [`counted`], with the number in `t3` too unless `strong`.
fn counted_as(template: &str, n: usize, strong: bool) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let (before, after) = template.split_once("{}").unwrap_or(("", template));
    if !before.is_empty() {
        let before = label(before, &["hk-t3"]);
        before.set_ellipsize(gtk::pango::EllipsizeMode::None);
        line.append(&before);
    }
    let number = if strong {
        ["hk-medium", "hk-numeric"]
    } else {
        ["hk-t3", "hk-numeric"]
    };
    line.append(&label(&format::count(n), &number));
    if !after.is_empty() {
        let rest = label(after, &["hk-t3"]);
        rest.set_ellipsize(gtk::pango::EllipsizeMode::None);
        line.append(&rest);
    }
    line
}

/// One row of "What changes": what, then its figure.
pub(super) fn change_row(what: &str, figure: &gtk::Box) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("hk-import-change");
    let name = label(what, &[]);
    name.set_hexpand(true);
    name.set_valign(gtk::Align::Center);
    row.append(&name);
    figure.set_valign(gtk::Align::Center);
    figure.set_halign(gtk::Align::End);
    row.append(figure);
    row
}

/// The rows of "What changes" for `totals`: those with nothing are left out.
/// `user_data` takes the words of a data file, whose counts are raised, not
/// added.
pub(super) fn change_rows(totals: &PlanTotals, user_data: bool) -> Vec<gtk::Box> {
    let tracks = |n: usize| counted(&ntr("{} track", "{} tracks", n), n);
    let mut rows = Vec::new();
    let mut add = |what: String, n: usize, figure: gtk::Box| {
        if n > 0 {
            rows.push(change_row(&what, &figure));
        }
    };
    if user_data {
        add(
            tr("Play counts raised"),
            totals.play_counts,
            tracks(totals.play_counts),
        );
        add(
            tr("Skip counts raised"),
            totals.skip_counts,
            tracks(totals.skip_counts),
        );
    } else {
        let on = |added: u64, on: usize| {
            let line = counted("{}", usize::try_from(added).unwrap_or(usize::MAX));
            line.append(&counted_as(
                // Translators: after a number of plays or skips added: "48,330
                // on 6,102 tracks".
                &ntr(" on {} track", " on {} tracks", on),
                on,
                false,
            ));
            line
        };
        add(
            tr("Plays added"),
            totals.play_counts,
            on(totals.plays_added, totals.play_counts),
        );
        add(
            tr("Skips added"),
            totals.skip_counts,
            on(totals.skips_added, totals.skip_counts),
        );
    }
    add(tr("Ratings set"), totals.ratings, tracks(totals.ratings));
    add(
        tr("Your rating kept"),
        totals.rating_conflicts,
        tracks(totals.rating_conflicts),
    );
    add(tr("Marked loved"), totals.loved, tracks(totals.loved));
    add(
        tr("First play moved earlier"),
        totals.first_played,
        tracks(totals.first_played),
    );
    add(
        tr("Last play moved later"),
        totals.last_played,
        tracks(totals.last_played),
    );
    add(
        tr("Date added moved earlier"),
        totals.added,
        tracks(totals.added),
    );
    add(
        tr("Listening history entries added"),
        totals.history,
        counted("{}", totals.history),
    );
    add(
        tr("Saved searches added"),
        totals.saved_searches,
        counted("{}", totals.saved_searches),
    );
    add(
        tr("Tracks added to Favourites"),
        totals.favourites,
        counted("{}", totals.favourites),
    );
    add(
        tr("Labels made"),
        totals.labels,
        counted("{}", totals.labels),
    );
    if totals.label_links > 0 {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let parts = [
            (
                totals.label_tracks,
                ntr("{} track", "{} tracks", totals.label_tracks),
            ),
            (
                totals.label_albums,
                ntr("{} album", "{} albums", totals.label_albums),
            ),
            (
                totals.label_artists,
                ntr("{} artist", "{} artists", totals.label_artists),
            ),
        ];
        let mut first = true;
        for (n, template) in parts {
            if n == 0 {
                continue;
            }
            if !first {
                line.append(&label(" · ", &["hk-t3"]));
            }
            line.append(&counted_as(&template, n, first));
            first = false;
        }
        add(tr("Labels given"), totals.label_links, line);
    }
    add(
        tr("Labels left out"),
        totals.label_unmatched,
        counted(
            // Translators: after the number of labels an import leaves out.
            &tr("{} · nothing to give them to"),
            totals.label_unmatched,
        ),
    );
    add(
        tr("Fields defined"),
        totals.fields,
        counted("{}", totals.fields),
    );
    rows
}

/// What the second line of a playlist row says, and whether the row is dim.
fn playlist_line(
    playlist: &PlannedPlaylist,
    earlier: Option<SystemTime>,
    past: bool,
    user_data: bool,
) -> String {
    match &playlist.kind {
        PlannedPlaylistKind::Static {
            items,
            from_rules: true,
            ..
        } => fill(
            &ntr(
                "The {} track it held: its rules cannot be read",
                "The {} tracks it held: its rules cannot be read",
                *items,
            ),
            &[&format::count(*items)],
        ),
        PlannedPlaylistKind::Static { items, matched, .. } => {
            let all = fill(
                &ntr("{} track", "{} tracks", *items),
                &[&format::count(*items)],
            );
            let missing = items.saturating_sub(*matched);
            if missing == 0 {
                fill(&tr("{}, all in your library"), &[&all])
            } else if past {
                fill(
                    &ntr(
                        "{}, {} of them waiting for its file",
                        "{}, {} of them waiting for their file",
                        missing,
                    ),
                    &[&all, &format::count(missing)],
                )
            } else {
                fill(
                    &ntr(
                        "{}, {} not in your library",
                        "{}, {} not in your library",
                        missing,
                    ),
                    &[&all, &format::count(missing)],
                )
            }
        }
        PlannedPlaylistKind::Smart(query) => query.clone(),
        PlannedPlaylistKind::Unmapped(reason) => reason.clone(),
        PlannedPlaylistKind::Exists if user_data => tr("A playlist of this name is here already"),
        PlannedPlaylistKind::Exists => match earlier {
            Some(at) => fill(&tr("From this file on {}"), &[&date(Some(at))]),
            None => tr("From this file, imported before"),
        },
    }
}

/// A row of the Playlists column: the name with its folder, what it holds,
/// and what it becomes.
pub(super) fn playlist_row(
    playlist: &PlannedPlaylist,
    earlier: Option<SystemTime>,
    past: bool,
    user_data: bool,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("hk-import-playlist");
    let dim = matches!(playlist.kind, PlannedPlaylistKind::Exists);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let name_line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    if !playlist.folder.is_empty() {
        name_line.append(&label(
            &format!("{} / ", playlist.folder.join(" / ")),
            &["hk-t3"],
        ));
    }
    let name = label(&playlist.name, if dim { &["hk-t3"] } else { &[] });
    name_line.append(&name);
    text.append(&name_line);
    let second = playlist_line(playlist, earlier, past, user_data);
    let line = if matches!(playlist.kind, PlannedPlaylistKind::Smart(_)) {
        label(&second, &["hk-import-query", "hk-t3"])
    } else {
        label(&second, &["hk-import-line", "hk-t3"])
    };
    line.set_tooltip_text(Some(&second));
    text.append(&line);
    row.append(&text);
    let becomes = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    becomes.set_valign(gtk::Align::Center);
    let what = match &playlist.kind {
        PlannedPlaylistKind::Static { .. } => label(&tr("Playlist"), &[]),
        PlannedPlaylistKind::Smart(_) => label(&tr("Smart playlist"), &[]),
        PlannedPlaylistKind::Unmapped(_) => {
            let icon = gtk::Image::from_icon_name("helikodu-warning-symbolic");
            becomes.append(&icon);
            label(&tr("Left out"), &["hk-import-left-out"])
        }
        PlannedPlaylistKind::Exists => label(&tr("Imported before"), &["hk-t3"]),
    };
    becomes.append(&what);
    row.append(&becomes);
    row.update_property(&[gtk::accessible::Property::Label(&fill(
        &tr("{}: {}, {}"),
        &[&playlist.name, &what.text(), &second],
    ))]);
    row
}

/// The Favourites row of a data file.
pub(super) fn favourites_row(tracks: usize) -> gtk::Box {
    let favourites = PlannedPlaylist {
        name: tr("Favourites"),
        folder: Vec::new(),
        kind: PlannedPlaylistKind::Static {
            items: tracks,
            matched: tracks,
            from_rules: false,
        },
    };
    let row = playlist_row(&favourites, None, false, true);
    let text = fill(
        &ntr(
            "{} track is added to yours",
            "{} tracks are added to yours",
            tracks,
        ),
        &[&format::count(tracks)],
    );
    // The second line and the right side say what happens to Favourites.
    if let Some(column) = row.first_child()
        && let Some(line) = column.last_child().and_downcast::<gtk::Label>()
    {
        line.set_text(&text);
    }
    if let Some(what) = row
        .last_child()
        .and_then(|b| b.last_child())
        .and_downcast::<gtk::Label>()
    {
        what.set_text(&tr("Favourites"));
    }
    row
}

/// A definition as the box of a field shows it.
fn definition(definition: &FieldDefinition) -> String {
    match definition {
        FieldDefinition::Computed { expression } => expression.clone(),
        FieldDefinition::Tag { key } => fill(&tr("the tag {}"), &[key]),
    }
}

/// The box of a field defined differently here and in the file.
pub(super) fn field_box(conflict: &FieldConflict) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.add_css_class("hk-import-field");
    column.append(&label(&conflict.name, &["hk-import-field-name"]));
    let grid = gtk::Grid::new();
    grid.set_column_spacing(16);
    grid.set_row_spacing(4);
    for (row, (name, text, classes)) in [
        (
            tr("Here"),
            definition(&conflict.ours),
            &["hk-import-field-definition"][..],
        ),
        (
            tr("In the file"),
            definition(&conflict.theirs),
            &["hk-import-field-definition", "hk-t3"][..],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let row = i32::try_from(row).unwrap_or(0);
        grid.attach(&label(&name, &["hk-t2"]), 0, row, 1, 1);
        let value = label(&text, classes);
        value.set_hexpand(true);
        value.set_tooltip_text(Some(&text));
        grid.attach(&value, 1, row, 1, 1);
    }
    column.append(&grid);
    column
}

/// The notice box: a warning icon, the message in 500 and a `t3` line under
/// it, with room for a button at the right. Returns the box, the message, the
/// line and the room.
pub(super) fn notice() -> (gtk::Box, gtk::Label, gtk::Label, gtk::Box) {
    let notice = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    notice.add_css_class("hk-notice");
    let icon = gtk::Image::from_icon_name("helikodu-warning-symbolic");
    icon.set_valign(gtk::Align::Start);
    notice.append(&icon);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let message = label("", &["hk-medium"]);
    message.set_wrap(true);
    message.set_single_line_mode(false);
    message.set_ellipsize(gtk::pango::EllipsizeMode::None);
    text.append(&message);
    let line = label("", &["hk-t3", "hk-secondary"]);
    line.set_wrap(true);
    line.set_single_line_mode(false);
    line.set_ellipsize(gtk::pango::EllipsizeMode::None);
    text.append(&line);
    notice.append(&text);
    let room = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    room.set_valign(gtk::Align::Center);
    notice.append(&room);
    (notice, message, line, room)
}

/// Opens a popover under `anchor` headed `head` that lists `lines`.
pub(super) fn list_popover(anchor: &gtk::Button, head: &str, lines: &[String]) {
    let popover = gtk::Popover::new();
    popover.set_widget_name("import-list-popover");
    popover.add_css_class("menu");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.add_css_class("hk-import-popover-list");
    column.set_margin_start(10);
    column.set_margin_end(10);
    column.set_margin_top(8);
    column.set_margin_bottom(8);
    column.append(&label(head, &["hk-import-popover-head"]));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    for line in lines {
        let entry = label(line, &[]);
        entry.set_selectable(false);
        entry.set_tooltip_text(Some(line));
        list.append(&entry);
    }
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_propagate_natural_height(true);
    scroller.set_max_content_height(320);
    scroller.set_min_content_width(320);
    scroller.set_child(Some(&list));
    column.append(&scroller);
    popover.set_child(Some(&column));
    popover.set_parent(anchor);
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}

/// The head of a list of entries left out: "The first 200 of 880", or the
/// number when all are listed.
pub(super) fn listed_head(listed: usize, all: usize) -> String {
    if all > listed {
        fill(
            &tr("The first {} of {}"),
            &[&format::count(listed), &format::count(all)],
        )
    } else {
        fill(&ntr("{} entry", "{} entries", all), &[&format::count(all)])
    }
}
