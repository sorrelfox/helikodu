// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sheets embedded in audio files.
//!
//! In order of preference: the `CUESHEET` tag (a Vorbis comment in FLAC and Ogg,
//! an APEv2 item in WavPack and Monkey's Audio) as the tag reader sees it; the
//! same tag as FFmpeg sees it, which also covers values longer than the tag
//! reader keeps and formats it does not read; then the binary `CUESHEET` block of a
//! FLAC file. Every `FILE` of an embedded sheet stands for the audio file itself.
//! The file's own tags fill the album fields the sheet lacks.

use std::fs::File;
use std::path::{Path, PathBuf};

use helikodu_ffmpeg_bridge::MediaFile;

use crate::tags::{ReadOptions, TrackTags, read_tags};

use super::flac_block::{self, FlacCueSheet};
use super::reading::{CueError, CueOptions, CueOrigin, CueTracks};
use super::resolve::{Located, VirtualTrack, resolve_with};
use super::sheet::CueSheet;
use super::warning::CueWarningKind as Kind;
use helikodu_model::library::{MediaSource, SourceSection};

/// Reads the sheet embedded in the audio file at `path` and returns its virtual
/// tracks, all in that file; `None` when the file has no embedded sheet.
pub fn read_embedded_cue(
    path: impl AsRef<Path>,
    options: &CueOptions,
) -> Result<Option<CueTracks>, CueError> {
    let path = path.as_ref();
    let read_options = ReadOptions {
        legacy_charset: options.legacy_charset.clone(),
        ..ReadOptions::default()
    };
    let host = read_tags(path, &read_options);
    let (host_tags, mut text) = match &host {
        Ok(file) => (
            file.tags.clone(),
            file.tags.extra_values("cuesheet").next().map(str::to_owned),
        ),
        Err(_) => (TrackTags::default(), None),
    };
    if text.is_none() {
        // FFmpeg keeps values of any length and reads formats lofty does not.
        text = MediaFile::open(path)
            .ok()
            .and_then(|media| media.tags().get("cuesheet").map(str::to_owned));
    }
    if let Some(text) = text {
        let sheet = CueSheet::parse_text(&text)?;
        let (mut tracks, resolve_warnings) = resolve_with(&sheet, embedded_locator(path));
        for track in &mut tracks {
            fill_from_host(&mut track.tags, &host_tags);
        }
        let mut warnings = sheet.warnings.clone();
        warnings.extend(resolve_warnings);
        return Ok(Some(CueTracks {
            origin: CueOrigin::Tag,
            sheet: Some(sheet),
            tracks,
            warnings,
        }));
    }
    if let Some((block, rate)) = flac_block::read(&mut File::open(path)?)? {
        let tracks = block_tracks(path, &block, rate, &host_tags);
        return Ok(Some(CueTracks {
            origin: CueOrigin::FlacBlock,
            sheet: None,
            tracks,
            warnings: Vec::new(),
        }));
    }
    match host {
        Ok(_) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Every `FILE` is the audio file; a second `FILE` is not used.
fn embedded_locator(path: &Path) -> impl FnMut(Option<usize>) -> Located {
    let path = path.to_path_buf();
    move |file| match file {
        None | Some(0) => Ok((path.clone(), None)),
        Some(_) => Err(Kind::EmbeddedExtraFile),
    }
}

/// The virtual tracks of a FLAC cue sheet block.
fn block_tracks(
    path: &Path,
    block: &FlacCueSheet,
    rate: std::num::NonZeroU32,
    host: &TrackTags,
) -> Vec<VirtualTrack> {
    let audio: Vec<_> = block
        .tracks
        .iter()
        .filter(|t| t.audio && !t.is_lead_out())
        .collect();
    let total = u32::try_from(audio.len()).unwrap_or(u32::MAX);
    // (track, start, pregap) with starts strictly increasing.
    let mut starts = Vec::new();
    for track in audio {
        // INDEX 01, as for text sheets, or else the lowest index above it, or INDEX 00.
        let fallback = track
            .indexes
            .iter()
            .map(|(n, _)| *n)
            .filter(|n| *n > 1)
            .min();
        let Some(start) = track
            .index(1)
            .or_else(|| fallback.and_then(|n| track.index(n)))
            .or_else(|| track.index(0))
        else {
            continue;
        };
        if starts.last().is_some_and(|(_, last, _)| *last >= start) {
            continue;
        }
        let pregap = track.index(0).filter(|p| *p < start);
        starts.push((track, start, pregap));
    }
    let path: PathBuf = path.to_path_buf();
    starts
        .iter()
        .enumerate()
        .map(|(i, (track, start, pregap))| {
            let end = starts
                .get(i + 1)
                .map(|(_, next, _)| flac_block::time(*next, rate));
            let mut tags = TrackTags {
                track_number: Some(u32::from(track.number)),
                track_total: Some(total),
                ..TrackTags::default()
            };
            if let Some(catalog) = &block.catalog {
                tags.extra.push(("barcode".into(), catalog.clone()));
            }
            if let Some(isrc) = &track.isrc {
                tags.extra.push(("isrc".into(), isrc.clone()));
            }
            fill_from_host(&mut tags, host);
            VirtualTrack {
                number: u32::from(track.number),
                source: MediaSource::file(path.clone()).with_section(SourceSection::Span {
                    start: flac_block::time(*start, rate),
                    end,
                }),
                tags,
                pregap_start: pregap.map(|p| flac_block::time(p, rate)),
                pre_emphasis: track.pre_emphasis,
            }
        })
        .collect()
}

/// Fills the album fields `tags` lacks from the tags of the file holding the
/// sheet. A whole-album file's artist is the album artist.
fn fill_from_host(tags: &mut TrackTags, host: &TrackTags) {
    if tags.album.is_none() {
        tags.album.clone_from(&host.album);
    }
    if tags.album_artist.is_none() {
        let (credited, list) = if host.album_artist.is_some() {
            (&host.album_artist, &host.album_artists)
        } else {
            (&host.artist, &host.artists)
        };
        tags.album_artist.clone_from(credited);
        tags.album_artists.clone_from(list);
    }
    if tags.artist.is_none() {
        tags.artist.clone_from(&tags.album_artist);
        tags.artists.clone_from(&tags.album_artists);
    }
    if tags.date.is_none() {
        tags.date.clone_from(&host.date);
    }
    if tags.original_date.is_none() {
        tags.original_date.clone_from(&host.original_date);
    }
    if tags.genres.is_empty() {
        tags.genres.clone_from(&host.genres);
    }
    if tags.disc_number.is_none() {
        tags.disc_number = host.disc_number;
        tags.disc_total = tags.disc_total.or(host.disc_total);
    }
    if tags.labels.is_empty() {
        tags.labels.clone_from(&host.labels);
    }
    if tags.catalog_numbers.is_empty() {
        tags.catalog_numbers.clone_from(&host.catalog_numbers);
    }
    if tags.release_types.is_empty() {
        tags.release_types.clone_from(&host.release_types);
    }
    if tags.musicbrainz.release.is_none() {
        tags.musicbrainz
            .release
            .clone_from(&host.musicbrainz.release);
    }
    if tags.musicbrainz.release_group.is_none() {
        tags.musicbrainz
            .release_group
            .clone_from(&host.musicbrainz.release_group);
    }
    if tags.replay_gain.album_gain_db.is_none() {
        tags.replay_gain.album_gain_db = host.replay_gain.album_gain_db;
        tags.replay_gain.album_peak = tags.replay_gain.album_peak.or(host.replay_gain.album_peak);
    }
    if tags.compilation.is_none() {
        tags.compilation = host.compilation;
    }
}
