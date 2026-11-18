// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What plays for a library track, and ids rebuilt from plain numbers, through the
//! public API.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::path::Path;

use helikodu_library::{
    AlbumId, ArtistQuery, ArtistRole, GenreId, GenreQuery, PersonId, TrackFilter, TrackQuery,
    TrackSection, TrackSource,
};
use helikodu_model::audio::{MediaTime, SampleRate};
use helikodu_model::library::{MediaSource, SourceSection, TrackId};
use support::{library, record, tagged};

#[test]
fn a_whole_file_and_the_tracks_of_a_cue_sheet() {
    let (library, root) = library();
    let whole = library
        .upsert_track(record(root, "/music/a.flac", "a"))
        .unwrap()
        .track;
    let mut first = record(root, "/music/album.flac", "cue");
    first.section = TrackSection::CueSheet;
    first.sub_index = 1;
    first.frames = Some((0, Some(44_100 * 200)));
    let mut second = first.clone();
    second.sub_index = 2;
    second.frames = Some((44_100 * 200, None));
    let written = library.upsert_tracks(vec![first, second]).unwrap();

    assert_eq!(
        library.track_source(whole).unwrap(),
        Some(TrackSource {
            path: "/music/a.flac".into(),
            section: None,
        })
    );
    let rate = SampleRate::new(44_100).unwrap();
    let at = |frames| MediaTime::from_frames(frames, rate);
    let first = library.track_source(written[0].track).unwrap().unwrap();
    assert_eq!(
        first.section,
        Some(SourceSection::Span {
            start: at(0),
            end: Some(at(44_100 * 200)),
        })
    );
    let second = library.track_source(written[1].track).unwrap().unwrap();
    assert_eq!(second.path, Path::new("/music/album.flac"));
    let span = SourceSection::Span {
        start: at(44_100 * 200),
        end: None,
    };
    assert_eq!(second.section, Some(span.clone()));
    assert_eq!(
        MediaSource::from(second),
        MediaSource::file("/music/album.flac").with_section(span)
    );
    assert_eq!(library.track_source(TrackId::new(999)).unwrap(), None);
}

#[test]
fn ids_from_plain_numbers_find_their_rows() {
    let (library, root) = library();
    let mut track = tagged(root, "/music/a.flac", "a", "Song", "Singer", "Record");
    track.tags.genres = vec!["Jazz".into()];
    library.upsert_track(track).unwrap();

    let query = |filter| TrackQuery {
        filter,
        ..TrackQuery::default()
    };
    let none = library
        .tracks(&query(TrackFilter::Album(AlbumId::from_raw(-1))))
        .unwrap();
    assert!(none.items.is_empty(), "no album has the number -1");

    let artists = library
        .artists(&ArtistQuery::new(ArtistRole::Artist))
        .unwrap();
    let person = PersonId::from_raw(i64::from(artists.items[0].id));
    assert_eq!(person, artists.items[0].id);
    let genres = library.genres(&GenreQuery::default()).unwrap();
    let genre = GenreId::from_raw(genres.items[0].id.get());
    let tracks = library.tracks(&query(TrackFilter::Genre(genre))).unwrap();
    assert_eq!(tracks.items.len(), 1);
    let album = tracks.items[0].album.unwrap();
    assert_eq!(AlbumId::from_raw(album.get()), album);
    let by_person = library
        .tracks(&query(TrackFilter::Artist(person, None)))
        .unwrap();
    assert_eq!(by_person.items.len(), 1);
}
