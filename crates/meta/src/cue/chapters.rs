// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Container chapters (MP4/M4B, Matroska, Ogg `CHAPTERxxx`, ID3v2 `CHAP`) as
//! virtual tracks.
//!
//! Chapters are taken in order of their start; a chapter starting where the
//! previous one starts is left out. A chapter ends where the container says, but
//! no later than the next chapter's start, and the last one without an end is
//! open-ended. The title is the chapter's; the artist is the chapter's `artist`
//! tag or the file's; the album is the file's album, or else its title; the album
//! artist, date and genres are the file's.

use std::path::Path;

use helikodu_ffmpeg_bridge::{Chapter, MediaFile};
use helikodu_model::library::{MediaSource, SourceSection};

use crate::tags::{ReadOptions, TrackTags, read_tags};

use super::reading::CueError;
use super::resolve::VirtualTrack;

/// The chapters of the file at `path` as virtual tracks; empty when it has none.
pub fn read_chapters(path: impl AsRef<Path>) -> Result<Vec<VirtualTrack>, CueError> {
    let path = path.as_ref();
    let chapters = MediaFile::open(path)?.chapters();
    if chapters.is_empty() {
        return Ok(Vec::new());
    }
    let file_tags = read_tags(path, &ReadOptions::default())
        .map(|f| f.tags)
        .unwrap_or_default();
    Ok(chapters_to_tracks(path, &chapters, &file_tags))
}

/// `chapters` of the file at `path`, whose own tags are `file_tags`, as virtual
/// tracks.
pub fn chapters_to_tracks(
    path: &Path,
    chapters: &[Chapter],
    file_tags: &TrackTags,
) -> Vec<VirtualTrack> {
    let mut ordered: Vec<&Chapter> = chapters.iter().collect();
    ordered.sort_by_key(|c| c.start);
    ordered.dedup_by_key(|c| c.start);
    let total = u32::try_from(ordered.len()).unwrap_or(u32::MAX);

    let album = file_tags.album.clone().or_else(|| file_tags.title.clone());
    let (album_artist, album_artists) = if file_tags.album_artist.is_some() {
        (&file_tags.album_artist, &file_tags.album_artists)
    } else {
        (&file_tags.artist, &file_tags.artists)
    };

    ordered
        .iter()
        .enumerate()
        .map(|(i, chapter)| {
            let next = ordered.get(i + 1).map(|c| c.start);
            let end = match (chapter.end.filter(|e| *e > chapter.start), next) {
                (Some(end), Some(next)) => Some(end.min(next)),
                (end, next) => end.or(next),
            };
            let number = u32::try_from(i + 1).unwrap_or(u32::MAX);
            let artist = chapter
                .tags
                .get("artist")
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .map(str::to_owned);
            let (artist, artists) = match artist {
                Some(a) => (Some(a.clone()), vec![a]),
                None => (file_tags.artist.clone(), file_tags.artists.clone()),
            };
            let tags = TrackTags {
                title: chapter
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned),
                artist,
                artists,
                album: album.clone(),
                album_artist: album_artist.clone(),
                album_artists: album_artists.clone(),
                track_number: Some(number),
                track_total: Some(total),
                disc_number: file_tags.disc_number,
                disc_total: file_tags.disc_total,
                date: file_tags.date.clone(),
                genres: file_tags.genres.clone(),
                ..TrackTags::default()
            };
            VirtualTrack {
                number,
                source: MediaSource::file(path).with_section(SourceSection::Span {
                    start: chapter.start,
                    end,
                }),
                tags,
                pregap_start: None,
                pre_emphasis: false,
            }
        })
        .collect()
}
