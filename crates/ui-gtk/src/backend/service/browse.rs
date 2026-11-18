// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Composers, years, media kinds, resume positions and chapters over the
//! service's library.

use std::path::Path;
use std::time::Duration;

use helikodu_model::library::TrackId;
use helikodu_service::library::{
    ArtistQuery, ArtistRole, MAX_PAGE, MusicLibrary, Page, TrackFilter, TrackKind, TrackSummary,
    YearSpan, read_file_info,
};

use super::super::{ArtistRow, BackendError, ChapterRow, MediaKind, TrackKey, YearRange, YearRow};
use super::{artist_row, error, person};
use crate::i18n::tr;

/// The library's span of `range`.
pub(super) fn year_span(range: YearRange) -> YearSpan {
    match range {
        YearRange::Span { first, last } => YearSpan::Years { first, last },
        YearRange::Unknown => YearSpan::Unknown,
    }
}

/// The library's kind of `kind`.
fn track_kind(kind: MediaKind) -> TrackKind {
    match kind {
        MediaKind::Music => TrackKind::Music,
        MediaKind::Audiobook => TrackKind::Audiobook,
        MediaKind::Video => TrackKind::AudioFromVideo,
    }
}

/// The kind of an album from its tracks: audiobook or video when all are,
/// music otherwise.
pub(super) fn album_kind<'a>(tracks: impl IntoIterator<Item = &'a TrackSummary>) -> MediaKind {
    let mut kinds = tracks.into_iter().map(|t| t.kind);
    let Some(first) = kinds.next() else {
        return MediaKind::Music;
    };
    if kinds.any(|k| k != first) {
        return MediaKind::Music;
    }
    match first {
        TrackKind::Audiobook => MediaKind::Audiobook,
        TrackKind::AudioFromVideo => MediaKind::Video,
        _ => MediaKind::Music,
    }
}

/// Every person in the composer role.
pub(super) fn composers(music: &MusicLibrary) -> Result<Vec<ArtistRow>, BackendError> {
    let mut rows = Vec::new();
    loop {
        let mut query = ArtistQuery::new(ArtistRole::Composer);
        query.page = Page::new(rows.len(), MAX_PAGE);
        let page = music.artists(&query).map_err(error)?;
        let got = page.items.len();
        rows.extend(page.items.iter().map(artist_row));
        if got == 0 || rows.len() >= page.total {
            return Ok(rows);
        }
    }
}

/// The number of composers.
pub(super) fn composer_count(music: &MusicLibrary) -> usize {
    let mut query = ArtistQuery::new(ArtistRole::Composer);
    query.page = Page::first(1);
    music.artists(&query).map_or(0, |page| page.total)
}

/// The release years.
pub(super) fn years(music: &MusicLibrary) -> Result<Vec<YearRow>, BackendError> {
    Ok(music
        .years()
        .map_err(error)?
        .into_iter()
        .map(|y| YearRow {
            year: y.year,
            album_count: y.album_count,
            track_count: y.track_count,
        })
        .collect())
}

/// The length of the tracks of `composer`.
pub(super) fn composer_duration(
    music: &MusicLibrary,
    composer: super::super::ArtistKey,
) -> Result<Duration, BackendError> {
    Ok(music
        .all_tracks(TrackFilter::Artist(
            person(composer),
            Some(ArtistRole::Composer),
        ))
        .map_err(error)?
        .iter()
        .filter_map(|t| t.duration)
        .sum())
}

/// Gives `tracks` the kind `kind`.
pub(super) fn set_kind(
    music: &MusicLibrary,
    tracks: &[TrackId],
    kind: MediaKind,
) -> Result<(), BackendError> {
    music.set_kind(tracks, track_kind(kind)).map_err(error)
}

/// Forgets the resume positions of `tracks`.
pub(super) fn forget_resume(music: &MusicLibrary, tracks: &[TrackKey]) -> Result<(), BackendError> {
    for track in tracks {
        music
            .set_resume_position(TrackId::new(track.0), None)
            .map_err(error)?;
    }
    Ok(())
}

/// The chapters of the file of `track`.
pub(super) fn chapters(
    music: &MusicLibrary,
    track: TrackKey,
) -> Result<Vec<ChapterRow>, BackendError> {
    Ok(music
        .file_sections(TrackId::new(track.0))
        .map_err(error)?
        .into_iter()
        .map(|s| ChapterRow {
            key: TrackKey(s.id.get()),
            number: s.number,
            title: s.title.unwrap_or_else(|| tr("Unknown title")),
            start: s.start,
            length: s.length,
        })
        .collect())
}

/// The title and length of a file outside the library, read from the file.
pub(super) fn file_tags(path: &Path) -> (Option<String>, Option<Duration>) {
    match read_file_info(path) {
        Ok(info) => (info.tags.title, info.audio.duration),
        Err(_) => (None, None),
    }
}
