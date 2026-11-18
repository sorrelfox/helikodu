// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Statistics, ratings and history through the public API, and the identity that keeps
//! them across moves.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use helikodu_library::{
    DbError, HistoryQuery, HistoryTotals, Listen, ListenEnd, ListenOutcome, PlayRules, Rating,
    TrackChange, TrackFingerprint, TrackKind, TrackStats,
};
use helikodu_model::library::TrackId;
use support::{library, record, tagged};

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn listen(started: u64, played_secs: u64, end: ListenEnd) -> Listen {
    Listen {
        started_at: at(started),
        played: Duration::from_secs(played_secs),
        end,
        position: None,
        context: None,
    }
}

#[test]
fn plays_and_skips_are_counted_by_the_rules() {
    let (library, root) = library();
    // 200 s long: a play from 100 s.
    let track = library
        .upsert_track(record(root, "/music/a.flac", "a"))
        .unwrap()
        .track;
    let rules = PlayRules::default();
    let outcomes: Vec<ListenOutcome> = [
        listen(1000, 150, ListenEnd::Finished),
        listen(2000, 30, ListenEnd::Skipped),
        listen(3000, 100, ListenEnd::Skipped),
        listen(4000, 20, ListenEnd::Stopped),
        listen(5000, 5, ListenEnd::Finished),
    ]
    .into_iter()
    .map(|l| library.record_listen(track, l, rules).unwrap())
    .collect();
    assert_eq!(
        outcomes,
        [
            ListenOutcome::Play,
            ListenOutcome::Skip,
            ListenOutcome::Play,
            ListenOutcome::Partial,
            ListenOutcome::Partial,
        ]
    );
    let stats = library.stats(track).unwrap();
    assert_eq!(stats.play_count, 2);
    assert_eq!(stats.skip_count, 1);
    assert_eq!(stats.first_played, Some(at(1000)));
    assert_eq!(stats.last_played, Some(at(3000)));
    assert_eq!(stats.last_skipped, Some(at(2000)));
    assert_eq!(stats.rating, None);

    // A listen reported late does not move the last played time back.
    library
        .record_listen(track, listen(500, 200, ListenEnd::Finished), rules)
        .unwrap();
    let stats = library.stats(track).unwrap();
    assert_eq!(stats.play_count, 3);
    assert_eq!(stats.first_played, Some(at(1000)));
    assert_eq!(stats.last_played, Some(at(3000)));
}

#[test]
fn a_new_track_has_empty_statistics() {
    let (library, root) = library();
    let track = library
        .upsert_track(record(root, "/music/a.flac", "a"))
        .unwrap()
        .track;
    assert_eq!(library.stats(track).unwrap(), TrackStats::default());
}

#[test]
fn rating_round_trip_including_not_rated() {
    let (library, root) = library();
    let track = library
        .upsert_track(record(root, "/music/a.flac", "a"))
        .unwrap()
        .track;
    for half_stars in 0..=10 {
        let rating = Rating::from_half_stars(half_stars);
        library.set_rating(track, rating).unwrap();
        assert_eq!(library.stats(track).unwrap().rating, rating);
    }
    // Zero stars is a rating; None is not.
    library.set_rating(track, Rating::from_stars(0)).unwrap();
    assert_eq!(library.stats(track).unwrap().rating, Rating::from_stars(0));
    library.set_rating(track, None).unwrap();
    assert_eq!(library.stats(track).unwrap().rating, None);
}

#[test]
fn the_rating_from_tags_is_taken_once() {
    let (library, root) = library();
    let mut tagged_record = record(root, "/music/a.flac", "a");
    tagged_record.rating_from_tags = Rating::from_stars(4);
    let track = library.upsert_track(tagged_record.clone()).unwrap().track;
    assert_eq!(library.stats(track).unwrap().rating, Rating::from_stars(4));

    // The user changes it; a rescan with the old tag does not overwrite it.
    library
        .set_rating(track, Rating::from_half_stars(5))
        .unwrap();
    library.upsert_track(tagged_record.clone()).unwrap();
    assert_eq!(
        library.stats(track).unwrap().rating,
        Rating::from_half_stars(5)
    );

    // Cleared ratings stay cleared, too.
    library.set_rating(track, None).unwrap();
    library.upsert_track(tagged_record).unwrap();
    assert_eq!(library.stats(track).unwrap().rating, None);
}

/// A track with some statistics; returns it with its identity.
fn played_track(
    library: &helikodu_library::Library,
    root: helikodu_library::RootId,
) -> (TrackId, helikodu_library::IdentityId) {
    let write = library
        .upsert_track(tagged(
            root,
            "/music/old/a.flac",
            "content a",
            "Song",
            "Band",
            "Album",
        ))
        .unwrap();
    library
        .record_listen(
            write.track,
            listen(1000, 200, ListenEnd::Finished),
            PlayRules::default(),
        )
        .unwrap();
    library
        .set_rating(write.track, Rating::from_stars(5))
        .unwrap();
    (write.track, write.identity)
}

fn assert_kept(library: &helikodu_library::Library, track: TrackId) {
    let stats = library.stats(track).unwrap();
    assert_eq!(stats.play_count, 1);
    assert_eq!(stats.rating, Rating::from_stars(5));
    let history = library
        .history(&HistoryQuery {
            track: Some(track),
            ..HistoryQuery::default()
        })
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].track, Some(track));
}

#[test]
fn statistics_survive_a_watched_move() {
    let (library, root) = library();
    let (track, identity) = played_track(&library, root);
    library
        .move_file(root, "/music/old/a.flac", "/music/new/b.flac")
        .unwrap();
    assert_eq!(
        library.track_at("/music/new/b.flac".as_ref(), 0).unwrap(),
        Some(track)
    );
    assert_eq!(
        library.track_at("/music/old/a.flac".as_ref(), 0).unwrap(),
        None
    );
    assert_eq!(library.identity_of(track).unwrap(), identity);
    assert_kept(&library, track);
}

#[test]
fn statistics_survive_a_file_that_disappears_and_reappears_elsewhere() {
    let (library, root) = library();
    let (track, identity) = played_track(&library, root);
    library
        .mark_file_missing("/music/old/a.flac", at(5000))
        .unwrap();
    // The scanner finds the same content under a new name.
    let write = library
        .upsert_track(tagged(
            root,
            "/music/new/b.flac",
            "content a",
            "Song",
            "Band",
            "Album",
        ))
        .unwrap();
    assert_eq!(write.change, TrackChange::Moved);
    assert_eq!(write.track, track);
    assert_eq!(write.identity, identity);
    assert_kept(&library, track);
}

#[test]
fn statistics_come_back_with_a_track_that_left_the_library() {
    let (library, root) = library();
    let (track, identity) = played_track(&library, root);
    assert_eq!(library.remove_file("/music/old/a.flac").unwrap(), 1);
    assert!(matches!(
        library.stats(track),
        Err(DbError::UnknownTrack(_))
    ));

    let write = library
        .upsert_track(tagged(
            root,
            "/music/new/b.flac",
            "content a",
            "Song",
            "Band",
            "Album",
        ))
        .unwrap();
    assert_eq!(write.change, TrackChange::Returned);
    assert_eq!(write.identity, identity);
    assert_kept(&library, write.track);
    assert_eq!(
        library
            .identities_with(&TrackFingerprint::of(&tagged(
                root,
                "/x/y.flac",
                "content a",
                "",
                "",
                ""
            )))
            .unwrap(),
        [identity]
    );
}

#[test]
fn a_re_encoded_file_gets_a_new_identity() {
    let (library, root) = library();
    let (track, identity) = played_track(&library, root);
    library.remove_track(track).unwrap();
    let write = library
        .upsert_track(tagged(
            root,
            "/music/old/a.flac",
            "re-encoded",
            "Song",
            "Band",
            "Album",
        ))
        .unwrap();
    assert_eq!(write.change, TrackChange::Added);
    assert_ne!(write.identity, identity);
    assert_eq!(library.stats(write.track).unwrap(), TrackStats::default());
}

#[test]
fn tracks_of_one_file_have_their_own_identities() {
    let (library, root) = library();
    let mut first = record(root, "/music/album.flac", "cue");
    first.sub_index = 1;
    first.frames = Some((0, Some(44_100 * 200)));
    let mut second = first.clone();
    second.sub_index = 2;
    second.frames = Some((44_100 * 200, None));
    let written = library.upsert_tracks(vec![first, second]).unwrap();
    assert_ne!(written[0].identity, written[1].identity);
    library
        .record_listen(
            written[1].track,
            listen(1, 200, ListenEnd::Finished),
            PlayRules::default(),
        )
        .unwrap();
    assert_eq!(library.stats(written[0].track).unwrap().play_count, 0);
    assert_eq!(library.stats(written[1].track).unwrap().play_count, 1);

    // Moving the file moves both, with their statistics.
    library
        .move_file(root, "/music/album.flac", "/music/moved/album.flac")
        .unwrap();
    assert_eq!(
        library
            .track_at("/music/moved/album.flac".as_ref(), 2)
            .unwrap(),
        Some(written[1].track)
    );
    assert_eq!(library.stats(written[1].track).unwrap().play_count, 1);
}

#[test]
fn history_is_appended_and_queried_newest_first() {
    let (library, root) = library();
    let a = library
        .upsert_track(record(root, "/music/a.flac", "a"))
        .unwrap()
        .track;
    let b = library
        .upsert_track(record(root, "/music/b.flac", "b"))
        .unwrap()
        .track;
    let rules = PlayRules::default();
    let mut with_context = listen(100, 200, ListenEnd::Finished);
    with_context.context = Some("album:Homogenic".to_owned());
    library.record_listen(a, with_context, rules).unwrap();
    library
        .record_listen(b, listen(200, 10, ListenEnd::Skipped), rules)
        .unwrap();
    library
        .record_listen(a, listen(300, 150, ListenEnd::Skipped), rules)
        .unwrap();
    // Nothing was played: no history entry, but still a skip.
    library
        .record_listen(b, listen(400, 0, ListenEnd::Skipped), rules)
        .unwrap();

    let all = library.history(&HistoryQuery::default()).unwrap();
    let summary: Vec<(TrackId, u64, bool)> = all
        .iter()
        .map(|entry| {
            (
                entry.track.unwrap(),
                entry
                    .started_at
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                entry.counted,
            )
        })
        .collect();
    assert_eq!(summary, [(a, 300, true), (b, 200, false), (a, 100, true)]);
    assert_eq!(all[2].context.as_deref(), Some("album:Homogenic"));
    assert_eq!(all[2].played, Duration::from_secs(200));
    assert_eq!(library.stats(b).unwrap().skip_count, 2);

    let plays_of_a = library
        .history(&HistoryQuery {
            track: Some(a),
            plays_only: true,
            since: Some(at(200)),
            ..HistoryQuery::default()
        })
        .unwrap();
    assert_eq!(plays_of_a.len(), 1);
    assert_eq!(plays_of_a[0].started_at, at(300));
    let limited = library
        .history(&HistoryQuery {
            until: Some(at(300)),
            limit: 1,
            ..HistoryQuery::default()
        })
        .unwrap();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].started_at, at(200));

    let totals = library.history_totals().unwrap();
    assert_eq!((totals.plays, totals.skips), (2, 1));
    assert_eq!(totals.first, Some(at(100)));
}

#[test]
fn an_empty_history_has_no_totals() {
    let (library, _) = library();
    assert_eq!(library.history_totals().unwrap(), HistoryTotals::default());
}

#[test]
fn long_tracks_and_audiobooks_remember_where_listening_stopped() {
    let (library, root) = library();
    let mut book = record(root, "/music/book.m4b", "book");
    book.kind = TrackKind::Audiobook;
    let book = library.upsert_track(book).unwrap().track;
    let song = library
        .upsert_track(record(root, "/music/song.flac", "song"))
        .unwrap()
        .track;
    let rules = PlayRules::default();
    let stopped_at = |position| Listen {
        position: Some(Duration::from_secs(position)),
        ..listen(1, 60, ListenEnd::Stopped)
    };
    library.record_listen(book, stopped_at(61), rules).unwrap();
    library.record_listen(song, stopped_at(61), rules).unwrap();
    assert_eq!(
        library.stats(book).unwrap().resume_position,
        Some(Duration::from_secs(61))
    );
    assert_eq!(library.stats(song).unwrap().resume_position, None);

    library
        .record_listen(book, listen(2, 200, ListenEnd::Finished), rules)
        .unwrap();
    assert_eq!(library.stats(book).unwrap().resume_position, None);

    library
        .set_resume_position(book, Some(Duration::from_millis(1500)))
        .unwrap();
    assert_eq!(
        library.stats(book).unwrap().resume_position,
        Some(Duration::from_millis(1500))
    );
}

#[test]
fn unknown_tracks_are_errors() {
    let (library, root) = library();
    let missing = TrackId::new(42);
    assert!(matches!(library.stats(missing), Err(DbError::UnknownTrack(t)) if t == missing));
    assert!(matches!(
        library.set_rating(missing, None),
        Err(DbError::UnknownTrack(_))
    ));
    assert!(matches!(
        library.record_listen(
            missing,
            listen(1, 1, ListenEnd::Finished),
            PlayRules::default()
        ),
        Err(DbError::UnknownTrack(_))
    ));
    assert!(matches!(
        library.move_file(root, "/a/b", "/c/d"),
        Err(DbError::UnknownPath(_))
    ));
}
