// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The equalizer of a track: stored, read back, replaced and removed, and kept
//! with the track's identity.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use helikodu_library::DbError;
use helikodu_model::library::TrackId;
use helikodu_model::settings::{EqMode, EqualizerSettings, FilterKind, ParametricFilter};
use support::{library, record};

#[test]
fn a_track_equalizer_round_trips() {
    let (library, root) = library();
    let track = library
        .upsert_track(record(root, "/music/a.flac", "a"))
        .unwrap()
        .track;
    let other = library
        .upsert_track(record(root, "/music/b.flac", "b"))
        .unwrap()
        .track;
    assert_eq!(library.track_equalizer(track).unwrap(), None);
    let mut eq = EqualizerSettings {
        enabled: true,
        mode: EqMode::Parametric,
        ..EqualizerSettings::default()
    };
    eq.parametric = vec![ParametricFilter {
        kind: FilterKind::LowShelf,
        frequency_hz: 105.0,
        gain_db: -3.5,
        q: 0.7,
        enabled: true,
    }];
    library.set_track_equalizer(track, Some(&eq)).unwrap();
    assert_eq!(library.track_equalizer(track).unwrap(), Some(eq.clone()));
    assert_eq!(library.track_equalizer(other).unwrap(), None);
    assert_eq!(library.tracks_with_equalizer().unwrap(), [track]);
    // Replaced, then removed.
    eq.mode = EqMode::Graphic10;
    eq.graphic_10[2] = 4.0;
    library.set_track_equalizer(track, Some(&eq)).unwrap();
    assert_eq!(library.track_equalizer(track).unwrap(), Some(eq));
    library.set_track_equalizer(track, None).unwrap();
    assert_eq!(library.track_equalizer(track).unwrap(), None);
    assert!(library.tracks_with_equalizer().unwrap().is_empty());
    assert!(matches!(
        library.set_track_equalizer(TrackId::new(9_999), None),
        Err(DbError::UnknownTrack(_))
    ));
}

#[test]
fn a_track_equalizer_keeps_its_name() {
    let (library, root) = library();
    let track = library
        .upsert_track(record(root, "/music/a.flac", "a"))
        .unwrap()
        .track;
    let other = library
        .upsert_track(record(root, "/music/b.flac", "b"))
        .unwrap()
        .track;
    assert!(!library.any_track_equalizer(&[track, other]).unwrap());
    let eq = EqualizerSettings {
        enabled: true,
        ..EqualizerSettings::default()
    };
    library
        .set_named_track_equalizer(track, Some(&eq), Some("Bright master fix"))
        .unwrap();
    let named = library.named_track_equalizer(track).unwrap().unwrap();
    assert_eq!(named.name.as_deref(), Some("Bright master fix"));
    assert_eq!(named.settings, eq);
    assert!(library.any_track_equalizer(&[other, track]).unwrap());
    assert!(!library.any_track_equalizer(&[other]).unwrap());
    // Set without a name, it has none.
    library.set_track_equalizer(track, Some(&eq)).unwrap();
    assert_eq!(
        library.named_track_equalizer(track).unwrap().unwrap().name,
        None
    );
    library.set_track_equalizer(track, None).unwrap();
    assert!(!library.any_track_equalizer(&[track]).unwrap());
}
