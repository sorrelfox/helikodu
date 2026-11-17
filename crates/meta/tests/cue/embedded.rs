// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sheets embedded in audio files written at test time.

use std::fs;
use std::process::Command;

use helikodu_meta::cue::{CueOptions, CueOrigin, CueWarningKind, read_embedded_cue};
use helikodu_model::audio::{MediaTime, SampleRate};

use crate::media::{encode, folder, have};

const SHEET: &str = "REM DATE 2001\nPERFORMER \"Sheet Artist\"\nFILE \"CDImage.wav\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"One\"\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    TITLE \"Two\"\n    INDEX 00 00:03:00\n    INDEX 01 00:04:00\n";

fn cuesheet_metadata(sheet: &str) -> Vec<String> {
    vec![
        "-metadata".into(),
        format!("CUESHEET={sheet}"),
        "-metadata".into(),
        "ALBUM=Host Album".into(),
        "-metadata".into(),
        "GENRE=Ambient".into(),
    ]
}

fn check_two_tracks(tracks: &[helikodu_meta::VirtualTrack], path: &std::path::Path) {
    assert_eq!(tracks.len(), 2);
    assert!(tracks.iter().all(|t| t.source.path == path));
    assert_eq!(tracks[0].start(), MediaTime::ZERO);
    assert_eq!(tracks[0].end(), Some(MediaTime::from_cue_frames(300)));
    assert_eq!(tracks[1].start(), MediaTime::from_cue_frames(300));
    assert_eq!(tracks[1].end(), None);
    assert_eq!(
        tracks[1].pregap_start,
        Some(MediaTime::from_cue_frames(225))
    );
}

#[test]
fn cuesheet_tag_in_flac() {
    let dir = folder("embedded-flac-tag");
    let mut args = vec!["-c:a".to_owned(), "flac".to_owned()];
    args.extend(cuesheet_metadata(SHEET));
    let Some(path) = encode(&dir.join("album.flac"), &args) else {
        return;
    };
    let found = read_embedded_cue(&path, &CueOptions::default())
        .unwrap()
        .expect("a sheet");
    assert_eq!(found.origin, CueOrigin::Tag);
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
    check_two_tracks(&found.tracks, &path);
    let tags = &found.tracks[1].tags;
    assert_eq!(tags.title.as_deref(), Some("Two"));
    assert_eq!(tags.artist.as_deref(), Some("Sheet Artist"));
    assert_eq!(tags.date.as_deref(), Some("2001"));
    // The file's own tags fill what the sheet lacks.
    assert_eq!(tags.album.as_deref(), Some("Host Album"));
    assert_eq!(tags.genres, ["Ambient"]);
}

#[test]
fn cuesheet_tag_in_wavpack() {
    let dir = folder("embedded-wavpack-tag");
    let mut args = vec!["-c:a".to_owned(), "wavpack".to_owned()];
    args.extend(cuesheet_metadata(SHEET));
    args.extend(["-f".to_owned(), "wv".to_owned()]);
    let Some(path) = encode(&dir.join("album.wv"), &args) else {
        return;
    };
    let found = read_embedded_cue(&path, &CueOptions::default())
        .unwrap()
        .expect("a sheet");
    assert_eq!(found.origin, CueOrigin::Tag);
    check_two_tracks(&found.tracks, &path);
}

#[test]
fn a_long_cuesheet_tag_is_read_through_ffmpeg() {
    // Longer than the tag reader keeps for an unmapped field.
    let mut sheet = String::from("FILE \"a.wav\" WAVE\n");
    for i in 1..=99u32 {
        sheet.push_str(&format!(
            "  TRACK {i:02} AUDIO\n    TITLE \"A rather long title for track number {i}\"\n    PERFORMER \"Somebody with a long name\"\n    REM COMMENT \"padding padding padding padding\"\n    INDEX 01 00:{:02}:{:02}\n",
            i / 10,
            (i % 10) * 7
        ));
    }
    assert!(sheet.len() > 16 * 1024);
    let dir = folder("embedded-long-tag");
    let mut args = vec!["-c:a".to_owned(), "flac".to_owned()];
    args.extend(cuesheet_metadata(&sheet));
    let Some(path) = encode(&dir.join("long.flac"), &args) else {
        return;
    };
    let found = read_embedded_cue(&path, &CueOptions::default())
        .unwrap()
        .expect("a sheet");
    assert_eq!(found.tracks.len(), 99);
    assert_eq!(
        found.tracks[98].tags.title.as_deref(),
        Some("A rather long title for track number 99")
    );
}

#[test]
fn an_embedded_sheet_with_several_files_keeps_the_first() {
    let sheet = "FILE a.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE b.wav WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n";
    let dir = folder("embedded-two-files");
    let mut args = vec!["-c:a".to_owned(), "flac".to_owned()];
    args.extend(cuesheet_metadata(sheet));
    let Some(path) = encode(&dir.join("two.flac"), &args) else {
        return;
    };
    let found = read_embedded_cue(&path, &CueOptions::default())
        .unwrap()
        .expect("a sheet");
    assert_eq!(found.tracks.len(), 1);
    assert_eq!(
        found
            .warnings
            .iter()
            .map(|w| w.kind.clone())
            .collect::<Vec<_>>(),
        [CueWarningKind::EmbeddedExtraFile]
    );
}

#[test]
fn flac_cuesheet_block() {
    if !have("metaflac") {
        eprintln!("SKIPPED: metaflac is not installed; the FLAC cue sheet block not tested");
        return;
    }
    let dir = folder("embedded-flac-block");
    let args = [
        "-c:a",
        "flac",
        "-metadata",
        "ALBUM=Block Album",
        "-metadata",
        "ARTIST=Block Artist",
    ];
    let Some(path) = encode(&dir.join("block.flac"), &args) else {
        return;
    };
    let cue = dir.join("block.cue");
    fs::write(
        &cue,
        "CATALOG 1234567890123\nFILE \"block.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    FLAGS PRE\n    INDEX 00 00:03:00\n    INDEX 01 00:04:00\n  TRACK 03 AUDIO\n    INDEX 01 00:07:00\n",
    )
    .unwrap();
    let status = Command::new("metaflac")
        .arg(format!("--import-cuesheet-from={}", cue.display()))
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());

    let found = read_embedded_cue(&path, &CueOptions::default())
        .unwrap()
        .expect("a sheet");
    assert_eq!(found.origin, CueOrigin::FlacBlock);
    assert!(found.sheet.is_none());
    let t = &found.tracks;
    assert_eq!(t.len(), 3);
    let rate = SampleRate::new(44_100).unwrap();
    // Positions are samples at the stream's rate and equal the sheet's frames.
    assert_eq!(t[1].start(), MediaTime::from_frames(176_400, rate));
    assert_eq!(t[1].start(), MediaTime::from_cue_frames(300));
    assert_eq!(t[0].end(), Some(MediaTime::from_cue_frames(300)));
    assert_eq!(t[1].end(), Some(MediaTime::from_cue_frames(525)));
    assert_eq!(t[2].end(), None);
    assert_eq!(t[1].pregap_start, Some(MediaTime::from_cue_frames(225)));
    assert!(t[1].pre_emphasis && !t[0].pre_emphasis);
    assert_eq!(t[2].tags.track_number, Some(3));
    assert_eq!(t[2].tags.track_total, Some(3));
    assert_eq!(t[0].tags.album.as_deref(), Some("Block Album"));
    assert_eq!(t[0].tags.album_artist.as_deref(), Some("Block Artist"));
    assert_eq!(
        t[0].tags.extra_values("barcode").collect::<Vec<_>>(),
        ["1234567890123"]
    );
}

#[test]
fn files_without_a_sheet() {
    let dir = folder("embedded-none");
    let Some(path) = encode(&dir.join("plain.flac"), &["-c:a", "flac"]) else {
        return;
    };
    assert!(
        read_embedded_cue(&path, &CueOptions::default())
            .unwrap()
            .is_none()
    );
    let text = dir.join("not-audio.txt");
    fs::write(&text, "hello").unwrap();
    assert!(read_embedded_cue(&text, &CueOptions::default()).is_err());
    assert!(read_embedded_cue(dir.join("missing.flac"), &CueOptions::default()).is_err());
}
