// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The facets of the track table's browser and the counts of the statistics view.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use helikodu_library::{
    FacetChoice, FacetField, FacetKey, Library, Listen, ListenEnd, PlayRules, QueryRequest, RootId,
    StatisticsRequest, TrackTags, parse_query,
};
use helikodu_model::library::TrackId;
use helikodu_model::query::Query;
use support::{library, record};

#[allow(clippy::too_many_arguments)]
fn add(
    library: &Library,
    root: RootId,
    name: &str,
    genre: &str,
    album_artist: &str,
    album: &str,
    year: i32,
    codec: &str,
    bits: u16,
) -> TrackId {
    let mut track = record(root, &format!("/music/{name}.x"), name);
    track.tags = TrackTags {
        title: Some(name.to_owned()),
        artists: vec![album_artist.to_owned()],
        album_artists: vec![album_artist.to_owned()],
        album: Some(album.to_owned()),
        genres: vec![genre.to_owned()],
        year: Some(year),
        ..TrackTags::default()
    };
    track.audio.codec = Some(codec.to_owned());
    track.audio.bit_depth = Some(bits);
    library.upsert_track(track).unwrap().track
}

fn fixture() -> (Library, Vec<TrackId>) {
    let (library, root) = library();
    let tracks = vec![
        add(
            &library,
            root,
            "a1",
            "Ambient",
            "Mari Kask",
            "Harbour",
            2019,
            "flac",
            24,
        ),
        add(
            &library,
            root,
            "a2",
            "Ambient",
            "Mari Kask",
            "Harbour",
            2019,
            "flac",
            24,
        ),
        add(
            &library,
            root,
            "a3",
            "Ambient",
            "Okto",
            "Night Ferry",
            2020,
            "mp3",
            0,
        ),
        add(
            &library, root, "b1", "Blues", "Okto", "Delta", 2020, "FLAC", 16,
        ),
        add(
            &library, root, "c1", "Choral", "Sola", "Voices", 2021, "opus", 0,
        ),
    ];
    (library, tracks)
}

fn request(query: Query) -> QueryRequest {
    QueryRequest::new(query)
}

fn names(list: &helikodu_library::FacetList) -> Vec<(String, usize)> {
    list.values
        .iter()
        .map(|v| (v.name.clone(), v.tracks))
        .collect()
}

#[test]
fn panes_count_the_values_and_narrow_each_other() {
    let (library, _) = fixture();
    let all = request(Query::default());
    let genres = library.facet_values(&all, &[], FacetField::Genre).unwrap();
    assert_eq!(genres.tracks, 5);
    assert_eq!(
        names(&genres),
        [
            ("Ambient".to_owned(), 3),
            ("Blues".to_owned(), 1),
            ("Choral".to_owned(), 1)
        ]
    );
    let ambient = genres.values[0].key.clone();
    let chosen = [FacetChoice {
        field: FacetField::Genre,
        keys: vec![ambient.clone()],
    }];
    let artists = library
        .facet_values(&all, &chosen, FacetField::AlbumArtist)
        .unwrap();
    assert_eq!(artists.tracks, 3);
    assert_eq!(
        names(&artists),
        [("Mari Kask".to_owned(), 2), ("Okto".to_owned(), 1)]
    );
    // Two genres at once.
    let blues = genres.values[1].key.clone();
    let both = [FacetChoice {
        field: FacetField::Genre,
        keys: vec![ambient, blues],
    }];
    let albums = library
        .facet_values(&all, &both, FacetField::Album)
        .unwrap();
    assert_eq!(albums.tracks, 4);
    assert_eq!(albums.values.len(), 3);
    assert_eq!(library.facet_track_ids(&all, &both).unwrap().len(), 4);

    // Years newest first; formats by their number, codecs in lower case.
    let years = library.facet_values(&all, &[], FacetField::Year).unwrap();
    assert_eq!(years.values[0].key, FacetKey::Id(2021));
    let formats = library.facet_values(&all, &[], FacetField::Format).unwrap();
    assert_eq!(formats.values[0].key, FacetKey::Text("flac".into()));
    assert_eq!(formats.values[0].tracks, 3);
}

#[test]
fn the_query_filters_before_the_panes() {
    let (library, _) = fixture();
    let query = request(parse_query("year:2020").unwrap());
    let genres = library
        .facet_values(&query, &[], FacetField::Genre)
        .unwrap();
    assert_eq!(genres.tracks, 2);
    assert_eq!(
        names(&genres),
        [("Ambient".to_owned(), 1), ("Blues".to_owned(), 1)]
    );
    let chosen = [FacetChoice {
        field: FacetField::Format,
        keys: vec![FacetKey::Text("flac".into())],
    }];
    let ids = library.facet_track_ids(&query, &chosen).unwrap();
    assert_eq!(ids.len(), 1);
}

#[test]
fn statistics_count_formats_quality_and_plays() {
    let (library, tracks) = fixture();
    let rules = PlayRules::default();
    for (track, times) in [(tracks[0], 3), (tracks[3], 1)] {
        for i in 0..times {
            library
                .record_listen(
                    track,
                    Listen {
                        started_at: SystemTime::now() - Duration::from_secs(60 * (i + 1)),
                        played: Duration::from_secs(200),
                        end: ListenEnd::Finished,
                        position: None,
                        context: None,
                    },
                    rules,
                )
                .unwrap();
        }
    }
    let statistics = library
        .statistics(&StatisticsRequest {
            added_since: UNIX_EPOCH,
            listening_since: SystemTime::now() - Duration::from_secs(86_400),
            top: 5,
        })
        .unwrap();
    assert_eq!(statistics.added_since, 5);
    assert_eq!(statistics.albums_without_cover, 4);
    assert_eq!(statistics.formats[0], (Some("flac".to_owned()), 3));
    assert_eq!(statistics.lossless_hi_res, 2);
    assert_eq!(statistics.lossless_cd, 1);
    assert_eq!(statistics.lossy, 2);
    assert_eq!(statistics.plays, 4);
    assert_eq!(statistics.listened, Duration::from_secs(800));
    assert_eq!(statistics.never_played, 3);
    assert_eq!(statistics.top_artists[0].name, "Mari Kask");
    assert_eq!(statistics.top_artists[0].plays, 3);
    assert_eq!(statistics.top_albums[0].title.as_deref(), Some("Harbour"));
    assert_eq!(statistics.top_albums.len(), 2);
}
