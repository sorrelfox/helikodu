// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A file as the tag editor loads it.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::covers::{CoverHash, CoverLimits, read_embedded_covers};
use crate::tags::reading::{ReadOptions, TagError, read_tags_from_file};
use crate::tags::record::{AudioProperties, TagSource, TrackTags};

use super::changes::FieldKey;
use super::formats;
use super::safe_save::EditStamp;
use super::values::field_values;
use super::{WriteError, is_writable};

/// How often a file that changes while it is being read is read again.
const READ_ATTEMPTS: usize = 3;

/// One file opened in the tag editor.
#[derive(Debug, Clone, PartialEq)]
pub struct EditableFile {
    /// The file.
    pub path: PathBuf,
    /// Its size and modification time when it was read; give it to the write so
    /// that a file changed meanwhile is not overwritten.
    pub stamp: EditStamp,
    /// The tags.
    pub tags: TrackTags,
    /// The audio properties.
    pub audio: AudioProperties,
    /// Whether its tags can be written. When not, the editor shows them
    /// insensitive with the reason [`WriteError::ReadOnlyFormat`] gives.
    pub writable: bool,
    /// The tags the file carries, such as `ID3v2.4` and `APEv2`.
    pub tag_formats: Vec<String>,
    /// The unsynchronized lyrics in the tags.
    pub lyrics: Option<String>,
    /// The hash of the embedded front cover (the best embedded picture).
    pub front_cover: Option<CoverHash>,
    /// Whether some text was decoded from a legacy charset; saving writes it as
    /// Unicode.
    pub charset_repaired: bool,
}

impl EditableFile {
    /// The values of `key`, as [`field_values`] gives them.
    pub fn values(&self, key: &FieldKey) -> Vec<String> {
        field_values(&self.tags, key)
    }
}

/// Reads the file at `path` for editing: its stamp, tags, lyrics and front cover.
/// A file that changes while it is read is read again.
pub fn load_for_editing(
    path: impl AsRef<Path>,
    options: &ReadOptions,
) -> Result<EditableFile, TagError> {
    let path = path.as_ref();
    let mut attempt = 0;
    loop {
        attempt += 1;
        let stamp = EditStamp::of(path)?;
        let file = load_once(path, stamp, options)?;
        if EditStamp::of(path)? == stamp || attempt == READ_ATTEMPTS {
            return Ok(file);
        }
    }
}

fn load_once(
    path: &Path,
    stamp: EditStamp,
    options: &ReadOptions,
) -> Result<EditableFile, TagError> {
    let read = read_tags_from_file(File::open(path)?, options)?;
    let writable = read.source == TagSource::Lofty && is_writable(path)?;
    let (tag_formats, lyrics) = if writable {
        let mut file = File::open(path)?;
        (
            formats::tag_formats(&mut file).unwrap_or_default(),
            formats::lyrics(&mut file).unwrap_or_default(),
        )
    } else {
        (Vec::new(), None)
    };
    let front_cover = read_embedded_covers(path, &CoverLimits::default())
        .ok()
        .and_then(|covers| covers.front().and_then(|c| c.hash().ok()));
    Ok(EditableFile {
        path: path.to_owned(),
        stamp,
        tags: read.tags,
        audio: read.audio,
        writable,
        tag_formats,
        lyrics,
        front_cover,
        charset_repaired: read.charset_repaired,
    })
}

/// Reads the unsynchronized lyrics of the file at `path` (ID3v2 `USLT`, the
/// Vorbis `LYRICS` comment, APEv2 `Lyrics`, MP4 `©lyr`); `None` for a file
/// without them or of a format whose tags cannot be written.
pub fn read_lyrics(path: impl AsRef<Path>) -> Result<Option<String>, WriteError> {
    formats::lyrics(&mut File::open(path)?)
}
