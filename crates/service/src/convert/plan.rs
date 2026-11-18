// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a conversion will do to each track: where its file goes, and whether it is
//! converted, copied as it is or already there. Nothing is written here.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use helikodu_library::{AudioProperties, TrackTags};
use helikodu_model::library::{MediaSource, TrackId};
use helikodu_model::query::Field;

use super::naming::{FieldValues, NameStyle, NamingPattern};
use super::presets::Preset;
use crate::library::{LibraryError, MusicLibrary};

/// A library track as a conversion reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceTrack {
    /// The track.
    pub track: TrackId,
    /// What the decoder opens: the file, and the part of it for a CUE track.
    pub source: MediaSource,
    /// The file.
    pub path: PathBuf,
    /// The tags, as the library holds them.
    pub tags: TrackTags,
    /// The audio properties.
    pub audio: AudioProperties,
    /// The size of the file in bytes.
    pub size: Option<u64>,
    /// The number of discs of its album, as far as the tracks read with it show.
    pub disc_total: Option<u32>,
}

impl SourceTrack {
    /// Whether the track is its whole file (not a section of one).
    pub fn is_whole_file(&self) -> bool {
        self.source.section.is_none()
    }

    /// The length.
    pub fn duration(&self) -> Option<Duration> {
        self.audio.duration
    }

    /// The codec, as FFmpeg names it (`flac`, `mp3`, …).
    pub fn codec(&self) -> &str {
        self.audio.codec.as_deref().unwrap_or("")
    }

    /// The bit rate in kbit/s.
    pub fn kbps(&self) -> Option<u32> {
        self.audio.bitrate.map(|b| b / 1000)
    }
}

impl FieldValues for SourceTrack {
    fn value(&self, field: &Field) -> Option<String> {
        let t = &self.tags;
        let text = |v: &Option<String>| v.clone().filter(|v| !v.trim().is_empty());
        let list = |v: &[String]| (!v.is_empty()).then(|| v.join(", "));
        let stem = || {
            self.path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        };
        match field {
            Field::Title => text(&t.title).or_else(stem),
            Field::Artist => list(&t.artists),
            Field::AlbumArtist => list(&t.album_artists).or_else(|| list(&t.artists)),
            Field::Album => text(&t.album),
            Field::Composer => list(&t.composers),
            Field::Genre => list(&t.genres),
            Field::Year => t.year.map(|y| y.to_string()),
            Field::Track => t.track_no.map(|n| n.to_string()),
            Field::Disc => t.disc_no.map(|n| n.to_string()),
            Field::Codec => self.audio.codec.clone(),
            Field::Container => self.audio.container.clone(),
            Field::SampleRate => self.audio.sample_rate.map(|r| r.to_string()),
            Field::BitDepth => self.audio.bit_depth.map(|b| b.to_string()),
            Field::Channels => self.audio.channels.map(|c| c.to_string()),
            Field::Bitrate => self.audio.bitrate.map(|b| (b / 1000).to_string()),
            Field::Filename => stem(),
            Field::Folder => self
                .path
                .parent()
                .and_then(Path::file_name)
                .map(|s| s.to_string_lossy().into_owned()),
            _ => None,
        }
    }

    fn disc_total(&self) -> Option<u32> {
        self.disc_total
    }
}

/// Reads `tracks` from the library for a conversion; tracks that are gone are left
/// out.
pub fn source_tracks(
    library: &MusicLibrary,
    tracks: &[TrackId],
) -> Result<Vec<SourceTrack>, LibraryError> {
    let mut out = Vec::with_capacity(tracks.len());
    for &track in tracks {
        let Some(details) = library.track_details(track)? else {
            continue;
        };
        if details.missing_since.is_some() {
            continue;
        }
        let Some(source) = library.track_source(track)? else {
            continue;
        };
        let size = std::fs::metadata(&details.path).ok().map(|m| m.len());
        out.push(SourceTrack {
            track,
            source: source.source,
            path: details.path,
            tags: details.tags,
            audio: details.audio,
            size,
            disc_total: None,
        });
    }
    // An album shows several discs when one of its tracks is on a disc after the
    // first.
    let mut discs: std::collections::HashMap<(Option<String>, Vec<String>), u32> =
        std::collections::HashMap::new();
    for t in &out {
        let key = (t.tags.album.clone(), t.tags.album_artists.clone());
        let disc = t.tags.disc_no.unwrap_or(1);
        let entry = discs.entry(key).or_insert(1);
        *entry = (*entry).max(disc);
    }
    for t in &mut out {
        let key = (t.tags.album.clone(), t.tags.album_artists.clone());
        t.disc_total = discs.get(&key).copied().filter(|n| *n > 1);
    }
    Ok(out)
}

/// What happens to one track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannedAction {
    /// It is converted with the preset.
    Convert,
    /// It is in the preset's format already and copied as it is.
    Copy,
    /// A file is at the target already and is left alone.
    AlreadyThere,
}

/// One track of a planned conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedItem {
    /// The track.
    pub track: SourceTrack,
    /// The target, relative to the destination folder, with its extension.
    pub relative: PathBuf,
    /// The target.
    pub target: PathBuf,
    /// What happens.
    pub action: PlannedAction,
    /// The size the file is expected to have, in bytes.
    pub expected_bytes: u64,
}

/// What a conversion of `tracks` with `preset` into `folder` will do. Targets are
/// named by `pattern`; two tracks that would get the same name get ` (2)`, ` (3)`,
/// … Existing files are replaced only with `replace`.
pub fn plan_conversion(
    tracks: &[SourceTrack],
    preset: &Preset,
    folder: &Path,
    pattern: &NamingPattern,
    style: NameStyle,
    replace: bool,
) -> Vec<PlannedItem> {
    let mut taken: HashSet<PathBuf> = HashSet::new();
    tracks
        .iter()
        .map(|track| {
            let copy = track.is_whole_file()
                && preset.takes_as_is(track.codec(), track.kbps(), track.audio.sample_rate);
            let extension = if copy {
                track
                    .path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_else(|| preset.encode_format().extension().to_owned())
            } else {
                preset.encode_format().extension().to_owned()
            };
            let stem = pattern.render(track, style);
            let relative = unique(&stem, &extension, &mut taken);
            let target = folder.join(&relative);
            let action = if !replace && target.exists() {
                PlannedAction::AlreadyThere
            } else if copy {
                PlannedAction::Copy
            } else {
                PlannedAction::Convert
            };
            let expected_bytes = match action {
                PlannedAction::Convert => preset.expected_bytes(
                    track.duration().map_or(0.0, |d| d.as_secs_f64()),
                    track.audio.sample_rate.unwrap_or(44_100),
                    track.audio.channels.unwrap_or(2),
                    u32::from(track.audio.bit_depth.unwrap_or(16)),
                ),
                PlannedAction::Copy => track.size.unwrap_or(0),
                PlannedAction::AlreadyThere => 0,
            };
            PlannedItem {
                track: track.clone(),
                relative,
                target,
                action,
                expected_bytes,
            }
        })
        .collect()
}

/// `stem.extension`, or with ` (2)`, ` (3)`, … when that is taken (compared
/// without regard to case, as FAT and exFAT compare names).
fn unique(stem: &Path, extension: &str, taken: &mut HashSet<PathBuf>) -> PathBuf {
    let with = |n: usize| {
        let mut name = OsString::from(stem.as_os_str());
        if n > 1 {
            name.push(format!(" ({n})"));
        }
        name.push(".");
        name.push(extension);
        PathBuf::from(name)
    };
    let key = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    let mut n = 1;
    loop {
        let candidate = with(n);
        if taken.insert(key(&candidate)) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, codec: &str, kbps: Option<u32>) -> SourceTrack {
        let tags = TrackTags {
            title: Some(title.into()),
            album_artists: vec!["Mari Kask".into()],
            album: Some("Tidewater Rooms".into()),
            track_no: Some(1),
            ..TrackTags::default()
        };
        let audio = AudioProperties {
            codec: Some(codec.into()),
            bitrate: kbps.map(|k| k * 1000),
            sample_rate: Some(48_000),
            duration: Some(Duration::from_secs(60)),
            ..AudioProperties::default()
        };
        SourceTrack {
            track: TrackId::new(1),
            source: MediaSource::file(format!("/music/{title}.{codec}")),
            path: PathBuf::from(format!("/music/{title}.{codec}")),
            tags,
            audio,
            size: Some(1000),
            disc_total: None,
        }
    }

    #[test]
    fn plans_names_actions_and_clashes() {
        let presets = Preset::builtins();
        let opus = &presets[0];
        let pattern = NamingPattern::parse(NamingPattern::DEFAULT).unwrap();
        let tracks = [
            track("Tidewater", "flac", None),
            track("Tidewater", "flac", None),
            track("Kelp", "opus", Some(120)),
        ];
        let plan = plan_conversion(
            &tracks,
            opus,
            Path::new("/nonexistent/phone"),
            &pattern,
            NameStyle::Portable,
            false,
        );
        let names: Vec<_> = plan.iter().map(|p| p.relative.clone()).collect();
        assert_eq!(
            names,
            [
                PathBuf::from("Mari Kask/Tidewater Rooms/01 Tidewater.opus"),
                PathBuf::from("Mari Kask/Tidewater Rooms/01 Tidewater (2).opus"),
                PathBuf::from("Mari Kask/Tidewater Rooms/01 Kelp.opus"),
            ]
        );
        assert_eq!(plan[0].action, PlannedAction::Convert);
        assert_eq!(plan[2].action, PlannedAction::Copy);
        assert_eq!(plan[0].expected_bytes, 960_000);
    }
}
