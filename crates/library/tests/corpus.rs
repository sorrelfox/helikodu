// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The library against the generated test corpus (`cargo xtask corpus`, see
//! `tests/corpus/README.md`): the tag, script and CUE sets scan into the tracks
//! and titles their manifest entries expect, and the locale names of the script
//! set sort as recorded for each locale.
//!
//! The corpus is read from `target/corpus/` (or `$HELIKODU_CORPUS`). When it has
//! not been generated, every test prints `SKIPPED:` and passes.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "support/scanning.rs"]
mod support;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use helikodu_library::{
    CancelToken, DatabaseConfig, Library, LocaleCollation, ScanEvent, ScanOptions,
};
use helikodu_meta::{ReadOptions, read_tags};
use serde::Deserialize;
use support::{have_ffmpeg, worker_program};

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Values {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
struct CueExpect {
    number: u32,
    file: String,
    title: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Expect {
    #[serde(default)]
    tags: BTreeMap<String, Values>,
    charset: Option<String>,
    #[serde(default)]
    cue: Vec<CueExpect>,
    #[serde(default)]
    sort: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct Entry {
    path: String,
    set: String,
    #[serde(default)]
    expect: Option<Expect>,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(rename = "file", default)]
    files: Vec<Entry>,
}

/// The corpus folder and its files of `sets`, or `None` after printing why the
/// test is skipped.
fn corpus(test: &str, sets: &[&str]) -> Option<(PathBuf, Vec<Entry>)> {
    let dir = std::env::var_os("HELIKODU_CORPUS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus"),
        PathBuf::from,
    );
    let Ok(text) = fs::read_to_string(dir.join("manifest.toml")) else {
        eprintln!(
            "SKIPPED: {test}: no corpus at {} (make it with `cargo xtask corpus`)",
            dir.display()
        );
        return None;
    };
    let manifest: ManifestFile = toml::from_str(&text).expect("the corpus manifest parses");
    let files: Vec<Entry> = manifest
        .files
        .into_iter()
        .filter(|f| sets.contains(&f.set.as_str()))
        .collect();
    if files.is_empty() {
        eprintln!(
            "SKIPPED: {test}: the corpus has no {} files",
            sets.join(" or ")
        );
        return None;
    }
    Some((dir, files))
}

/// Decodes a percent-encoded name: `%XX` is the byte `0xXX`.
fn decode_name(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = name
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn path_of(dir: &Path, name: &str) -> PathBuf {
    dir.join(OsStr::from_bytes(&decode_name(name)))
}

fn title_of(library: &Library, path: &Path, sub_index: u32) -> Option<String> {
    let track = library.track_at(path, sub_index).unwrap()?;
    let id = i64::try_from(track.get()).unwrap();
    library
        .database()
        .read(|c| c.query_row("SELECT title FROM tracks WHERE id = ?1", [id], |r| r.get(0)))
        .unwrap()
}

#[test]
fn locale_names_sort_as_recorded() {
    let test = "locale_names_sort_as_recorded";
    let Some((dir, files)) = corpus(test, &["scripts"]) else {
        return;
    };
    let named: Vec<(String, BTreeMap<String, u32>)> = files
        .iter()
        .filter_map(|entry| {
            let expect = entry.expect.clone()?;
            (!expect.sort.is_empty()).then_some((entry, expect.sort))
        })
        .map(|(entry, sort)| {
            let tags = read_tags(path_of(&dir, &entry.path), &ReadOptions::default())
                .unwrap()
                .tags;
            (tags.artist.expect("a locale file has an artist"), sort)
        })
        .collect();
    assert!(!named.is_empty(), "the script set has no locale files");
    let locales: Vec<&String> = named[0].1.keys().collect();
    let mut problems = Vec::new();
    for locale in locales {
        let collation = LocaleCollation::new(locale).unwrap();
        let mut sorted: Vec<&String> = named.iter().map(|(artist, _)| artist).collect();
        sorted.sort_by(|a, b| collation.compare(a, b));
        let mut expected: Vec<(&String, u32)> = named
            .iter()
            .map(|(artist, sort)| (artist, sort[locale]))
            .collect();
        expected.sort_by_key(|(_, position)| *position);
        let expected: Vec<&String> = expected.into_iter().map(|(artist, _)| artist).collect();
        if sorted != expected {
            problems.push(format!("{locale}: {sorted:?}, expected {expected:?}"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn tags_scripts_and_sheets_scan_as_recorded() {
    let test = "tags_scripts_and_sheets_scan_as_recorded";
    let sets = ["tags", "scripts", "cue"];
    let Some((dir, files)) = corpus(test, &sets) else {
        return;
    };
    if !have_ffmpeg() {
        return;
    }
    let mut options = ScanOptions::new(sets.iter().map(|set| dir.join(set)));
    options.workers.program = Some(worker_program());
    options.workers.count = 2;
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let (sender, receiver) = mpsc::channel();
    let summary = library
        .scan(&options, Some(&sender), &CancelToken::new())
        .unwrap();
    drop(sender);
    let failures: Vec<ScanEvent> = receiver
        .into_iter()
        .filter(|e| matches!(e, ScanEvent::FileFailed { .. }))
        .collect();

    // Audio files that more than one sheet splits are left out: which sheet
    // wins is not what this test is about.
    let mut sheets_of: HashMap<Vec<u8>, BTreeSet<&str>> = HashMap::new();
    for entry in &files {
        for track in entry.expect.iter().flat_map(|e| &e.cue) {
            sheets_of
                .entry(audio_of(&entry.path, &track.file))
                .or_default()
                .insert(entry.path.as_str());
        }
    }
    let mut problems = Vec::new();
    let mut checked = 0;
    for entry in &files {
        let expect = entry.expect.clone().unwrap_or_default();
        let is_sheet_audio = |path: &str| sheets_of.contains_key(&decode_name(path));
        // Files without a title (the untagged ones) are listed under their
        // file names, and those that need a chosen charset are left out.
        if expect.cue.is_empty()
            && !is_sheet_audio(&entry.path)
            && expect.charset.is_none()
            && let Some(wanted) = expect.tags.get("title").and_then(|title| match title {
                Values::One(t) => Some(t.clone()),
                Values::Many(list) => list.first().cloned(),
            })
        {
            let actual = title_of(&library, &path_of(&dir, &entry.path), 0);
            checked += 1;
            if actual.as_ref() != Some(&wanted) {
                problems.push(format!(
                    "{}: title {actual:?}, expected {wanted:?}",
                    entry.path
                ));
            }
        }
        for track in &expect.cue {
            let audio = audio_of(&entry.path, &track.file);
            if sheets_of[&audio].len() > 1 {
                continue;
            }
            checked += 1;
            let path = dir.join(OsStr::from_bytes(&audio));
            let actual = title_of(&library, &path, track.number);
            if actual != track.title {
                problems.push(format!(
                    "{} track {}: title {actual:?}, expected {:?}",
                    entry.path, track.number, track.title
                ));
            }
        }
    }
    eprintln!("{test}: {summary:?}; {checked} titles checked");
    assert!(failures.is_empty(), "scan failures: {failures:?}");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The corpus path of the audio file `file` (relative to the sheet's folder,
/// percent-encoded) of the sheet or embedding file at `sheet`, as raw bytes.
fn audio_of(sheet: &str, file: &str) -> Vec<u8> {
    let sheet = decode_name(sheet);
    let folder = &sheet[..sheet.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1)];
    let mut path = folder.to_vec();
    path.extend(decode_name(file));
    path
}
