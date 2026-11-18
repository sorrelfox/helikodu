// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Copying a track's tags and front cover into its converted file.

use std::path::Path;

use helikodu_library::pictures::{CoverLimits, PictureFormat};
use helikodu_library::tag_editing::{
    CoverChange, FieldKey, ReadOptions, TagChanges, TagField, WriteOptions, load_for_editing,
    write_tags,
};
use helikodu_library::{TrackTags, read_cover};
use helikodu_model::library::TrackId;

use crate::library::MusicLibrary;

/// The fields of the file at `path` as changes for a new file. Opus header gains
/// (`R128_*`) are left out: they belong to the source's encoding.
fn file_changes(path: &Path) -> Result<TagChanges, String> {
    let file = load_for_editing(path, &ReadOptions::default()).map_err(|e| e.to_string())?;
    let mut changes = TagChanges::new();
    for &field in TagField::ALL {
        if field.is_r128() {
            continue;
        }
        let values = file.values(&FieldKey::Known(field));
        if !values.is_empty() {
            changes.set(field, values);
        }
    }
    Ok(changes)
}

/// The library's fields of a track that is a section of a file (a CUE track),
/// whose file tags describe the whole file.
fn catalog_changes(tags: &TrackTags) -> TagChanges {
    let mut changes = TagChanges::new();
    let mut set = |field: TagField, values: Vec<String>| {
        if !values.is_empty() {
            changes.set(field, values);
        }
    };
    set(TagField::Title, tags.title.iter().cloned().collect());
    set(TagField::Artist, tags.artists.clone());
    set(TagField::AlbumArtist, tags.album_artists.clone());
    set(TagField::Album, tags.album.iter().cloned().collect());
    set(TagField::Composer, tags.composers.clone());
    set(TagField::Genre, tags.genres.clone());
    set(
        TagField::TrackNumber,
        tags.track_no.iter().map(u32::to_string).collect(),
    );
    set(
        TagField::DiscNumber,
        tags.disc_no.iter().map(u32::to_string).collect(),
    );
    set(
        TagField::Date,
        tags.date
            .clone()
            .or_else(|| tags.year.map(|y| y.to_string()))
            .into_iter()
            .collect(),
    );
    changes
}

/// The encoded front cover of `track`, from its file or its folder, as the library
/// knows it; JPEG or PNG only, which tags take.
pub(crate) fn track_cover(library: &MusicLibrary, track: TrackId) -> Option<Vec<u8>> {
    let inner = library.inner();
    let info = inner.track_cover(track).ok()??;
    let locations = inner.cover_locations(&info.hash).ok()?;
    let bytes = read_cover(&info.hash, &locations, &CoverLimits::default()).ok()?;
    matches!(
        PictureFormat::sniff(&bytes),
        Some(PictureFormat::Jpeg | PictureFormat::Png)
    )
    .then_some(bytes)
}

/// Writes the tags of the source into the converted file at `path`: those of the
/// file `source` when the track is the whole file, else the library's `tags`.
pub(crate) fn write_into(
    path: &Path,
    source: &Path,
    whole_file: bool,
    tags: Option<&TrackTags>,
    cover: Option<Vec<u8>>,
) -> Result<(), String> {
    let mut changes = match (whole_file, tags) {
        (false, Some(tags)) => catalog_changes(tags),
        _ => file_changes(source)?,
    };
    changes.cover = cover.map(CoverChange::SetFront);
    if changes.is_empty() {
        return Ok(());
    }
    write_tags(path, &changes, None, &WriteOptions::default())
        .map(drop)
        .map_err(|e| e.to_string())
}
