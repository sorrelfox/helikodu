// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Formats lofty lacks, read through FFmpeg.

use helikodu_meta::tags::{ReadOptions, TagSource, read_tags};

use crate::media::{encode, metadata};

const ALBUM_ID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

#[test]
fn matroska_audio() {
    let args = metadata(&[
        ("title", "Windowlicker"),
        ("artist", "Aphex Twin"),
        ("album", "Windowlicker"),
        ("album_artist", "Aphex Twin"),
        ("date", "1999"),
        ("genre", "Electronic"),
        ("track", "1"),
        ("MUSICBRAINZ_ALBUMID", ALBUM_ID),
        ("REPLAYGAIN_TRACK_GAIN", "-9.10 dB"),
        ("RATING", "4"),
    ]);
    let Some(path) = encode("fallback.mka", "flac", &args, "matroska") else {
        return;
    };
    let file = read_tags(&path, &ReadOptions::default()).expect("read");
    assert_eq!(file.source, TagSource::Ffmpeg);
    assert!(file.fallback_reason.is_some());
    let t = &file.tags;
    assert_eq!(t.title.as_deref(), Some("Windowlicker"));
    assert_eq!(t.artist.as_deref(), Some("Aphex Twin"));
    assert_eq!(t.album_artist.as_deref(), Some("Aphex Twin"));
    assert_eq!(t.year(), Some(1999));
    assert_eq!(t.genres, ["Electronic"]);
    assert_eq!(t.track_number, Some(1));
    assert_eq!(t.musicbrainz.release.as_deref(), Some(ALBUM_ID));
    assert_eq!(t.replay_gain.track_gain_db, Some(-9.1));
    assert_eq!(t.rating.map(|r| r.half_stars()), Some(8));
    let a = &file.audio;
    assert_eq!(a.codec.as_deref(), Some("flac"));
    assert_eq!(a.container.as_deref(), Some("matroska"));
    assert_eq!(a.sample_rate, Some(44_100));
    assert_eq!(a.channels, Some(2));
    assert!(a.duration.is_some());
}

#[test]
fn true_audio_with_ape_tags() {
    let args = metadata(&[
        ("title", "Tone"),
        ("artist", "Generator"),
        ("album", "Signals"),
        ("track", "2/9"),
    ]);
    let Some(path) = encode("fallback.tta", "tta", &args, "tta") else {
        return;
    };
    let file = read_tags(&path, &ReadOptions::default()).expect("read");
    assert_eq!(file.source, TagSource::Ffmpeg);
    let t = &file.tags;
    assert_eq!(t.title.as_deref(), Some("Tone"));
    assert_eq!(t.artist.as_deref(), Some("Generator"));
    assert_eq!(t.album.as_deref(), Some("Signals"));
    assert_eq!((t.track_number, t.track_total), (Some(2), Some(9)));
    assert_eq!(file.audio.codec.as_deref(), Some("tta"));
}

#[test]
fn windows_media_audio() {
    let args = metadata(&[
        ("title", "Bliss"),
        ("artist", "Windows"),
        ("album", "Defaults"),
        ("album_artist", "Windows"),
        ("WM/SharedUserRating", "75"),
    ]);
    let Some(path) = encode("fallback.wma", "wmav2", &args, "asf") else {
        return;
    };
    let file = read_tags(&path, &ReadOptions::default()).expect("read");
    assert_eq!(file.source, TagSource::Ffmpeg);
    let t = &file.tags;
    assert_eq!(t.title.as_deref(), Some("Bliss"));
    assert_eq!(t.artist.as_deref(), Some("Windows"));
    assert_eq!(t.album.as_deref(), Some("Defaults"));
    assert_eq!(t.album_artist.as_deref(), Some("Windows"));
    assert_eq!(t.rating.map(|r| r.half_stars()), Some(8));
    assert_eq!(file.audio.codec.as_deref(), Some("wmav2"));
    assert_eq!(file.audio.container.as_deref(), Some("asf"));
}
