// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Container chapters, written with an FFMETADATA file, as virtual tracks.

use std::fs;
use std::path::Path;

use helikodu_meta::cue::read_chapters;
use helikodu_model::audio::MediaTime;

use crate::media::{encode, folder};

const FFMETADATA: &str = ";FFMETADATA1
title=The Book
artist=Narrator
[CHAPTER]
TIMEBASE=1/1000
START=0
END=4000
title=Opening
[CHAPTER]
TIMEBASE=1/1000
START=4000
END=10000
title=Second Part
artist=Guest Reader
";

fn chaptered(dir: &Path, name: &str, codec: &str, format: &str) -> Option<std::path::PathBuf> {
    let meta = dir.join("chapters.txt");
    fs::write(&meta, FFMETADATA).expect("write the chapters");
    let meta = meta.to_string_lossy().into_owned();
    let args = [
        "-i",
        &meta,
        "-map",
        "0:a",
        "-map_metadata",
        "1",
        "-map_chapters",
        "1",
        "-c:a",
        codec,
        "-f",
        format,
    ];
    encode(&dir.join(name), &args)
}

fn check(path: &Path) {
    let tracks = read_chapters(path).expect("chapters");
    assert_eq!(tracks.len(), 2);
    assert!(tracks.iter().all(|t| t.source.path == path));
    assert_eq!(tracks[0].start(), MediaTime::ZERO);
    assert_eq!(tracks[0].end(), Some(MediaTime::from_millis(4000)));
    assert_eq!(tracks[1].start(), MediaTime::from_millis(4000));
    assert_eq!(tracks[1].end(), Some(MediaTime::from_millis(10_000)));
    assert_eq!(tracks[0].tags.title.as_deref(), Some("Opening"));
    assert_eq!(tracks[1].tags.title.as_deref(), Some("Second Part"));
    assert_eq!(tracks[0].tags.album.as_deref(), Some("The Book"));
    assert_eq!(tracks[0].tags.artist.as_deref(), Some("Narrator"));
    assert_eq!(tracks[1].tags.track_number, Some(2));
    assert_eq!(tracks[1].tags.track_total, Some(2));
}

#[test]
fn matroska_chapters() {
    let dir = folder("chapters-mka");
    let Some(path) = chaptered(&dir, "book.mka", "flac", "matroska") else {
        return;
    };
    check(&path);
    let tracks = read_chapters(&path).unwrap();
    assert_eq!(tracks[1].tags.artist.as_deref(), Some("Guest Reader"));
}

#[test]
fn m4b_chapters() {
    let dir = folder("chapters-m4b");
    let Some(path) = chaptered(&dir, "book.m4b", "aac", "ipod") else {
        return;
    };
    check(&path);
}

#[test]
fn files_without_chapters() {
    let dir = folder("chapters-none");
    let Some(path) = encode(&dir.join("plain.flac"), &["-c:a", "flac"]) else {
        return;
    };
    assert!(read_chapters(&path).unwrap().is_empty());
}
