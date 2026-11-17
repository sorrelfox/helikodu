// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Everything known about one file or track: its tags, audio properties and loudness
//! values, and its place in the library with its statistics.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use helikodu_library::{
    AudioProperties, IdentityId, Library, TrackDetails, TrackKind, TrackStats, TrackTags,
    read_file_details,
};
use helikodu_model::library::TrackId;
use helikodu_model::player::Loudness;

use super::LibraryError;

/// Where the tags and audio properties of a [`TrackInfo`] come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagOrigin {
    /// They were read from the file just now.
    File,
    /// They are what the library stored when it last read the file.
    Library,
}

/// What the library holds about a track besides its tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    /// The track.
    pub track: TrackId,
    /// Its identity, which its statistics belong to.
    pub identity: IdentityId,
    /// What kind of recording it is.
    pub kind: TrackKind,
    /// When it entered the library.
    pub added_at: SystemTime,
    /// When its file disappeared, if it did.
    pub missing_since: Option<SystemTime>,
    /// Its statistics.
    pub stats: TrackStats,
}

/// Everything known about one file or track.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackInfo {
    /// The file, as the bytes the kernel returned.
    pub path: PathBuf,
    /// The CUE track or subsong number, or 0 for a whole file.
    pub sub_index: u32,
    /// Where [`tags`](Self::tags) and [`audio`](Self::audio) come from.
    pub origin: TagOrigin,
    /// The tags.
    pub tags: TrackTags,
    /// The audio properties.
    pub audio: AudioProperties,
    /// The ReplayGain or Opus R128 values of the file's tags, relative to −18 LUFS;
    /// `None` when the file could not be read.
    pub loudness: Option<Loudness>,
    /// Why the file could not be read, for a library track whose file is gone or
    /// broken.
    pub file_error: Option<String>,
    /// The track's library entry, if it is in the library.
    pub library: Option<LibraryEntry>,
}

/// The tags of the file at `path`, read from the file, without a library.
pub fn read_file_info(path: &Path) -> Result<TrackInfo, LibraryError> {
    file_info(None, path)
}

pub(super) fn file_info(library: Option<&Library>, path: &Path) -> Result<TrackInfo, LibraryError> {
    // The library stores absolute paths, as the scan found them.
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let in_library = match library {
        Some(library) => library.track_at(&path, 0)?.map(|track| (library, track)),
        None => None,
    };
    match read_file_details(&path) {
        Ok(details) => {
            let entry = match in_library {
                Some((library, track)) => library
                    .track_details(track)?
                    .map(|stored| entry(library, &stored))
                    .transpose()?,
                None => None,
            };
            Ok(TrackInfo {
                path,
                sub_index: 0,
                origin: TagOrigin::File,
                tags: details.tags,
                audio: details.audio,
                loudness: Some(details.loudness),
                file_error: None,
                library: entry,
            })
        }
        Err(error) => match in_library {
            Some((library, track)) => track_info(library, track)?.ok_or(LibraryError::ReadFile {
                path,
                reason: error.to_string(),
            }),
            None => Err(LibraryError::ReadFile {
                path,
                reason: error.to_string(),
            }),
        },
    }
}

pub(super) fn track_info(
    library: &Library,
    track: TrackId,
) -> Result<Option<TrackInfo>, LibraryError> {
    let Some(stored) = library.track_details(track)? else {
        return Ok(None);
    };
    let entry = entry(library, &stored)?;
    let (loudness, file_error) = match read_file_details(&stored.path) {
        Ok(details) => (Some(details.loudness), None),
        Err(error) => (None, Some(error.to_string())),
    };
    Ok(Some(TrackInfo {
        path: stored.path,
        sub_index: stored.sub_index,
        origin: TagOrigin::Library,
        tags: stored.tags,
        audio: stored.audio,
        loudness,
        file_error,
        library: Some(entry),
    }))
}

fn entry(library: &Library, stored: &TrackDetails) -> Result<LibraryEntry, LibraryError> {
    Ok(LibraryEntry {
        track: stored.track,
        identity: stored.identity,
        kind: stored.kind,
        added_at: stored.added_at,
        missing_since: stored.missing_since,
        stats: library.stats(stored.track)?,
    })
}
