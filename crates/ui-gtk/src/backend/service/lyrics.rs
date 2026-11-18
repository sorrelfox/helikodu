// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The lyrics service's events as the window's updates.

use std::sync::Arc;

use helikodu_service::Service;
use helikodu_service::lyrics::{Lyrics, LyricsEvent, LyricsSource, LyricsState, SidecarKind};

use super::super::{LyricRow, LyricsOrigin, LyricsView, Update};

/// The window's form of `lyrics` with the offset `offset_ms`.
pub(super) fn view(lyrics: &Lyrics, offset_ms: i64) -> LyricsView {
    let origin = match &lyrics.source {
        LyricsSource::Sidecar {
            kind: SidecarKind::Lrc,
            ..
        } => LyricsOrigin::LrcFile,
        LyricsSource::Sidecar { .. } => LyricsOrigin::TextFile,
        _ => LyricsOrigin::Tags,
    };
    LyricsView {
        lines: lyrics
            .lines
            .iter()
            .map(|l| LyricRow {
                time: l.time,
                text: l.text.clone(),
                verse_start: l.verse_start,
            })
            .collect(),
        synced: lyrics.synced,
        origin,
        file: lyrics.source.path().map(ToOwned::to_owned),
        offset_ms,
    }
}

/// The updates for a lyrics event.
pub(super) fn updates(event: &LyricsEvent) -> Vec<Update> {
    match event {
        LyricsEvent::Changed {
            lyrics, offset_ms, ..
        } => vec![Update::Lyrics(
            lyrics.as_deref().map(|l| Arc::new(view(l, *offset_ms))),
        )],
        LyricsEvent::Line { index, .. } => vec![Update::LyricsLine(*index)],
        _ => Vec::new(),
    }
}

/// The updates that bring a new window up to date.
pub(super) fn initial(service: &Service) -> Vec<Update> {
    let Some(lyrics) = service.lyrics() else {
        return Vec::new();
    };
    let LyricsState {
        lyrics,
        offset_ms,
        line,
        ..
    } = lyrics.current();
    vec![
        Update::Lyrics(lyrics.as_deref().map(|l| Arc::new(view(l, offset_ms)))),
        Update::LyricsLine(line),
    ]
}
