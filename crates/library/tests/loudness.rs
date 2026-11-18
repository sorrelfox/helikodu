// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Measured loudness through the public API: stored and read back, kept apart from
//! the tag values, stale after the file changes, and the tracks still to measure.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::time::{Duration, SystemTime};

use helikodu_library::{
    AlbumMeasurement, LoudnessCoverage, LoudnessSelection, MeasuredLoudness, ReplayGain,
};
use helikodu_model::library::TrackId;
use support::{library, record, tagged};

fn measurement(track: TrackId, lufs: f64, album: Option<AlbumMeasurement>) -> MeasuredLoudness {
    MeasuredLoudness {
        track,
        version: 1,
        integrated_lufs: Some(lufs),
        range_lu: 5.5,
        sample_peak: 0.891,
        true_peak: 0.977,
        album,
    }
}

const ALBUM: AlbumMeasurement = AlbumMeasurement {
    integrated_lufs: Some(-12.25),
    range_lu: 7.0,
    sample_peak: 0.99,
    true_peak: 1.02,
};

#[test]
fn measurements_round_trip_apart_from_tags() {
    let (library, root) = library();
    let mut tracks = Vec::new();
    for (n, name) in ["one", "two", "three"].iter().enumerate() {
        let mut track = tagged(
            root,
            &format!("/music/r/{name}.flac"),
            name,
            name,
            "Band",
            "Record",
        );
        track.tags.track_no = Some(n as u32 + 1);
        track.replay_gain = ReplayGain {
            track_gain_db: Some(-3.0),
            ..ReplayGain::default()
        };
        tracks.push(library.upsert_track(track).unwrap().track);
    }
    let single = library
        .upsert_track(record(root, "/music/single.flac", "single"))
        .unwrap()
        .track;

    assert_eq!(
        library.loudness_coverage().unwrap(),
        LoudnessCoverage {
            tracks: 4,
            with_values: 3,
            measured: 0,
        }
    );
    let targets = library
        .loudness_targets(&LoudnessSelection::Unmeasured { version: 1 })
        .unwrap();
    let order: Vec<TrackId> = targets.iter().map(|t| t.track).collect();
    assert_eq!(order, [tracks[0], tracks[1], tracks[2], single]);
    assert!(
        targets[..3]
            .iter()
            .all(|t| t.album.is_some() && t.album_size == 3)
    );
    assert_eq!(targets[3].album, None);
    assert_eq!(targets[3].album_size, 0);
    assert!(targets.iter().all(|t| t.whole_file));
    assert_eq!(
        targets[3].source.path,
        std::path::Path::new("/music/single.flac")
    );

    let before = SystemTime::now() - Duration::from_secs(1);
    let mut stored: Vec<MeasuredLoudness> = tracks
        .iter()
        .map(|t| measurement(*t, -11.0, Some(ALBUM)))
        .collect();
    stored.push(MeasuredLoudness {
        integrated_lufs: None,
        ..measurement(single, 0.0, None)
    });
    library.store_loudness(stored.clone()).unwrap();

    let back = library.measured_loudness(tracks[1]).unwrap().unwrap();
    assert_eq!(back.measured, stored[1]);
    assert!(back.current);
    assert!(back.measured_at >= before);
    assert_eq!(back.measured.track_gain_db(), Some(-7.0));
    assert_eq!(back.measured.album.unwrap().gain_db(), Some(-5.75));
    let silent = library.measured_loudness(single).unwrap().unwrap();
    assert_eq!(silent.measured.integrated_lufs, None);
    assert_eq!(silent.measured.album, None);
    assert_eq!(library.measured_loudness(TrackId::new(999)).unwrap(), None);

    // The tag values stay as the scan stored them.
    let id = i64::try_from(tracks[1].get()).unwrap();
    let tagged_gain: Option<f64> = library
        .database()
        .read(|c| {
            c.query_row(
                "SELECT rg_track_gain FROM tracks WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(tagged_gain, Some(-3.0));

    assert_eq!(
        library.loudness_coverage().unwrap(),
        LoudnessCoverage {
            tracks: 4,
            with_values: 3,
            measured: 4,
        }
    );
    let none = LoudnessSelection::Unmeasured { version: 1 };
    assert!(library.loudness_targets(&none).unwrap().is_empty());
    let newer = LoudnessSelection::Unmeasured { version: 2 };
    assert_eq!(library.loudness_targets(&newer).unwrap().len(), 4);

    // Measuring again replaces the earlier values.
    library
        .store_loudness(vec![measurement(single, -20.0, None)])
        .unwrap();
    let single_back = library.measured_loudness(single).unwrap().unwrap();
    assert_eq!(single_back.measured.integrated_lufs, Some(-20.0));
}

#[test]
fn a_changed_file_makes_its_measurement_stale() {
    let (library, root) = library();
    let a = library
        .upsert_track(tagged(root, "/music/r/a.flac", "a", "A", "Band", "Record"))
        .unwrap()
        .track;
    let b = library
        .upsert_track(tagged(root, "/music/r/b.flac", "b", "B", "Band", "Record"))
        .unwrap()
        .track;
    library
        .store_loudness(vec![
            measurement(a, -10.0, Some(ALBUM)),
            measurement(b, -14.0, Some(ALBUM)),
        ])
        .unwrap();
    let unmeasured = LoudnessSelection::Unmeasured { version: 1 };
    assert!(library.loudness_targets(&unmeasured).unwrap().is_empty());

    // The file of `a` is written again (its tags changed, say): a new stamp.
    let mut changed = tagged(root, "/music/r/a.flac", "a-changed", "A", "Band", "Record");
    changed.file.mtime_ns = Some(2);
    assert_eq!(library.upsert_track(changed).unwrap().track, a);
    let stale = library.measured_loudness(a).unwrap().unwrap();
    assert!(!stale.current);
    assert!(library.measured_loudness(b).unwrap().unwrap().current);
    // Both tracks of the album are measured again, for the album values.
    let targets: Vec<TrackId> = library
        .loudness_targets(&unmeasured)
        .unwrap()
        .iter()
        .map(|t| t.track)
        .collect();
    assert_eq!(targets, [a, b]);
    assert_eq!(library.loudness_coverage().unwrap().measured, 1);

    // After writing the tags itself, the scanner takes the new stamp.
    library.restamp_loudness(vec![a]).unwrap();
    assert!(library.measured_loudness(a).unwrap().unwrap().current);
    assert!(library.loudness_targets(&unmeasured).unwrap().is_empty());
}

#[test]
fn selections_of_tracks_and_albums() {
    let (library, root) = library();
    let a = library
        .upsert_track(tagged(root, "/music/r/a.flac", "a", "A", "Band", "Record"))
        .unwrap()
        .track;
    let b = library
        .upsert_track(tagged(root, "/music/r/b.flac", "b", "B", "Band", "Record"))
        .unwrap()
        .track;
    let other = library
        .upsert_track(tagged(root, "/music/o/c.flac", "c", "C", "Band", "Other"))
        .unwrap()
        .track;
    let picked = library
        .loudness_targets(&LoudnessSelection::Tracks(vec![b, b, TrackId::new(999)]))
        .unwrap();
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].track, b);
    assert_eq!(picked[0].album_size, 2);
    let album = picked[0].album.unwrap();

    let whole = library
        .loudness_targets(&LoudnessSelection::Albums(vec![album]))
        .unwrap();
    let tracks: Vec<TrackId> = whole.iter().map(|t| t.track).collect();
    assert_eq!(tracks, [a, b]);
    assert!(!tracks.contains(&other));

    // A missing file is left out.
    library
        .mark_file_missing("/music/r/a.flac", SystemTime::now())
        .unwrap();
    let whole = library
        .loudness_targets(&LoudnessSelection::Albums(vec![album]))
        .unwrap();
    assert_eq!(whole.len(), 1);
    assert_eq!(whole[0].album_size, 1);
}
