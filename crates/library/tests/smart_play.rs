// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smarter playback over a small library: the facts the picks read, related tracks
//! for endless play, the Auto-DJ's picks and generator playlists.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::collections::HashSet;

use helikodu_library::{
    GeneratorError, GeneratorSteps, Library, NoSoundSimilarity, PlaySeed, Rating, RootId,
};
use helikodu_model::library::TrackId;
use helikodu_model::settings::{AutoDjSettings, EndlessSettings, PlayWeight};
use support::{library, tagged};

/// Artist, album, genre, year, rating in half stars.
const TRACKS: &[(&str, &str, &str, i32, Option<u8>)] = &[
    ("Ana", "Blue", "Jazz", 1960, Some(10)),
    ("Ana", "Blue", "Jazz", 1960, Some(8)),
    ("Ana", "Green", "Jazz", 1962, None),
    ("Ben", "Night", "Jazz", 1961, Some(2)),
    ("Ben", "Night", "Jazz", 1961, None),
    ("Cleo", "Loud", "Metal", 2005, Some(10)),
    ("Cleo", "Loud", "Metal", 2005, Some(6)),
    ("Dan", "Fast", "Metal", 2007, None),
    ("Dan", "Fast", "Metal", 2007, Some(1)),
    ("Eve", "Calm", "Ambient", 1995, None),
];

fn filled() -> (Library, RootId, Vec<TrackId>) {
    let (library, root) = library();
    let mut ids = Vec::new();
    for (n, (artist, album, genre, year, rating)) in TRACKS.iter().enumerate() {
        let mut record = tagged(
            root,
            &format!("/music/{artist}/{album}/{n}.flac"),
            &format!("content {n}"),
            &format!("Song {n}"),
            artist,
            album,
        );
        record.tags.genres = vec![(*genre).to_owned()];
        record.tags.year = Some(*year);
        record.tags.album_artists = vec![(*artist).to_owned()];
        record.tags.track_no = Some(n as u32 + 1);
        let track = library.upsert_track(record).unwrap().track;
        library
            .set_rating(track, rating.and_then(Rating::from_half_stars))
            .unwrap();
        ids.push(track);
    }
    (library, root, ids)
}

#[test]
fn facts_carry_the_statistics() {
    let (library, _, ids) = filled();
    let facts = library.play_facts(&ids).unwrap();
    assert_eq!(facts[&ids[0]].rating, Some(10));
    assert_eq!(facts[&ids[2]].rating, None);
    assert_eq!(facts[&ids[0]].plays, 0);
    assert_eq!(facts[&ids[0]].days_since_played, None);
    let track = &library.track_facts(&ids[..1]).unwrap()[0];
    assert_eq!(track.year, Some(1960));
    assert_eq!(track.artists.len(), 1);
    assert_eq!(track.album_artists.len(), 1);
    assert_eq!(track.genres.len(), 1);
}

#[test]
fn related_tracks_stay_close_to_the_seed() {
    let (library, _, ids) = filled();
    let settings = EndlessSettings::default();
    let avoid = HashSet::new();
    let picked = library
        .related_tracks(
            PlaySeed::Track(ids[0]),
            &settings,
            &avoid,
            3,
            11,
            &NoSoundSimilarity,
        )
        .unwrap();
    assert_eq!(picked.len(), 3);
    assert!(!picked.contains(&ids[0]), "the seed itself is not added");
    let jazz: HashSet<TrackId> = ids[..5].iter().copied().collect();
    assert!(
        picked.iter().filter(|t| jazz.contains(t)).count() >= 2,
        "{picked:?}"
    );
    // The same random seed gives the same picks; avoided tracks never come.
    let again = library
        .related_tracks(
            PlaySeed::Track(ids[0]),
            &settings,
            &avoid,
            3,
            11,
            &NoSoundSimilarity,
        )
        .unwrap();
    assert_eq!(picked, again);
    let avoid: HashSet<TrackId> = picked.iter().copied().collect();
    let more = library
        .related_tracks(
            PlaySeed::Track(ids[0]),
            &settings,
            &avoid,
            20,
            11,
            &NoSoundSimilarity,
        )
        .unwrap();
    assert_eq!(more.len(), ids.len() - 1 - picked.len());
    assert!(more.iter().all(|t| !avoid.contains(t)));
    // A filter limits the candidates.
    let filtered = EndlessSettings {
        filter: "genre:Metal".into(),
        ..EndlessSettings::default()
    };
    let metal = library
        .related_tracks(
            PlaySeed::Track(ids[0]),
            &filtered,
            &HashSet::new(),
            10,
            1,
            &NoSoundSimilarity,
        )
        .unwrap();
    let mut metal_sorted = metal.clone();
    metal_sorted.sort();
    assert_eq!(metal_sorted, ids[5..9].to_vec());
}

#[test]
fn album_and_artist_seeds() {
    let (library, _, ids) = filled();
    let album = library.track_facts(&ids[5..6]).unwrap()[0].album.unwrap();
    let profile = library.seed_profile(PlaySeed::Album(album)).unwrap();
    assert_eq!(profile.tracks, vec![ids[5], ids[6]]);
    assert_eq!(profile.years, Some((2005, 2005)));
    let artist = library.track_facts(&ids[3..4]).unwrap()[0].artists[0];
    let profile = library.seed_profile(PlaySeed::Artist(artist)).unwrap();
    assert_eq!(profile.tracks.len(), 2);
    let picked = library
        .related_tracks(
            PlaySeed::Artist(artist),
            &EndlessSettings::default(),
            &HashSet::new(),
            2,
            5,
            &NoSoundSimilarity,
        )
        .unwrap();
    assert_eq!(picked.len(), 2);
}

#[test]
fn auto_dj_filters_and_floors() {
    let (library, _, ids) = filled();
    let settings = AutoDjSettings {
        filter: "genre:Jazz".into(),
        min_rating: 4,
        weight_by: vec![PlayWeight::Rating],
        ..AutoDjSettings::default()
    };
    let mut picked = library
        .auto_dj_tracks(&settings, &HashSet::from([ids[1]]), 10, 3)
        .unwrap();
    picked.sort();
    // Jazz, not avoided, not rated below two stars (unrated tracks stay).
    assert_eq!(picked, vec![ids[0], ids[2], ids[4]]);
    let bad = AutoDjSettings {
        filter: "(genre:Jazz".into(),
        ..AutoDjSettings::default()
    };
    assert!(library.auto_dj_tracks(&bad, &HashSet::new(), 1, 1).is_err());
}

#[test]
fn generators_run_their_steps() {
    let (library, _, ids) = filled();
    let heard = library.create_playlist("Heard").unwrap();
    library.add_to_playlist(heard, vec![ids[0]]).unwrap();
    let steps: GeneratorSteps = "from library\n\
                                 where genre:Jazz\n\
                                 exclude playlist \"heard\"\n\
                                 sort rating desc\n\
                                 per-artist 1\n\
                                 limit 5"
        .parse()
        .unwrap();
    let id = library
        .create_generator("Jazz picks", &steps, false)
        .unwrap();
    assert!(matches!(
        library.create_generator("jazz PICKS", &steps, true),
        Err(GeneratorError::NameTaken(_))
    ));
    let first = library.open_generator(id, 1).unwrap();
    // Ana's best left after "Heard", then Ben's rated track before his unrated one.
    assert_eq!(first, vec![ids[1], ids[3]]);
    let stored = library.find_generator("jazz picks").unwrap().unwrap();
    assert_eq!(stored.tracks.as_deref(), Some(&first[..]));
    assert!(stored.generated.is_some());
    assert_eq!(stored.steps, steps);

    // Kept until asked, since it is not made on open.
    library.add_to_playlist(heard, vec![ids[1]]).unwrap();
    assert_eq!(library.open_generator(id, 1).unwrap(), first);
    // Ben's rated track now leads; Ana's unrated one follows.
    assert_eq!(library.regenerate(id, 1).unwrap(), vec![ids[3], ids[2]]);

    // Another generator can build on it, or leave its tracks out.
    let on_top: GeneratorSteps = "from playlist \"Jazz picks\"\nlimit 1".parse().unwrap();
    let other = library.create_generator("On top", &on_top, true).unwrap();
    assert_eq!(library.open_generator(other, 1).unwrap(), vec![ids[3]]);
    let minutes: GeneratorSteps = "from query genre:Metal\nsort random\nlimit 7 minutes"
        .parse()
        .unwrap();
    let preview = library.preview_generator(&minutes, 9).unwrap();
    // Each test track is 200 seconds long: two fit into seven minutes.
    assert_eq!(preview.len(), 2);
    assert_eq!(preview, library.preview_generator(&minutes, 9).unwrap());

    // A generator cannot use itself.
    let looping: GeneratorSteps = "from playlist \"Loop\"".parse().unwrap();
    let looped = library.create_generator("Loop", &looping, true).unwrap();
    assert!(matches!(
        library.open_generator(looped, 1),
        Err(GeneratorError::Cycle(_))
    ));

    library
        .update_generator(id, Some("Jazz"), None, Some(true))
        .unwrap();
    assert_eq!(library.generators().unwrap().len(), 3);
    assert!(library.remove_generator(id).unwrap());
    assert!(library.find_generator("Jazz").unwrap().is_none());
}
