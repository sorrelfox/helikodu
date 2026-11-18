// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The play history from the library: the listens with their tracks, the
//! totals, and the export to a file.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use helikodu_service::library::HistoryQuery;

use super::super::{
    BackendError, HistoryRow, HistoryTotals, LibrarySource, PlayedFrom, TrackKey, TrackRow,
};
use super::{ServiceLibrary, error};

/// The most listens the history view shows at once.
const HISTORY_LIMIT: usize = 20_000;

impl ServiceLibrary {
    pub(super) fn history_rows(
        &self,
        since: Option<SystemTime>,
    ) -> Result<Vec<HistoryRow>, BackendError> {
        let music = self.library()?;
        let entries = music
            .history(&HistoryQuery {
                since,
                limit: HISTORY_LIMIT,
                ..HistoryQuery::default()
            })
            .map_err(error)?;
        let mut keys: Vec<TrackKey> = entries
            .iter()
            .filter_map(|e| e.track.map(|t| TrackKey(t.get())))
            .collect();
        keys.sort_unstable_by_key(|k| k.0);
        keys.dedup();
        let rows: HashMap<TrackKey, TrackRow> = self
            .track_rows(&keys)?
            .into_iter()
            .map(|row| (row.key, row))
            .collect();
        let names: HashMap<u64, String> = music
            .playlists()
            .map_err(error)?
            .into_iter()
            .filter_map(|p| Some((u64::try_from(p.id.get()).ok()?, p.name)))
            .collect();
        Ok(entries
            .into_iter()
            .map(|entry| {
                let track = entry
                    .track
                    .and_then(|t| rows.get(&TrackKey(t.get())).cloned())
                    .unwrap_or_else(|| TrackRow {
                        missing: true,
                        ..TrackRow::default()
                    });
                HistoryRow {
                    started_at: entry.started_at,
                    listened: entry.played,
                    counted: entry.counted,
                    from: PlayedFrom::parse(entry.context.as_deref(), |id| names.get(&id).cloned()),
                    track,
                }
            })
            .collect())
    }

    pub(super) fn totals(&self) -> Result<HistoryTotals, BackendError> {
        let totals = self.library()?.history_totals().map_err(error)?;
        let settings = self.service.settings().get();
        let playback = &settings.playback;
        Ok(HistoryTotals {
            plays: totals.plays,
            first: totals.first,
            play_percent: playback.play_count_percent,
            play_minutes: playback.play_count_minutes,
        })
    }

    pub(super) fn export(
        &self,
        path: &Path,
        since: Option<SystemTime>,
    ) -> Result<usize, BackendError> {
        self.library()?.export_history(path, since).map_err(error)
    }
}
