// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The album page of an audiobook: where listening continues, which chapters have
//! been heard, the chapter rows and the sentence on what the media kind changes.

use std::time::Duration;

use gtk::prelude::*;

use crate::backend::{AlbumDetail, MediaKind, TrackRow};
use crate::format;
use crate::i18n::{fill, tr};
use crate::widgets::{PlayingIndicator, TitleLine, label};

/// The kinds of the Kind dropdown, in its order.
pub(super) const KINDS: [MediaKind; 3] = [MediaKind::Audiobook, MediaKind::Music, MediaKind::Video];

/// The names of [`KINDS`].
pub(super) fn kind_names() -> Vec<String> {
    vec![tr("Audiobook"), tr("Music"), tr("Audio from a video")]
}

/// One sentence on what `kind` changes.
pub(super) fn kind_text(kind: MediaKind) -> String {
    match kind {
        MediaKind::Audiobook => {
            tr("Audiobooks continue where you stopped and are left out of shuffle.")
        }
        MediaKind::Music => tr("Music plays from the start and takes part in shuffle."),
        MediaKind::Video => tr("Audio from a video file plays like music."),
    }
}

/// Where an audiobook stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Progress {
    /// The chapter "Play" continues with.
    pub(super) continue_at: usize,
    /// Whether each chapter has been heard.
    pub(super) heard: Vec<bool>,
    /// Where each chapter starts in the book.
    pub(super) starts: Vec<Duration>,
    /// How much has been heard.
    pub(super) heard_time: Duration,
    /// The length of the book.
    pub(super) total: Duration,
}

impl Progress {
    /// Where the book of `detail` stands: it continues with the chapter where
    /// listening stopped, or else after the last chapter played. The chapters
    /// before that one have been heard; a book whose last chapter was played to
    /// the end has been heard completely and starts again.
    pub(super) fn of(detail: &AlbumDetail) -> Self {
        let tracks: Vec<&TrackRow> = detail.tracks().collect();
        let mut starts = Vec::with_capacity(tracks.len());
        let mut at = Duration::ZERO;
        for track in &tracks {
            starts.push(at);
            at += track.duration.unwrap_or_default();
        }
        let total = at;
        let resume = detail.resume.and_then(|(key, position)| {
            Some((tracks.iter().position(|t| t.key == key)?, position))
        });
        let last_played = tracks.iter().rposition(|t| t.plays > 0);
        let (continue_at, finished) = match (resume, last_played) {
            (Some((index, _)), _) => (index, false),
            (None, Some(last)) if last + 1 < tracks.len() => (last + 1, false),
            (None, Some(_)) => (0, true),
            (None, None) => (0, false),
        };
        let heard: Vec<bool> = (0..tracks.len())
            .map(|i| finished || i < continue_at)
            .collect();
        let mut heard_time: Duration = tracks
            .iter()
            .zip(&heard)
            .filter(|(_, heard)| **heard)
            .filter_map(|(t, _)| t.duration)
            .sum();
        if let Some((_, position)) = resume {
            heard_time += position;
        }
        Self {
            continue_at,
            heard,
            starts,
            heard_time: heard_time.min(total),
            total,
        }
    }

    /// The share heard, from 0 to 1.
    pub(super) fn fraction(&self) -> f64 {
        if self.total.is_zero() {
            0.0
        } else {
            self.heard_time.as_secs_f64() / self.total.as_secs_f64()
        }
    }

    /// "36 % heard · 7 h 29 min left".
    pub(super) fn text(&self) -> String {
        let percent = (self.fraction() * 100.0).floor();
        fill(
            &tr("{} % heard · {} left"),
            &[
                &format!("{percent:.0}"),
                &format::minutes(self.total.saturating_sub(self.heard_time)),
            ],
        )
    }
}

/// The container badge of a file: its extension in capitals, such as `M4B`.
pub(super) fn container_badge(detail: &AlbumDetail) -> Option<String> {
    let path = detail.tracks().find_map(|t| t.path.clone())?;
    Some(path.extension()?.to_string_lossy().to_uppercase())
}

/// A row of the chapter table: #, Chapter, Starts at (`h:mm:ss`), Length.
pub(super) fn chapter_row(
    index: usize,
    track: &TrackRow,
    start: Duration,
    heard: bool,
    playing: bool,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_widget_name(&index.to_string());
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let number = index + 1;
    if playing {
        row.add_css_class("playing");
        row.add_css_class("hk-playing");
        let indicator = PlayingIndicator::default();
        indicator.set_size_request(24, -1);
        indicator.set_halign(gtk::Align::End);
        line.append(&indicator);
    } else if heard {
        row.add_css_class("hk-heard-row");
        let check = gtk::Image::from_icon_name("helikodu-check-symbolic");
        check.set_size_request(24, -1);
        check.set_halign(gtk::Align::End);
        check.add_css_class("hk-t3");
        line.append(&check);
    } else {
        let label = label(&number.to_string(), &["hk-t3", "hk-numeric"]);
        label.set_xalign(1.0);
        label.set_size_request(24, -1);
        line.append(&label);
    }
    let classes: &[&str] = if playing {
        &["hk-playing"]
    } else if heard {
        &["hk-t2"]
    } else {
        &[]
    };
    let title = TitleLine::new(&track.title, classes);
    line.append(&title);
    let starts = label(
        &format::duration_hours(start),
        &["hk-mono", "hk-t3", "hk-starts"],
    );
    starts.set_xalign(1.0);
    starts.set_size_request(70, -1);
    line.append(&starts);
    let length = label(
        &track.duration.map(format::duration).unwrap_or_default(),
        &["hk-t2", "hk-numeric"],
    );
    length.set_xalign(1.0);
    length.set_size_request(56, -1);
    line.append(&length);
    row.set_child(Some(&line));
    let mut name = fill(&tr("Chapter {}: {}"), &[&number.to_string(), &track.title]);
    if playing {
        name = fill(&tr("{}, playing"), &[&name]);
    } else if heard {
        name = fill(&tr("{}, heard"), &[&name]);
    }
    row.update_property(&[gtk::accessible::Property::Label(&name)]);
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{AlbumRow, DiscGroup, TrackKey};

    fn book(plays: &[u32], resume: Option<(u64, u64)>) -> AlbumDetail {
        let tracks = plays
            .iter()
            .enumerate()
            .map(|(i, plays)| TrackRow {
                key: TrackKey(i as u64 + 1),
                duration: Some(Duration::from_secs(600)),
                plays: *plays,
                ..TrackRow::default()
            })
            .collect();
        AlbumDetail {
            row: AlbumRow {
                key: crate::backend::AlbumKey(1),
                title: String::new(),
                artist: String::new(),
                year: None,
                format: crate::backend::AudioFormat::default(),
                track_count: plays.len(),
                cover_seed: 0,
                cover: None,
            },
            discs: vec![DiscGroup {
                number: None,
                subtitle: None,
                tracks,
            }],
            genre: None,
            added: None,
            size: None,
            folder: None,
            album_gain_db: None,
            kind: MediaKind::Audiobook,
            resume: resume.map(|(key, secs)| (TrackKey(key), Duration::from_secs(secs))),
            reader: None,
        }
    }

    #[test]
    fn a_book_continues_where_listening_stopped() {
        let progress = Progress::of(&book(&[1, 1, 0, 0], Some((3, 300))));
        assert_eq!(progress.continue_at, 2);
        assert_eq!(progress.heard, [true, true, false, false]);
        assert_eq!(progress.heard_time, Duration::from_secs(1500));
        assert_eq!(progress.starts[2], Duration::from_secs(1200));
        assert_eq!(progress.text(), "62 % heard · 15 min left");
    }

    #[test]
    fn a_book_continues_after_the_last_chapter_played() {
        let progress = Progress::of(&book(&[1, 0, 0], None));
        assert_eq!(progress.continue_at, 1);
        let fresh = Progress::of(&book(&[0, 0], None));
        assert_eq!((fresh.continue_at, fresh.heard_time), (0, Duration::ZERO));
        let finished = Progress::of(&book(&[1, 1], None));
        assert_eq!(finished.continue_at, 0);
        assert_eq!(finished.heard, [true, true]);
    }
}
