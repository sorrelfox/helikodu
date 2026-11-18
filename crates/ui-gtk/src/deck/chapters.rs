// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The deck while a chapter of a file plays: the title carries the chapter's
//! number, and the seek bar and the times cover the whole file with a gap at each
//! chapter boundary. Previous and Next move by chapter, as chapters are queue
//! entries of their own; seeking into another chapter plays that chapter from
//! the point chosen.

use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::Deck;
use crate::backend::{Backend, ChapterRow, PlayerRequest, TrackKey};
use crate::i18n::{fill, tr};
use crate::playback::PlayerView;

/// The chapters of the playing file, and which of them plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChapterSpan {
    chapters: Vec<ChapterRow>,
    index: usize,
}

impl ChapterSpan {
    /// The span of `chapters` while `track` plays; `None` unless `track` is one of
    /// two chapters or more.
    pub(super) fn new(chapters: Vec<ChapterRow>, track: TrackKey) -> Option<Self> {
        let index = chapters.iter().position(|c| c.key == track)?;
        (chapters.len() > 1).then_some(Self { chapters, index })
    }

    /// The playing chapter's track.
    pub(super) fn track(&self) -> TrackKey {
        self.chapters[self.index].key
    }

    /// Where the playing chapter starts in the file.
    pub(super) fn offset(&self) -> Duration {
        self.chapters[self.index].start
    }

    /// The length of the file: the end of its last chapter.
    pub(super) fn length(&self) -> Duration {
        self.chapters
            .last()
            .map(|c| c.start + c.length.unwrap_or_default())
            .unwrap_or_default()
    }

    /// The chapter boundaries as fractions of the file.
    pub(super) fn gaps(&self) -> Vec<f64> {
        let length = self.length().as_secs_f64();
        if length <= 0.0 {
            return Vec::new();
        }
        self.chapters
            .iter()
            .skip(1)
            .map(|c| c.start.as_secs_f64() / length)
            .collect()
    }

    /// The deck's title: "9. The Storm Lantern".
    pub(super) fn title(&self, title: &str) -> String {
        fill(
            &tr("{}. {}"),
            &[&self.chapters[self.index].number.to_string(), title],
        )
    }

    /// The chapter at `at` in the file, and the position within it.
    pub(super) fn locate(&self, at: Duration) -> (&ChapterRow, Duration) {
        let chapter = self
            .chapters
            .iter()
            .rev()
            .find(|c| c.start <= at)
            .unwrap_or(&self.chapters[0]);
        (chapter, at.saturating_sub(chapter.start))
    }
}

impl Deck {
    /// Reads the chapters of the playing file when the track changes.
    pub(super) fn follow_chapters(&self, view: &PlayerView, backend: &Backend) {
        let imp = self.imp();
        let Some(track) = view.now.as_ref().map(|n| n.track.key) else {
            self.set_chapters(None);
            return;
        };
        let known = imp.chapters.borrow().as_ref().map(|c| c.chapters.clone());
        if let Some(chapters) = known
            && let Some(span) = ChapterSpan::new(chapters, track)
        {
            self.set_chapters(Some(span));
            return;
        }
        self.set_chapters(None);
        if track.0 == 0 {
            return;
        }
        let weak = self.downgrade();
        let backend = backend.clone();
        glib::spawn_future_local(async move {
            let chapters = backend
                .query(move |l| l.chapters(track))
                .await
                .unwrap_or_default();
            let Some(deck) = weak.upgrade() else { return };
            let playing = deck.imp().now.borrow().as_ref().map(|n| n.track.key);
            if playing == Some(track) {
                deck.set_chapters(ChapterSpan::new(chapters, track));
            }
        });
    }

    fn set_chapters(&self, span: Option<ChapterSpan>) {
        let imp = self.imp();
        if *imp.chapters.borrow() == span {
            return;
        }
        match &span {
            Some(span) => imp.seek_bar.set_chapters(span.offset(), span.gaps()),
            None => imp.seek_bar.set_chapters(Duration::ZERO, Vec::new()),
        }
        if let (Some(span), Some(now)) = (&span, imp.now.borrow().as_ref()) {
            imp.title.set_text(&span.title(&now.track.title));
        }
        imp.chapters.replace(span);
        if let Some(playback) = imp.playback.borrow().upgrade() {
            self.show_times(&playback.view());
        }
    }

    /// The deck's position and length: in the file while a chapter plays.
    pub(super) fn shown_times(&self, view: &PlayerView) -> (Duration, Option<Duration>) {
        let chapters = self.imp().chapters.borrow();
        match chapters.as_ref() {
            Some(span)
                if view
                    .now
                    .as_ref()
                    .is_some_and(|n| n.track.key == span.track()) =>
            {
                (span.offset() + view.elapsed, Some(span.length()))
            }
            _ => (view.elapsed, view.length),
        }
    }

    /// Seeks to `to` in the bar's positions: within the playing chapter, or into
    /// another chapter of the file, which then plays from there.
    pub(super) fn seek_to(&self, to: Duration, backend: &Backend) {
        let span = self.imp().chapters.borrow().clone();
        let Some(span) = span else {
            backend.send(PlayerRequest::Seek(to));
            return;
        };
        let (chapter, within) = span.locate(to);
        if chapter.key == span.track() {
            backend.send(PlayerRequest::Seek(within));
            return;
        }
        let playback = self.imp().playback.borrow().upgrade();
        let entry = playback.and_then(|p| {
            p.view()
                .queue
                .items
                .iter()
                .find(|row| row.key == chapter.key)
                .and_then(|row| row.entry)
        });
        match entry {
            Some(entry) => {
                backend.send(PlayerRequest::PlayEntry(entry));
                backend.send(PlayerRequest::Seek(within));
            }
            // The chapter is not in the queue: stay in the playing one.
            None => {
                let current = &span.chapters[span.index];
                let end = current.length.unwrap_or_default();
                backend.send(PlayerRequest::Seek(
                    to.saturating_sub(span.offset()).min(end),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapter(n: u32, start: u64, length: u64) -> ChapterRow {
        ChapterRow {
            key: TrackKey(u64::from(n)),
            number: n,
            title: format!("Chapter {n}"),
            start: Duration::from_secs(start),
            length: Some(Duration::from_secs(length)),
        }
    }

    #[test]
    fn a_span_covers_the_file() {
        let chapters = vec![
            chapter(1, 0, 100),
            chapter(2, 100, 300),
            chapter(3, 400, 100),
        ];
        assert!(ChapterSpan::new(chapters[..1].to_vec(), TrackKey(1)).is_none());
        let span = ChapterSpan::new(chapters, TrackKey(2)).unwrap();
        assert_eq!(span.offset(), Duration::from_secs(100));
        assert_eq!(span.length(), Duration::from_secs(500));
        assert_eq!(span.gaps(), vec![0.2, 0.8]);
        assert_eq!(span.title("Night"), "2. Night");
        let (found, within) = span.locate(Duration::from_secs(450));
        assert_eq!((found.number, within), (3, Duration::from_secs(50)));
        let (found, within) = span.locate(Duration::from_secs(100));
        assert_eq!((found.number, within), (2, Duration::ZERO));
    }
}
