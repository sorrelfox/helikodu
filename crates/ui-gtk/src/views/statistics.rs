// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library statistics: five tiles (tracks, albums, artists, playing time, size),
//! the formats and their quality, listening this year, and the most played
//! artists and albums.
//!
//! The figures are counted in the background each time the view opens; they do
//! not follow the library while it is shown. A row of Formats or Quality opens
//! Tracks with the matching query; a row of the most played opens the artist or
//! the album. "Copy as text" puts the figures on the clipboard.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gtk::glib;
use gtk::prelude::*;

use crate::backend::{AlbumKey, ArtistKey, Backend, LibraryStats, StatisticsSince};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::models::CoverCache;
use crate::views::facet_browser::codec_name;
use crate::widgets::{CoverArt, label};

/// How many formats are listed by name; the rest are "Other".
const FORMAT_ROWS: usize = 6;

/// A length in days and hours (`52 d 7 h`), hours and minutes, or minutes.
pub(crate) fn long_duration(d: Duration) -> String {
    let minutes = d.as_secs() / 60;
    let (days, hours, minutes) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    if days > 0 {
        fill(&tr("{} d {} h"), &[&days.to_string(), &hours.to_string()])
    } else if hours > 0 {
        fill(
            &tr("{} h {} min"),
            &[&hours.to_string(), &minutes.to_string()],
        )
    } else {
        fill(&tr("{} min"), &[&minutes.to_string()])
    }
}

/// A length in minutes and seconds: `4 min 5 s`.
fn short_duration(d: Duration) -> String {
    let seconds = d.as_secs();
    fill(
        &tr("{} min {} s"),
        &[&(seconds / 60).to_string(), &(seconds % 60).to_string()],
    )
}

/// A share in whole percent, at least 1 for a part that is not nothing.
fn percent(part: usize, whole: usize) -> u64 {
    if whole == 0 || part == 0 {
        return 0;
    }
    ((part as f64 * 100.0 / whole as f64).round() as u64).max(1)
}

/// A size in binary units: `612.3 GiB`.
fn binary_size(bytes: u64) -> String {
    glib::format_size_full(bytes, glib::FormatSizeFlags::IEC_UNITS).to_string()
}

/// A local time as seconds since the epoch.
fn system_time(date: &glib::DateTime) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(u64::try_from(date.to_unix()).unwrap_or(0))
}

/// The start of this month and of this year, in local time.
pub(crate) fn since_now() -> StatisticsSince {
    let now = glib::DateTime::now_local().ok();
    let start = |month: Option<i32>| {
        now.as_ref()
            .and_then(|now| {
                glib::DateTime::from_local(now.year(), month.unwrap_or(now.month()), 1, 0, 0, 0.0)
                    .ok()
            })
            .map_or(UNIX_EPOCH, |d| system_time(&d))
    };
    StatisticsSince {
        month: start(None),
        year: start(Some(1)),
    }
}

/// One format row: its name, number of tracks, and the query that lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FormatRow {
    pub(crate) name: String,
    pub(crate) tracks: usize,
    pub(crate) query: String,
}

/// The rows of Formats: codecs under one name together (DSD, PCM), the largest
/// first, the rest as "Other".
pub(crate) fn format_rows(formats: &[(Option<String>, usize)]) -> Vec<FormatRow> {
    let mut named: Vec<(String, usize, Vec<String>)> = Vec::new();
    let mut unknown = 0;
    for (codec, tracks) in formats {
        let Some(codec) = codec else {
            unknown += tracks;
            continue;
        };
        let name = codec_name(codec);
        match named.iter_mut().find(|(n, _, _)| *n == name) {
            Some((_, count, codecs)) => {
                *count += tracks;
                codecs.push(codec.clone());
            }
            None => named.push((name, *tracks, vec![codec.clone()])),
        }
    }
    named.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let term = |codec: &str| {
        let lower = codec.to_ascii_lowercase();
        if lower.starts_with("dsd") {
            "codec:dsd".to_owned()
        } else if lower.starts_with("pcm") {
            "codec:pcm".to_owned()
        } else {
            format!("codec={lower}")
        }
    };
    let mut rows: Vec<FormatRow> = Vec::new();
    let mut listed: Vec<String> = Vec::new();
    for (name, tracks, codecs) in named.iter().take(FORMAT_ROWS) {
        let mut terms: Vec<String> = codecs.iter().map(|c| term(c)).collect();
        terms.dedup();
        listed.extend(terms.iter().cloned());
        rows.push(FormatRow {
            name: name.clone(),
            tracks: *tracks,
            query: terms.join(" OR "),
        });
    }
    let other: usize = named.iter().skip(FORMAT_ROWS).map(|n| n.1).sum::<usize>() + unknown;
    if other > 0 {
        rows.push(FormatRow {
            name: tr("Other"),
            tracks: other,
            query: listed
                .iter()
                .map(|t| format!("-{t}"))
                .collect::<Vec<_>>()
                .join(" "),
        });
    }
    rows
}

/// The figures as plain text, for the clipboard.
pub(crate) fn as_text(stats: &LibraryStats) -> String {
    let mut out = vec![
        tr("Library statistics"),
        String::new(),
        fill(&tr("Tracks: {}"), &[&format::count(stats.tracks)]),
    ];
    out.push(fill(&tr("Albums: {}"), &[&format::count(stats.albums)]));
    out.push(fill(&tr("Artists: {}"), &[&format::count(stats.artists)]));
    out.push(fill(
        &tr("Composers: {}"),
        &[&format::count(stats.composers)],
    ));
    out.push(fill(
        &tr("Playing time: {}"),
        &[&long_duration(stats.duration)],
    ));
    out.push(fill(&tr("Size: {}"), &[&binary_size(stats.size)]));
    out.push(String::new());
    out.push(tr("Formats"));
    for row in format_rows(&stats.formats) {
        out.push(format!(
            "  {}: {} ({} %)",
            row.name,
            format::count(row.tracks),
            percent(row.tracks, stats.tracks)
        ));
    }
    out.push(String::new());
    out.push(tr("Quality"));
    for (name, tracks) in quality_rows(stats) {
        out.push(format!(
            "  {name}: {} ({} %)",
            format::count(tracks),
            percent(tracks, stats.tracks)
        ));
    }
    out.push(String::new());
    out.push(tr("Listening this year"));
    out.push(fill(&tr("  Plays: {}"), &[&format::count(stats.plays)]));
    out.push(fill(
        &tr("  Listened: {}"),
        &[&long_duration(stats.listened)],
    ));
    out.push(fill(&tr("  Skips: {}"), &[&format::count(stats.skips)]));
    out.push(fill(
        &tr("  Never played: {} %"),
        &[&percent(stats.never_played, stats.tracks).to_string()],
    ));
    out.push(String::new());
    out.push(tr("Most played artists"));
    for (i, artist) in stats.top_artists.iter().enumerate() {
        out.push(format!("  {}. {} ({})", i + 1, artist.name, artist.plays));
    }
    out.push(String::new());
    out.push(tr("Most played albums"));
    for (i, album) in stats.top_albums.iter().enumerate() {
        out.push(format!(
            "  {}. {} · {} ({})",
            i + 1,
            album.title,
            album.artist,
            album.plays
        ));
    }
    out.join("\n") + "\n"
}

fn quality_rows(stats: &LibraryStats) -> [(String, usize); 3] {
    [
        (tr("Lossless, CD quality"), stats.lossless_cd),
        (tr("Lossless, hi-res"), stats.lossless_hi_res),
        (tr("Lossy"), stats.lossy),
    ]
}

/// The queries the Quality rows open.
const QUALITY_QUERIES: [&str; 3] = ["is:lossless -is:hires", "is:hires", "-is:lossless"];

type OpenText = Box<dyn Fn(&str)>;
type OpenArtist = Box<dyn Fn(ArtistKey)>;
type OpenAlbum = Box<dyn Fn(AlbumKey)>;

/// Where the rows lead.
pub(crate) struct Opens {
    /// Tracks with a query.
    pub(crate) tracks: OpenText,
    /// An artist.
    pub(crate) artist: OpenArtist,
    /// An album.
    pub(crate) album: OpenAlbum,
}

/// The view.
pub(crate) struct StatisticsView {
    pub(crate) root: gtk::Box,
    counted: gtk::Label,
    content: gtk::Box,
    backend: Backend,
    covers: Rc<CoverCache>,
    opens: Opens,
    stats: RefCell<Option<LibraryStats>>,
    generation: Cell<u64>,
}

impl std::fmt::Debug for StatisticsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatisticsView").finish_non_exhaustive()
    }
}

impl StatisticsView {
    pub(crate) fn new(backend: &Backend, covers: &Rc<CoverCache>, opens: Opens) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("hk-statistics-view");
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        toolbar.add_css_class("hk-toolbar");
        let title = label(&tr("Library statistics"), &["hk-view-title"]);
        title.set_hexpand(false);
        title.set_accessible_role(gtk::AccessibleRole::Heading);
        toolbar.append(&title);
        let counted = label("", &["hk-t3"]);
        counted.set_valign(gtk::Align::Baseline);
        counted.set_hexpand(true);
        toolbar.append(&counted);
        let copy = gtk::Button::with_label(&tr("Copy as text"));
        copy.add_css_class("hk-btn");
        copy.add_css_class("small");
        copy.set_valign(gtk::Align::Center);
        toolbar.append(&copy);
        root.append(&toolbar);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
        content.add_css_class("hk-statistics");
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&content));
        root.append(&scroller);
        let view = Rc::new(Self {
            root,
            counted,
            content,
            backend: backend.clone(),
            covers: covers.clone(),
            opens,
            stats: RefCell::new(None),
            generation: Cell::new(0),
        });
        let weak = Rc::downgrade(&view);
        copy.connect_clicked(move |button| {
            let Some(view) = weak.upgrade() else { return };
            if let Some(stats) = view.stats.borrow().as_ref() {
                button.clipboard().set_text(&as_text(stats));
            }
        });
        view
    }

    /// Counts the figures again and shows them when they are ready.
    pub(crate) fn show(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        if self.stats.borrow().is_none() {
            self.counted.set_text(&tr("counting…"));
        }
        let since = since_now();
        let weak = Rc::downgrade(self);
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let result = backend
                .query(move |library| library.statistics(since))
                .await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            match result {
                Ok(stats) => {
                    let time = glib::DateTime::now_local()
                        .and_then(|d| d.format("%H:%M"))
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    view.counted
                        .set_text(&fill(&tr("counted today at {}"), &[&time]));
                    view.fill(&stats);
                    view.stats.replace(Some(stats));
                }
                Err(error) => view.counted.set_text(&error.0),
            }
        });
    }

    /// The figures shown (for tests).
    pub(crate) fn stats(&self) -> Option<LibraryStats> {
        self.stats.borrow().clone()
    }

    /// The texts of the view, in order (for tests).
    pub(crate) fn texts(&self) -> Vec<String> {
        let mut out = vec![self.counted.text().to_string()];
        collect_labels(self.content.upcast_ref(), &mut out);
        out
    }

    pub(crate) fn focus_target(&self) -> gtk::Widget {
        self.content.clone().upcast()
    }

    fn fill(self: &Rc<Self>, stats: &LibraryStats) {
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
        self.content.append(&tiles(stats));
        let middle = columns();
        middle.append(&self.formats(stats));
        let right = gtk::Box::new(gtk::Orientation::Vertical, 24);
        right.append(&self.quality(stats));
        right.append(&listening(stats));
        middle.append(&right);
        self.content.append(&middle);
        let bottom = columns();
        bottom.append(&self.top_artists(stats));
        bottom.append(&self.top_albums(stats));
        self.content.append(&bottom);
    }

    fn formats(self: &Rc<Self>, stats: &LibraryStats) -> gtk::Box {
        let section = section(&tr("Formats"), &tr("tracks"));
        let rows = format_rows(&stats.formats);
        let largest = rows.iter().map(|r| r.tracks).max().unwrap_or(0);
        for row in rows {
            let button = self.bar_row(&row.name, row.tracks, largest, stats.tracks);
            let weak = Rc::downgrade(self);
            let query = row.query.clone();
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    (view.opens.tracks)(&query);
                }
            });
            section.append(&button);
        }
        section
    }

    fn quality(self: &Rc<Self>, stats: &LibraryStats) -> gtk::Box {
        let section = section(&tr("Quality"), &tr("tracks"));
        let rows = quality_rows(stats);
        let largest = rows.iter().map(|r| r.1).max().unwrap_or(0);
        for ((name, tracks), query) in rows.into_iter().zip(QUALITY_QUERIES) {
            let button = self.bar_row(&name, tracks, largest, stats.tracks);
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    (view.opens.tracks)(query);
                }
            });
            section.append(&button);
        }
        section
    }

    /// A row of Formats or Quality: the name, a bar against the largest row, the
    /// count and the share.
    fn bar_row(&self, name: &str, tracks: usize, largest: usize, total: usize) -> gtk::Button {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let title = label(name, &[]);
        title.set_size_request(150, -1);
        title.set_hexpand(false);
        row.append(&title);
        let bar = gtk::LevelBar::for_interval(0.0, largest.max(1) as f64);
        bar.add_css_class("hk-bar");
        bar.set_value(tracks as f64);
        bar.set_hexpand(true);
        bar.set_valign(gtk::Align::Center);
        bar.set_can_target(false);
        row.append(&bar);
        let count = label(&format::count(tracks), &["hk-numeric"]);
        count.set_xalign(1.0);
        count.set_size_request(56, -1);
        row.append(&count);
        let share = label(
            &format!("{} %", percent(tracks, total)),
            &["hk-t3", "hk-numeric"],
        );
        share.set_xalign(1.0);
        share.set_size_request(40, -1);
        row.append(&share);
        let button = gtk::Button::new();
        button.add_css_class("hk-stat-row");
        button.set_child(Some(&row));
        button.set_tooltip_text(Some(&fill(&tr("Show the {} tracks"), &[name])));
        button.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{}: {} tracks"),
            &[name, &format::count(tracks)],
        ))]);
        button
    }

    fn top_artists(self: &Rc<Self>, stats: &LibraryStats) -> gtk::Box {
        let section = section(&tr("Most played artists"), &tr("plays"));
        let most = stats.top_artists.first().map_or(0, |a| a.plays);
        for (rank, artist) in stats.top_artists.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let number = label(&(rank + 1).to_string(), &["hk-t3", "hk-numeric"]);
            number.set_xalign(1.0);
            number.set_size_request(16, -1);
            row.append(&number);
            let name = label(&artist.name, &[]);
            name.set_hexpand(true);
            row.append(&name);
            let bar = gtk::LevelBar::for_interval(0.0, most.max(1) as f64);
            bar.add_css_class("hk-bar");
            bar.add_css_class("plays");
            bar.set_value(artist.plays as f64);
            bar.set_size_request(140, -1);
            bar.set_valign(gtk::Align::Center);
            bar.set_can_target(false);
            row.append(&bar);
            row.append(&plays_label(artist.plays));
            let button = gtk::Button::new();
            button.add_css_class("hk-stat-row");
            button.add_css_class("plays");
            button.set_child(Some(&row));
            button.update_property(&[gtk::accessible::Property::Label(&fill(
                &ntr("{}: {} play", "{}: {} plays", artist.plays as usize),
                &[&artist.name, &format::count(artist.plays)],
            ))]);
            let weak = Rc::downgrade(self);
            let key = artist.key;
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    (view.opens.artist)(key);
                }
            });
            section.append(&button);
        }
        section
    }

    fn top_albums(self: &Rc<Self>, stats: &LibraryStats) -> gtk::Box {
        let section = section(&tr("Most played albums"), &tr("plays"));
        for album in &stats.top_albums {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let cover = CoverArt::new(22, 3.0);
            cover.set_valign(gtk::Align::Center);
            self.covers.show(&cover, album.cover, album.cover_seed, 22);
            row.append(&cover);
            let name = gtk::Label::new(None);
            name.set_markup(&format!(
                "{} <span alpha=\"60%\">· {}</span>",
                glib::markup_escape_text(&album.title),
                glib::markup_escape_text(&album.artist)
            ));
            name.set_xalign(0.0);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            name.set_hexpand(true);
            row.append(&name);
            row.append(&plays_label(album.plays));
            let button = gtk::Button::new();
            button.add_css_class("hk-stat-row");
            button.add_css_class("plays");
            button.set_child(Some(&row));
            button.update_property(&[gtk::accessible::Property::Label(&fill(
                &ntr(
                    "{} by {}: {} play",
                    "{} by {}: {} plays",
                    album.plays as usize,
                ),
                &[&album.title, &album.artist, &format::count(album.plays)],
            ))]);
            let weak = Rc::downgrade(self);
            let key = album.key;
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    (view.opens.album)(key);
                }
            });
            section.append(&button);
        }
        section
    }
}

fn plays_label(plays: u64) -> gtk::Label {
    let plays = label(&format::count(plays), &["hk-numeric"]);
    plays.set_xalign(1.0);
    plays.set_size_request(48, -1);
    plays
}

/// Two columns with 48 px between them.
fn columns() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 48);
    row.set_homogeneous(true);
    row
}

/// A section with its head: the title and a caption at the right.
fn section(title: &str, caption: &str) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("hk-stat-head");
    let heading = label(title, &["hk-stat-title"]);
    heading.set_hexpand(true);
    heading.set_accessible_role(gtk::AccessibleRole::Heading);
    head.append(&heading);
    head.append(&label(caption, &["hk-stat-caption"]));
    section.append(&head);
    section
}

/// The five tiles.
fn tiles(stats: &LibraryStats) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_homogeneous(true);
    let average = if stats.tracks > 0 {
        stats.duration / u32::try_from(stats.tracks).unwrap_or(u32::MAX)
    } else {
        Duration::ZERO
    };
    let folders = fill(
        &ntr("in {} folder", "in {} folders", stats.folders),
        &[&format::count(stats.folders)],
    );
    let drives = fill(
        &ntr("{} drive", "{} drives", stats.drives),
        &[&format::count(stats.drives)],
    );
    for (title, figure, caption) in [
        (
            tr("Tracks"),
            format::count(stats.tracks),
            fill(
                &ntr(
                    "{} added this month",
                    "{} added this month",
                    stats.added_this_month,
                ),
                &[&format::count(stats.added_this_month)],
            ),
        ),
        (
            tr("Albums"),
            format::count(stats.albums),
            fill(
                &ntr(
                    "{} without a cover",
                    "{} without a cover",
                    stats.albums_without_cover,
                ),
                &[&format::count(stats.albums_without_cover)],
            ),
        ),
        (
            tr("Artists"),
            format::count(stats.artists),
            fill(
                &ntr("{} composer", "{} composers", stats.composers),
                &[&format::count(stats.composers)],
            ),
        ),
        (
            tr("Playing time"),
            long_duration(stats.duration),
            fill(&tr("{} a track on average"), &[&short_duration(average)]),
        ),
        (
            tr("Size"),
            binary_size(stats.size),
            fill(&tr("{} and {}"), &[&folders, &drives]),
        ),
    ] {
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 2);
        tile.add_css_class("hk-stat");
        tile.append(&label(&title, &["hk-section-label"]));
        tile.append(&label(&figure, &["figure", "hk-numeric"]));
        tile.append(&label(&caption, &["caption"]));
        tile.set_accessible_role(gtk::AccessibleRole::Group);
        tile.update_property(&[gtk::accessible::Property::Label(&format!(
            "{title}: {figure}, {caption}"
        ))]);
        row.append(&tile);
    }
    row
}

/// Listening this year: four figures.
fn listening(stats: &LibraryStats) -> gtk::Box {
    let section = section(&tr("Listening"), &tr("this year"));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_homogeneous(true);
    row.set_margin_top(10);
    for (figure, caption) in [
        (format::count(stats.plays), tr("plays")),
        (long_duration(stats.listened), tr("listened")),
        (format::count(stats.skips), tr("skips")),
        (
            format!("{} %", percent(stats.never_played, stats.tracks)),
            tr("never played"),
        ),
    ] {
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 0);
        cell.append(&label(&figure, &["hk-stat-figure", "hk-numeric"]));
        cell.append(&label(&caption, &["hk-stat-caption"]));
        row.append(&cell);
    }
    section.append(&row);
    section
}

fn collect_labels(widget: &gtk::Widget, out: &mut Vec<String>) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        out.push(label.text().to_string());
        return;
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        collect_labels(&c, out);
        child = c.next_sibling();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert_eq!(
            long_duration(Duration::from_secs(52 * 86_400 + 7 * 3600 + 59)),
            "52 d 7 h"
        );
        assert_eq!(
            long_duration(Duration::from_secs(3 * 3600 + 120)),
            "3 h 2 min"
        );
        assert_eq!(short_duration(Duration::from_secs(245)), "4 min 5 s");
    }

    #[test]
    fn formats_merge_and_end_with_other() {
        let formats = vec![
            (Some("flac".to_owned()), 50),
            (Some("mp3".to_owned()), 20),
            (Some("dsd64".to_owned()), 3),
            (Some("dsd128".to_owned()), 2),
            (Some("opus".to_owned()), 10),
            (Some("aac".to_owned()), 6),
            (Some("alac".to_owned()), 4),
            (Some("wma".to_owned()), 1),
            (None, 1),
        ];
        let rows = format_rows(&formats);
        let names: Vec<(&str, usize)> = rows.iter().map(|r| (r.name.as_str(), r.tracks)).collect();
        assert_eq!(
            names,
            [
                ("FLAC", 50),
                ("MP3", 20),
                ("Opus", 10),
                ("AAC", 6),
                ("DSD", 5),
                ("ALAC", 4),
                ("Other", 2)
            ]
        );
        assert_eq!(rows[0].query, "codec=flac");
        assert_eq!(rows[4].query, "codec:dsd");
        assert!(rows[6].query.contains("-codec=flac"));
        assert_eq!(percent(1, 1000), 1);
        assert_eq!(percent(0, 1000), 0);
    }
}
