// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! An invented play history: listening sessions going back six weeks from
//! now, each from one place, with a skip now and then.

use std::path::Path;
use std::time::{Duration, SystemTime};

use helikodu_service::library::{ExportedListen, HistoryFormat, write_history};

use super::super::{BackendError, HistoryRow, HistoryTotals, PlayedFrom, TrackKey};
use super::{Random, SampleLibrary};

/// How far back the invented history goes.
const SPAN: Duration = Duration::from_secs(42 * 24 * 3600);

impl SampleLibrary {
    /// Every invented listen, newest first.
    fn invented_history(&self) -> Vec<HistoryRow> {
        let count = self.tracks.len() as u64;
        if count == 0 {
            return Vec::new();
        }
        let now = SystemTime::now();
        let mut r = Random(48);
        let mut at = now - Duration::from_secs(240);
        let mut rows = Vec::new();
        let places = [
            PlayedFrom::Album,
            PlayedFrom::Playlist("Late evening".into()),
            PlayedFrom::Search("slow".into()),
            PlayedFrom::Queue,
            PlayedFrom::Folder("Incoming".into()),
            PlayedFrom::Playlist("Favourites".into()),
            PlayedFrom::Album,
        ];
        while now.duration_since(at).unwrap_or_default() < SPAN {
            let from = places[r.below(places.len())].clone();
            let mut key = 1 + r.next() % count;
            for _ in 0..2 + r.below(7) {
                let Some(track) = self.track(TrackKey(key)) else {
                    break;
                };
                let length = track.duration.unwrap_or(Duration::from_secs(200));
                let skipped = r.below(9) == 0;
                let listened = if skipped {
                    Duration::from_secs(15 + r.below(110) as u64).min(length)
                } else {
                    length
                };
                rows.push(HistoryRow {
                    started_at: at,
                    listened,
                    counted: !skipped,
                    from: from.clone(),
                    track,
                });
                at -= listened + Duration::from_secs(r.below(20) as u64);
                key = if from == PlayedFrom::Album {
                    (key % count) + 1
                } else {
                    1 + r.next() % count
                };
            }
            at -= Duration::from_secs(3600 + r.below(16 * 3600) as u64);
        }
        rows
    }

    pub(super) fn sample_history(&self, since: Option<SystemTime>) -> Vec<HistoryRow> {
        let mut rows = self.invented_history();
        if let Some(since) = since {
            rows.retain(|row| row.started_at >= since);
        }
        rows
    }

    pub(super) fn sample_history_totals(&self) -> HistoryTotals {
        let rows = self.invented_history();
        HistoryTotals {
            plays: rows.iter().filter(|r| r.counted).count() as u64,
            first: rows.last().map(|r| r.started_at),
            ..HistoryTotals::default()
        }
    }

    pub(super) fn sample_export_history(
        &self,
        path: &Path,
        since: Option<SystemTime>,
    ) -> Result<usize, BackendError> {
        let listens: Vec<ExportedListen> = self
            .sample_history(since)
            .into_iter()
            .rev()
            .map(|row| ExportedListen {
                started_at: row.started_at,
                counted: row.counted,
                artist: row.track.artist,
                title: row.track.title,
                album: Some(row.track.album_title),
                track_number: row.track.number,
                length: row.track.duration,
            })
            .collect();
        let mut out = Vec::new();
        let written = write_history(HistoryFormat::for_path(path), &listens, &mut out)
            .map_err(|e| BackendError(e.to_string()))?;
        std::fs::write(path, out).map_err(|e| BackendError(e.to_string()))?;
        Ok(written)
    }
}
