// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The CUE set: single-file, multi-file and embedded sheets, pregaps, hidden
//! track one audio, and sheets in legacy charsets and UTF-8 with a byte order
//! mark split their audio into the tracks the manifest lists.

use std::os::unix::ffi::OsStrExt;

use helikodu_meta::cue::{CueOptions, CueTime};
use helikodu_meta::{CueTracks, read_cue, read_embedded_cue};

use crate::manifest::{Corpus, Entry, assert_no_problems, decode_name};

fn time(text: &str) -> helikodu_model::audio::MediaTime {
    CueTime::parse(text)
        .unwrap_or_else(|| panic!("`{text}` is not a CUE time"))
        .to_media_time()
}

/// Every difference between the tracks read and the entry's expectations.
fn compare(entry: &Entry, read: &CueTracks, problems: &mut Vec<String>) {
    let expect = entry.expect();
    let mut report = |what: String| problems.push(format!("{}: {what}", entry.path));
    if !read.warnings.is_empty() {
        report(format!("warnings: {:?}", read.warnings));
    }
    if read.tracks.len() != expect.cue.len() {
        report(format!(
            "{} tracks, expected {}",
            read.tracks.len(),
            expect.cue.len()
        ));
        return;
    }
    for (track, wanted) in read.tracks.iter().zip(&expect.cue) {
        let n = wanted.number;
        if track.number != n {
            report(format!("track {} where {n} was expected", track.number));
        }
        let file = track.source.path.as_os_str().as_bytes();
        let file = file
            .iter()
            .rposition(|&b| b == b'/')
            .map_or(file, |i| &file[i + 1..]);
        if file != decode_name(&wanted.file).as_slice() {
            report(format!(
                "track {n} plays {:?}, expected {}",
                String::from_utf8_lossy(file),
                wanted.file
            ));
        }
        if track.start() != time(&wanted.start) {
            report(format!(
                "track {n} starts at {}, expected {}",
                track.start(),
                wanted.start
            ));
        }
        if track.end() != wanted.end.as_deref().map(time) {
            report(format!(
                "track {n} ends at {:?}, expected {:?}",
                track.end(),
                wanted.end
            ));
        }
        if track.pregap_start != wanted.pregap.as_deref().map(time) {
            report(format!(
                "track {n} has its pregap at {:?}, expected {:?}",
                track.pregap_start, wanted.pregap
            ));
        }
        if let Some(title) = &wanted.title
            && track.tags.title.as_ref() != Some(title)
        {
            report(format!(
                "track {n} is titled {:?}, expected {title:?}",
                track.tags.title
            ));
        }
        if let Some(artist) = &wanted.artist
            && track.tags.artist.as_ref() != Some(artist)
        {
            report(format!(
                "track {n} is by {:?}, expected {artist:?}",
                track.tags.artist
            ));
        }
        for (key, values) in &expect.tags {
            let actual = match key.as_str() {
                "album" => track.tags.album.iter().cloned().collect(),
                "album_artist" => track.tags.album_artist.iter().cloned().collect(),
                "genres" => track.tags.genres.clone(),
                "date" => track.tags.date.iter().cloned().collect(),
                _ => {
                    report(format!("unknown field `{key}`"));
                    continue;
                }
            };
            if actual != values.list() {
                report(format!(
                    "track {n}: {key} is {actual:?}, expected {:?}",
                    values.list()
                ));
            }
        }
    }
}

#[test]
fn sheets_split_as_recorded() {
    let test = "sheets_split_as_recorded";
    let Some(corpus) = Corpus::load(test) else {
        return;
    };
    let Some(files) = corpus.select(test, &["cue"], |e| !e.cue.is_empty()) else {
        return;
    };
    let mut problems = Vec::new();
    for entry in &files {
        let options = CueOptions {
            legacy_charset: entry.expect().charset,
        };
        let path = corpus.path(entry);
        let read = if entry.path.ends_with(".cue") {
            read_cue(&path, &options).map(Some)
        } else {
            read_embedded_cue(&path, &options)
        };
        match read {
            Ok(Some(read)) => compare(entry, &read, &mut problems),
            Ok(None) => problems.push(format!("{}: no sheet found", entry.path)),
            Err(e) => problems.push(format!("{}: cannot read: {e}", entry.path)),
        }
    }
    assert_no_problems(test, files.len(), &problems);
}
