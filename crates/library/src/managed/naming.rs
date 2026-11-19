// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The values a naming pattern gives a file that is not in the library yet, from
//! the tags a worker read: the same values the organizer takes from the library
//! for a file it has, so that an imported file gets the name the organizer would
//! give it.

use std::path::Path;

use helikodu_meta::tags::{FileTags, Role};
use helikodu_model::query::Field;

use crate::organize::FieldMap;

/// The values of every field and tag of the file at `path` with the tags `read`.
/// `discs` is the number of discs of its album, when known from its other files.
pub(crate) fn values(read: &FileTags, path: &Path, discs: Option<u32>) -> FieldMap {
    let tags = &read.tags;
    let audio = &read.audio;
    let mut map = FieldMap {
        track_count: tags.track_total,
        disc_count: [discs, tags.disc_total, tags.disc_number]
            .into_iter()
            .flatten()
            .max(),
        ..FieldMap::default()
    };
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = tags
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| stem.clone());
    map.set(Field::Title, title);
    let artist = tags
        .artist
        .clone()
        .filter(|a| !a.trim().is_empty())
        .or_else(|| (!tags.artists.is_empty()).then(|| tags.artists.join(", ")));
    if let Some(artist) = &artist {
        map.set(Field::Artist, artist.clone());
    }
    let album_artist = tags
        .album_artist
        .clone()
        .filter(|a| !a.trim().is_empty())
        .or_else(|| (!tags.album_artists.is_empty()).then(|| tags.album_artists.join(", ")))
        .or(artist);
    if let Some(album_artist) = album_artist {
        map.set(Field::AlbumArtist, album_artist);
    }
    let text = |map: &mut FieldMap, field: Field, value: &Option<String>| {
        if let Some(value) = value {
            map.set(field, value.clone());
        }
    };
    text(&mut map, Field::Album, &tags.album);
    text(&mut map, Field::DiscSubtitle, &tags.disc_subtitle);
    text(&mut map, Field::Work, &tags.work);
    text(&mut map, Field::Movement, &tags.movement);
    text(&mut map, Field::Codec, &audio.codec);
    text(&mut map, Field::Container, &audio.container);
    if !tags.release_types.is_empty() {
        map.set(Field::ReleaseType, tags.release_types.join("; "));
    }
    let number = |map: &mut FieldMap, field: Field, value: Option<i64>| {
        if let Some(value) = value {
            map.set(field, value.to_string());
        }
    };
    number(&mut map, Field::Year, tags.year().map(i64::from));
    number(
        &mut map,
        Field::OriginalYear,
        tags.original_year().map(i64::from),
    );
    number(&mut map, Field::Track, tags.track_number.map(i64::from));
    number(&mut map, Field::Disc, tags.disc_number.map(i64::from));
    number(
        &mut map,
        Field::MovementNumber,
        tags.movement_number.map(i64::from),
    );
    number(
        &mut map,
        Field::SampleRate,
        audio.sample_rate.map(i64::from),
    );
    number(&mut map, Field::BitDepth, audio.bit_depth.map(i64::from));
    number(&mut map, Field::Channels, audio.channels.map(i64::from));
    number(&mut map, Field::Bitrate, audio.bitrate_kbps.map(i64::from));
    let people =
        |role: Role| -> Vec<String> { tags.credits_with(role).map(|c| c.name.clone()).collect() };
    map.set_list(Field::Composer, people(Role::Composer));
    map.set_list(Field::Performer, people(Role::Performer));
    map.set_list(Field::Conductor, people(Role::Conductor));
    map.set_list(Field::Lyricist, people(Role::Lyricist));
    map.set_list(Field::Genre, tags.genres.clone());
    map.set(Field::Filename, stem);
    if let Some(folder) = path.parent().and_then(Path::file_name) {
        map.set(Field::Folder, folder.to_string_lossy().into_owned());
    }
    let mut extra: Vec<(String, Vec<String>)> = Vec::new();
    for (key, value) in &tags.extra {
        let key = key.to_uppercase();
        match extra.iter_mut().find(|(k, _)| *k == key) {
            Some((_, values)) => values.push(value.clone()),
            None => extra.push((key, vec![value.clone()])),
        }
    }
    for (key, values) in extra {
        if let Some(field) = Field::tag(&key) {
            map.set_list(field, values);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use helikodu_meta::tags::{AudioProperties, Credit, TagSource, TrackTags};
    use helikodu_model::expression::FieldValues;

    use super::*;

    #[test]
    fn values_follow_the_library_fallbacks() {
        let read = FileTags {
            tags: TrackTags {
                title: Some("Tidewater".into()),
                artist: Some("Mari Kask".into()),
                album: Some("Rooms".into()),
                date: Some("2021-03-01".into()),
                track_number: Some(1),
                disc_number: Some(2),
                credits: vec![Credit {
                    role: Role::Composer,
                    name: "Arvo".into(),
                    instrument: None,
                }],
                extra: vec![("isrc".into(), "XX".into())],
                ..TrackTags::default()
            },
            audio: AudioProperties {
                codec: Some("flac".into()),
                ..AudioProperties::default()
            },
            source: TagSource::Lofty,
            fallback_reason: None,
            charset_repaired: false,
        };
        let map = values(&read, Path::new("/in/x/a.flac"), None);
        assert_eq!(map.value(&Field::AlbumArtist).as_deref(), Some("Mari Kask"));
        assert_eq!(map.value(&Field::Year).as_deref(), Some("2021"));
        assert_eq!(map.value(&Field::Composer).as_deref(), Some("Arvo"));
        assert_eq!(map.value(&Field::Folder).as_deref(), Some("x"));
        assert_eq!(
            map.value(&Field::tag("ISRC").unwrap()).as_deref(),
            Some("XX")
        );
        assert_eq!(map.disc_count(), Some(2));
    }
}
