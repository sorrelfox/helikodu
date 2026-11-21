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

#[test]
fn generator_trials_count_each_step() {
    let (library, _, ids) = filled();
    let heard = library.create_playlist("Heard").unwrap();
    library.add_to_playlist(heard, vec![ids[0]]).unwrap();
    let steps: GeneratorSteps = "from library\n\
                                 where genre:Jazz\n\
                                 exclude playlist \"Heard\"\n\
                                 per-artist 1\n\
                                 limit 1"
        .parse()
        .unwrap();
    let trial = library.try_generator(&steps, 1, None);
    assert!(trial.failure.is_none());
    assert_eq!(trial.counts, vec![10, 5, 4, 2, 1]);
    assert_eq!(trial.tracks.len(), 1);

    // A playlist that is gone stops the run at its step, with the counts before.
    let gone: GeneratorSteps =
        "from library\nwhere genre:Metal\nexclude playlist \"Gone\"\nlimit 2"
            .parse()
            .unwrap();
    let trial = library.try_generator(&gone, 1, None);
    assert_eq!(trial.counts, vec![10, 4]);
    assert!(matches!(
        trial.failure,
        Some((2, GeneratorError::UnknownPlaylist(ref name))) if name == "Gone"
    ));
    assert_eq!(trial.tracks.len(), 4);
}

#[test]
fn generators_sit_in_folders_and_pins() {
    use helikodu_library::PinTarget;
    let (library, _, _) = filled();
    let folder = library.create_playlist_folder("Moods", None).unwrap();
    let id = library
        .create_generator("Picks", &GeneratorSteps::default(), true)
        .unwrap();
    library.move_generator(id, Some(folder)).unwrap();
    assert_eq!(library.generator(id).unwrap().unwrap().parent, Some(folder));
    library.pin(PinTarget::Generator(id)).unwrap();
    let pins = library.pins().unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].name, "Picks");
    assert_eq!(pins[0].target, PinTarget::Generator(id));
    // Deleting the folder moves the generator up; removing it drops its pin.
    library.delete_playlist(folder).unwrap();
    assert_eq!(library.generator(id).unwrap().unwrap().parent, None);
    assert!(library.remove_generator(id).unwrap());
    assert!(library.pins().unwrap().is_empty());
}

/// The names at the top (or in `folder`), playlists and generators together, in
/// the sidebar's order.
fn sidebar(library: &Library, folder: Option<helikodu_library::PlaylistId>) -> Vec<String> {
    let mut names: Vec<String> = library
        .playlist_tree()
        .unwrap()
        .into_iter()
        .filter(|n| n.summary.parent == folder)
        .map(|n| n.summary.name)
        .collect();
    let places = library.generator_places().unwrap();
    let mut generators: Vec<(usize, String)> = library
        .generators()
        .unwrap()
        .into_iter()
        .filter(|g| g.parent == folder)
        .map(|g| {
            let place = places.iter().find(|(id, _)| *id == g.id).unwrap().1;
            (place, g.name)
        })
        .collect();
    generators.sort();
    for (place, name) in generators {
        names.insert(place.min(names.len()), name);
    }
    names
}

#[test]
fn generators_keep_their_place_among_playlists() {
    let (library, _, _) = filled();
    let first = library.create_playlist("First").unwrap();
    let steps = GeneratorSteps::default();
    let picks = library.create_generator("Picks", &steps, true).unwrap();
    let last = library.create_playlist("Last").unwrap();
    let top = sidebar(&library, None);
    let at = |name: &str, names: &[String]| names.iter().position(|n| n == name).unwrap();
    // Made between two playlists, it is listed between them, not after them.
    assert_eq!(at("Picks", &top), at("First", &top) + 1);
    assert_eq!(at("Last", &top), at("Picks", &top) + 1);
    // Moved to the first place, it still comes after Favourites.
    library.place_generator(picks, None, Some(0)).unwrap();
    let top = sidebar(&library, None);
    assert_eq!(at("Picks", &top), 1);
    assert_eq!(top[0], "Favourites");
    // A playlist moved before it counts it.
    library.move_playlist(last, None, Some(1)).unwrap();
    let top = sidebar(&library, None);
    assert_eq!(at("Last", &top), 1);
    assert_eq!(at("Picks", &top), 2);
    // Into a folder it goes after what is there, and can move to its start.
    let folder = library.create_playlist_folder("Moods", None).unwrap();
    library.move_playlist(first, Some(folder), None).unwrap();
    library.move_generator(picks, Some(folder)).unwrap();
    assert_eq!(sidebar(&library, Some(folder)), ["First", "Picks"]);
    library
        .place_generator(picks, Some(folder), Some(0))
        .unwrap();
    assert_eq!(sidebar(&library, Some(folder)), ["Picks", "First"]);
    let other = library.create_generator("Other", &steps, true).unwrap();
    library
        .place_generator(other, Some(folder), Some(1))
        .unwrap();
    assert_eq!(sidebar(&library, Some(folder)), ["Picks", "Other", "First"]);
    // Deleting the folder keeps their order, after what is at the top.
    library.delete_playlist(folder).unwrap();
    let top = sidebar(&library, None);
    assert_eq!(&top[top.len() - 3..], ["Picks", "Other", "First"]);
    assert!(matches!(
        library.place_generator(picks, Some(last), None),
        Err(GeneratorError::Unknown(_))
    ));
}
