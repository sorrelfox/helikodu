// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a conversion will do to each track: where its file goes, and whether it is
//! converted, copied as it is or already there. Nothing is written here.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::presets::Preset;
use crate::library::{LibraryError, MusicLibrary};
use helikodu_library::{AudioProperties, CharacterSet, FieldMap, NamingPattern, TrackTags};
use helikodu_model::library::{MediaSource, TrackId};

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
    /// The values a naming pattern names it by, as the library gives them to the
    /// file organizer too.
    pub names: FieldMap,
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

/// Reads `tracks` from the library for a conversion; tracks that are gone are left
/// out.
pub fn source_tracks(
    library: &MusicLibrary,
    tracks: &[TrackId],
) -> Result<Vec<SourceTrack>, LibraryError> {
    let mut names: std::collections::HashMap<TrackId, FieldMap> =
        library.inner().naming_values(tracks)?.into_iter().collect();
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
            names: names.remove(&track).unwrap_or_default(),
        });
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
/// named by `pattern` with the characters of `set`, a field that is empty outside
/// square brackets written as `unknown`; two tracks that would get the same name
/// get ` (2)`, ` (3)`, … Existing files are replaced only with `replace`.
pub fn plan_conversion(
    tracks: &[SourceTrack],
    preset: &Preset,
    folder: &Path,
    pattern: &NamingPattern,
    set: CharacterSet,
    unknown: &str,
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
            let stem = pattern.render_path(&track.names, set, unknown, &format!(".{extension}"));
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

    use helikodu_model::query::Field;

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
            names: FieldMap::default()
                .with(Field::Title, title)
                .with(Field::AlbumArtist, "Mari Kask")
                .with(Field::Album, "Tidewater Rooms")
                .with(Field::Track, "1"),
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
            CharacterSet::Windows,
            "Unknown",
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
