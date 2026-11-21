// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track analysis through the public API: stored and read back with its
//! envelope and silences, failures kept, stale after the file changes, a newer
//! version making every track pending, and the units the background job works in.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::time::{Duration, SystemTime};

use helikodu_library::{
    AnalysisCoverage, AnalysisOutcome, AnalysisPlan, Library, MeasuredLoudness, ReplayGain,
    SimilaritySeed,
};
use helikodu_model::analysis::{
    LoudnessEnvelope, SOUND_FEATURES_LEN, SilentSpan, SoundFeatures, TrackAnalysis,
};
use helikodu_model::library::TrackId;
use support::{library, record, tagged};

fn analysis() -> TrackAnalysis {
    TrackAnalysis {
        rate: 44_100,
        frames: 44_100 * 200,
        audio: Some(22_050..44_100 * 198),
        silences: vec![SilentSpan {
            start: 44_100 * 60,
            end: 44_100 * 63,
        }],
        integrated_lufs: Some(-12.5),
        head_lufs: Some(-20.0),
        tail_lufs: Some(-30.5),
        fade_in_end: Some(44_100 * 2),
        fade_out_start: Some(44_100 * 190),
        envelope: LoudnessEnvelope::from_lufs(1, (0..200).map(|s| -10.0 - f64::from(s) / 10.0)),
        features: Some(Box::new(sounding(0.0))),
    }
}

/// Sound features all at `level`.
fn sounding(level: f32) -> SoundFeatures {
    SoundFeatures([level; SOUND_FEATURES_LEN])
}

fn plan(loudness: Option<u32>) -> AnalysisPlan {
    AnalysisPlan {
        version: 1,
        loudness_version: loudness,
        max_album_tracks: 50,
    }
}

fn album(library: &Library, root: helikodu_library::RootId, n: usize) -> Vec<TrackId> {
    (0..n)
        .map(|i| {
            let mut track = tagged(
                root,
                &format!("/music/r/{i}.flac"),
                &format!("content-{i}"),
                &format!("T{i}"),
                "Band",
                "Record",
            );
            track.tags.track_no = Some(i as u32 + 1);
            library.upsert_track(track).unwrap().track
        })
        .collect()
}

#[test]
fn analyses_and_failures_round_trip() {
    let (library, root) = library();
    let tracks = album(&library, root, 3);
    let lone = library
        .upsert_track(record(root, "/music/lone.flac", "lone"))
        .unwrap()
        .track;
    assert_eq!(
        library.analysis_coverage(1).unwrap(),
        AnalysisCoverage {
            tracks: 4,
            analyzed: 0,
            failed: 0
        }
    );
    let all = library.tracks_to_analyze(1, None, 10).unwrap();
    assert_eq!(all, [tracks[0], tracks[1], tracks[2], lone]);
    // Pages after a track, in id order.
    assert_eq!(
        library.tracks_to_analyze(1, Some(tracks[0]), 2).unwrap(),
        [tracks[1], tracks[2]]
    );

    let before = SystemTime::now() - Duration::from_secs(1);
    library
        .store_analysis(tracks[0], 1, AnalysisOutcome::Analyzed(analysis()))
        .unwrap();
    library
        .store_analysis(lone, 1, AnalysisOutcome::Failed("cannot decode".into()))
        .unwrap();
    let silent = TrackAnalysis {
        rate: 48_000,
        frames: 48_000,
        envelope: LoudnessEnvelope::from_lufs(1, [f64::NEG_INFINITY]),
        ..TrackAnalysis::default()
    };
    library
        .store_analysis(tracks[1], 1, AnalysisOutcome::Analyzed(silent.clone()))
        .unwrap();

    let back = library.track_analysis(tracks[0]).unwrap().unwrap();
    assert_eq!(back.track, tracks[0]);
    assert_eq!(back.version, 1);
    assert!(back.current);
    assert!(back.analyzed_at >= before);
    assert_eq!(back.outcome, AnalysisOutcome::Analyzed(analysis()));
    let back = library.track_analysis(tracks[1]).unwrap().unwrap();
    assert_eq!(back.outcome, AnalysisOutcome::Analyzed(silent));
    let failed = library.track_analysis(lone).unwrap().unwrap();
    assert_eq!(
        failed.outcome,
        AnalysisOutcome::Failed("cannot decode".into())
    );
    assert_eq!(library.track_analysis(tracks[2]).unwrap(), None);
    assert_eq!(library.track_analysis(TrackId::new(999)).unwrap(), None);

    // A failure is not tried again: only the track never analysed is left.
    assert_eq!(library.tracks_to_analyze(1, None, 10).unwrap(), [tracks[2]]);
    assert_eq!(
        library.analysis_coverage(1).unwrap(),
        AnalysisCoverage {
            tracks: 4,
            analyzed: 2,
            failed: 1
        }
    );
    assert_eq!(library.analysis_coverage(1).unwrap().remaining(), 1);

    // Analysing again replaces the earlier result.
    library
        .store_analysis(lone, 1, AnalysisOutcome::Analyzed(analysis()))
        .unwrap();
    assert_eq!(library.analysis_coverage(1).unwrap().failed, 0);
}

#[test]
fn a_changed_file_makes_its_analysis_stale() {
    let (library, root) = library();
    let tracks = album(&library, root, 2);
    for track in &tracks {
        library
            .store_analysis(*track, 1, AnalysisOutcome::Analyzed(analysis()))
            .unwrap();
    }
    assert!(library.tracks_to_analyze(1, None, 10).unwrap().is_empty());

    let mut changed = tagged(root, "/music/r/0.flac", "content-0", "T0", "Band", "Record");
    changed.tags.track_no = Some(1);
    changed.file.mtime_ns = Some(2);
    assert_eq!(library.upsert_track(changed).unwrap().track, tracks[0]);

    let stale = library.track_analysis(tracks[0]).unwrap().unwrap();
    assert!(!stale.current);
    assert!(library.track_analysis(tracks[1]).unwrap().unwrap().current);
    assert_eq!(library.tracks_to_analyze(1, None, 10).unwrap(), [tracks[0]]);
    assert_eq!(library.analysis_coverage(1).unwrap().analyzed, 1);
}

#[test]
fn a_newer_version_makes_every_track_pending() {
    let (library, root) = library();
    let tracks = album(&library, root, 2);
    for track in &tracks {
        library
            .store_analysis(*track, 1, AnalysisOutcome::Failed("broken".into()))
            .unwrap();
    }
    assert!(library.tracks_to_analyze(1, None, 10).unwrap().is_empty());
    assert_eq!(library.tracks_to_analyze(2, None, 10).unwrap(), tracks);
    assert_eq!(library.analysis_coverage(2).unwrap().remaining(), 2);
    let newer = AnalysisPlan {
        version: 2,
        ..plan(None)
    };
    assert!(library.analysis_unit(tracks[0], &newer).unwrap().is_some());
}

#[test]
fn units_take_whole_albums_for_their_loudness() {
    let (library, root) = library();
    let tracks = album(&library, root, 3);
    let lone = library
        .upsert_track(record(root, "/music/lone.flac", "lone"))
        .unwrap()
        .track;

    // No track has loudness values: the whole album, measured.
    let unit = library
        .analysis_unit(tracks[1], &plan(Some(1)))
        .unwrap()
        .unwrap();
    assert!(unit.album.is_some());
    assert!(unit.measure_loudness);
    let order: Vec<TrackId> = unit.targets.iter().map(|t| t.track).collect();
    assert_eq!(order, tracks);
    assert!(unit.targets.iter().all(|t| t.needs_analysis));
    assert_eq!(
        unit.targets[0].source.path,
        std::path::Path::new("/music/r/0.flac")
    );
    let lone_unit = library
        .analysis_unit(lone, &plan(Some(1)))
        .unwrap()
        .unwrap();
    assert!(lone_unit.measure_loudness);
    assert_eq!(lone_unit.album, None);

    // Stopped half way: the album is still taken whole, for its loudness.
    for track in &tracks[..2] {
        library
            .store_analysis(*track, 1, AnalysisOutcome::Analyzed(analysis()))
            .unwrap();
    }
    assert_eq!(
        library.analysis_unit(tracks[0], &plan(Some(1))).unwrap(),
        None
    );
    let unit = library
        .analysis_unit(tracks[2], &plan(Some(1)))
        .unwrap()
        .unwrap();
    assert!(unit.measure_loudness);
    let needs: Vec<bool> = unit.targets.iter().map(|t| t.needs_analysis).collect();
    assert_eq!(needs, [false, false, true]);

    // Without the loudness, or for a larger album, only the tracks that need it.
    let unit = library
        .analysis_unit(tracks[2], &plan(None))
        .unwrap()
        .unwrap();
    assert!(!unit.measure_loudness);
    assert_eq!(unit.targets.len(), 1);
    let small = AnalysisPlan {
        max_album_tracks: 2,
        ..plan(Some(1))
    };
    let unit = library.analysis_unit(tracks[2], &small).unwrap().unwrap();
    assert!(!unit.measure_loudness);
    assert_eq!(unit.targets.len(), 1);

    // A measured track of the album: its loudness is left to the scanner.
    library
        .store_loudness(vec![MeasuredLoudness {
            track: tracks[0],
            version: 1,
            integrated_lufs: Some(-10.0),
            range_lu: 3.0,
            sample_peak: 0.9,
            true_peak: 0.95,
            album: None,
        }])
        .unwrap();
    let unit = library
        .analysis_unit(tracks[2], &plan(Some(1)))
        .unwrap()
        .unwrap();
    assert!(!unit.measure_loudness);
    assert_eq!(unit.targets.len(), 1);
    // An older measurement does not count.
    let unit = library
        .analysis_unit(tracks[2], &plan(Some(2)))
        .unwrap()
        .unwrap();
    assert!(unit.measure_loudness);

    // Gains in the tags count as loudness values.
    let mut tagged_lone = record(root, "/music/lone.flac", "lone");
    tagged_lone.replay_gain = ReplayGain {
        track_gain_db: Some(-4.0),
        ..ReplayGain::default()
    };
    library.upsert_track(tagged_lone).unwrap();
    let lone_unit = library
        .analysis_unit(lone, &plan(Some(1)))
        .unwrap()
        .unwrap();
    assert!(!lone_unit.measure_loudness);
}

#[test]
fn stored_features_make_tracks_comparable() {
    let (library, root) = library();
    let tracks = album(&library, root, 3);
    let lone = library
        .upsert_track(record(root, "/music/lone.flac", "lone"))
        .unwrap()
        .track;
    // Nothing analysed: nothing to compare.
    assert_eq!(library.track_similarity(tracks[0], lone).unwrap(), None);
    assert!(
        library
            .similar_tracks(SimilaritySeed::Track(lone), 5)
            .unwrap()
            .is_empty()
    );

    let with = |level: f32| TrackAnalysis {
        features: Some(Box::new(sounding(level))),
        ..analysis()
    };
    library
        .store_analysis(tracks[0], 2, AnalysisOutcome::Analyzed(with(0.0)))
        .unwrap();
    library
        .store_analysis(tracks[1], 2, AnalysisOutcome::Analyzed(with(0.1)))
        .unwrap();
    library
        .store_analysis(lone, 2, AnalysisOutcome::Analyzed(with(5.0)))
        .unwrap();
    // The third track is analysed but silent: no features.
    library
        .store_analysis(
            tracks[2],
            2,
            AnalysisOutcome::Analyzed(TrackAnalysis {
                features: None,
                ..analysis()
            }),
        )
        .unwrap();
    assert_eq!(
        library.track_analysis(tracks[1]).unwrap().unwrap().outcome,
        AnalysisOutcome::Analyzed(with(0.1))
    );

    let near = library
        .track_similarity(tracks[0], tracks[1])
        .unwrap()
        .unwrap();
    let far = library.track_similarity(tracks[0], lone).unwrap().unwrap();
    assert!(near > far, "{near} {far}");
    assert!((0.0..=1.0).contains(&far) && near <= 1.0);
    assert_eq!(
        library.track_similarity(tracks[0], tracks[2]).unwrap(),
        None
    );

    let found = library
        .similar_tracks(SimilaritySeed::Track(tracks[0]), 5)
        .unwrap();
    let order: Vec<TrackId> = found.iter().map(|s| s.track).collect();
    assert_eq!(order, [tracks[1], lone]);

    // An album leaves out its own tracks.
    let album_id = library.play_keys(&[tracks[0]]).unwrap()[&tracks[0]]
        .album
        .unwrap();
    let found = library
        .similar_tracks(SimilaritySeed::Album(album_id), 5)
        .unwrap();
    let order: Vec<TrackId> = found.iter().map(|s| s.track).collect();
    assert_eq!(order, [lone]);
    let artist = library.play_keys(&[tracks[0]]).unwrap()[&tracks[0]]
        .artist
        .unwrap();
    let found = library
        .similar_tracks(SimilaritySeed::Artist(artist), 5)
        .unwrap();
    assert_eq!(found.len(), 1);

    // A changed file drops out until it is analysed again.
    let mut changed = tagged(root, "/music/r/1.flac", "content-1", "T1", "Band", "Record");
    changed.tags.track_no = Some(2);
    changed.file.mtime_ns = Some(2);
    library.upsert_track(changed).unwrap();
    library
        .store_analysis(tracks[2], 2, AnalysisOutcome::Failed("x".into()))
        .unwrap();
    assert_eq!(
        library.track_similarity(tracks[0], tracks[1]).unwrap(),
        None
    );
}
