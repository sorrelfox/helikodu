// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning a sheet into virtual tracks: a file, a span of it and tags. The pregap
//! policy is documented on [`VirtualTrack`].

use std::path::{Path, PathBuf};

use helikodu_model::audio::MediaTime;
use helikodu_model::library::{MediaSource, SourceSection};

use crate::tags::TrackTags;

use super::fields::track_tags;
use super::path::relative_path;
use super::sheet::{CueFile, CueIndex, CueSheet, CueTrack};
use super::warning::{CueWarning, CueWarningKind as Kind, Warnings};

/// A track that is a part of a file: one track of a CUE sheet, or a chapter.
///
/// Which index starts a track, and the pregaps:
///
/// - A track starts at its `INDEX 01`. Without one, it starts at its lowest index
///   above 01, or else at `INDEX 00`, with a warning; without any index it is left
///   out.
/// - A track ends where the next track of the same file starts, at that track's
///   `INDEX 01`. The pregap of the next track (its `INDEX 00` to `INDEX 01`) is
///   therefore played at the end of this track, as a CD player does when playing
///   through. The last track of a file is open-ended: it ends with the file.
/// - `INDEX 00` is exposed as [`pregap_start`](Self::pregap_start) when it lies in
///   the same file before the start, so a player can offer to play a track with
///   its pregap. A pregap at the end of the previous file (multi-file rips with
///   gaps appended) is part of that file's last track.
/// - `PREGAP` and `POSTGAP` are silence that is not in the file; they are kept on
///   [`CueTrack`] and not turned into spans.
/// - Only `AUDIO` tracks become virtual tracks. A track that does not start after
///   the previous track of its file is left out, so spans never overlap.
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualTrack {
    /// The track number in the sheet, or the chapter's position counted from 1.
    pub number: u32,
    /// The file and the span of it to play.
    pub source: MediaSource,
    /// The track's tags, album fields included.
    pub tags: TrackTags,
    /// Where the track's pregap (`INDEX 00`) starts, when it lies in the same file
    /// before the track's start.
    pub pregap_start: Option<MediaTime>,
    /// Whether the audio has pre-emphasis (`FLAGS PRE`, or the FLAC cue sheet
    /// block's flag), which a player may undo.
    pub pre_emphasis: bool,
}

impl VirtualTrack {
    /// Where the track starts in its file.
    pub fn start(&self) -> MediaTime {
        match &self.source.section {
            Some(SourceSection::Span { start, .. }) => *start,
            _ => MediaTime::ZERO,
        }
    }

    /// Where the track ends in its file, or `None` for the end of the file.
    pub fn end(&self) -> Option<MediaTime> {
        match &self.source.section {
            Some(SourceSection::Span { end, .. }) => *end,
            _ => None,
        }
    }
}

/// Where a track's audio is: the file and the start, with its pregap.
struct Candidate<'a> {
    track: &'a CueTrack,
    file: Option<usize>,
    start: MediaTime,
    pregap_start: Option<MediaTime>,
}

/// The outcome of finding a sheet's file: the path, and a warning to report with
/// it; or why the file's tracks are left out.
pub(crate) type Located = Result<(PathBuf, Option<Kind>), Kind>;

/// Resolves `sheet` into virtual tracks; `locate` turns a `FILE` (an index into
/// `sheet.files`, or `None` for a track with no `FILE`) into a path.
pub(crate) fn resolve_with(
    sheet: &CueSheet,
    mut locate: impl FnMut(Option<usize>) -> Located,
) -> (Vec<VirtualTrack>, Vec<CueWarning>) {
    let mut warnings = Warnings::default();
    let track_total = u32::try_from(sheet.audio_tracks().count()).unwrap_or(u32::MAX);

    // Each track's start, in sheet order; a track that does not start after the
    // previous track of its file is left out.
    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    for track in sheet.audio_tracks() {
        let Some(start) = start_index(track, &mut warnings) else {
            continue;
        };
        let begin = start.time.to_media_time();
        if let Some(previous) = candidates.iter().rev().find(|c| c.file == start.file)
            && previous.start >= begin
        {
            warnings.at(track.line, Kind::OutOfOrder(track.number));
            continue;
        }
        let pregap_start = match track.index(0) {
            Some(pregap) if pregap.file == start.file && pregap.time < start.time => {
                Some(pregap.time.to_media_time())
            }
            Some(pregap) if pregap.file == start.file && pregap.time > start.time => {
                warnings.at(pregap.line, Kind::PregapAfterStart(track.number));
                None
            }
            _ => None,
        };
        candidates.push(Candidate {
            track,
            file: start.file,
            start: begin,
            pregap_start,
        });
    }

    // The paths, found once per file.
    let mut paths: Vec<(Option<usize>, Option<PathBuf>)> = Vec::new();
    let mut tracks = Vec::with_capacity(candidates.len());
    for (i, candidate) in candidates.iter().enumerate() {
        let cached = paths.iter().find(|(f, _)| *f == candidate.file);
        let path = match cached {
            Some((_, path)) if candidate.file.is_some() => path.clone(),
            _ => {
                let line = candidate
                    .file
                    .and_then(|f| sheet.files.get(f))
                    .map_or(candidate.track.line, |f| f.line);
                let path = match locate(candidate.file) {
                    Ok((path, warning)) => {
                        if let Some(kind) = warning {
                            warnings.at(line, kind);
                        }
                        Some(path)
                    }
                    Err(Kind::TrackWithoutFile(_)) => {
                        let number = candidate.track.number;
                        warnings.at(line, Kind::TrackWithoutFile(number));
                        None
                    }
                    Err(kind) => {
                        warnings.at(line, kind);
                        None
                    }
                };
                paths.push((candidate.file, path.clone()));
                path
            }
        };
        let Some(path) = path else {
            continue;
        };
        let end = candidates[i + 1..]
            .iter()
            .find(|next| next.file == candidate.file)
            .map(|next| next.start);
        let track = candidate.track;
        tracks.push(VirtualTrack {
            number: track.number,
            source: MediaSource::file(path).with_section(SourceSection::Span {
                start: candidate.start,
                end,
            }),
            tags: track_tags(sheet, track, track_total),
            pregap_start: candidate.pregap_start,
            pre_emphasis: track.flags.pre_emphasis,
        });
    }
    (tracks, warnings.into_vec())
}

/// The index a track starts at.
fn start_index<'a>(track: &'a CueTrack, warnings: &mut Warnings) -> Option<&'a CueIndex> {
    if let Some(index) = track.index(1) {
        return Some(index);
    }
    let fallback = track
        .indexes
        .iter()
        .filter(|i| i.number > 1)
        .min_by_key(|i| i.number)
        .or_else(|| track.index(0));
    match fallback {
        Some(index) => {
            warnings.at(track.line, Kind::MissingIndex01(track.number));
            Some(index)
        }
        None => {
            warnings.at(track.line, Kind::NoIndex(track.number));
            None
        }
    }
}

/// Finds a sheet's files by their names alone, inside `folder`, without looking at
/// the disk.
pub(crate) fn lexical_locator(
    sheet: &CueSheet,
    folder: &Path,
) -> impl FnMut(Option<usize>) -> Located {
    move |file| {
        let file = file_of(sheet, file)?;
        relative_path(file.name.as_bytes())
            .map(|rel| (folder.join(rel), None))
            .map_err(|reason| Kind::RejectedPath {
                name: file.name.clone(),
                reason,
            })
    }
}

/// The `FILE` with index `file`, or the warning for a track without one (its
/// number is filled in by the caller).
pub(crate) fn file_of(sheet: &CueSheet, file: Option<usize>) -> Result<&CueFile, Kind> {
    file.and_then(|f| sheet.files.get(f))
        .ok_or(Kind::TrackWithoutFile(0))
}
