// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tag and script sets: every tag format the library reads, several
//! values, legacy charsets, sort and classical fields, ReplayGain and R128
//! gains, ratings as players write them, embedded pictures and lyrics, and
//! text in many scripts.

use helikodu_meta::covers::{CoverLimits, read_embedded_covers};
use helikodu_meta::lyrics::read_embedded;
use helikodu_meta::tags::Role;
use helikodu_meta::{ReadOptions, TrackTags, read_tags};

use crate::manifest::{Corpus, Entry, assert_no_problems};

/// The sets whose files carry tags to check.
const SETS: &[&str] = &["tags", "scripts"];

/// Fields compared as numbers.
const GAINS: &[&str] = &["track_gain", "track_peak", "album_gain", "album_peak"];

fn one<T: ToString>(value: Option<T>) -> Vec<String> {
    value.map(|v| v.to_string()).into_iter().collect()
}

fn credits(tags: &TrackTags, role: Role) -> Vec<String> {
    tags.credits_with(role)
        .map(|c| match &c.instrument {
            Some(instrument) => format!("{} ({instrument})", c.name),
            None => c.name.clone(),
        })
        .collect()
}

/// The values of the field the manifest calls `key`, or `None` for a key this
/// test does not know.
fn field(tags: &TrackTags, key: &str) -> Option<Vec<String>> {
    let rg = &tags.replay_gain;
    Some(match key {
        "title" => one(tags.title.as_ref()),
        "title_sort" => one(tags.title_sort.as_ref()),
        "artist" => one(tags.artist.as_ref()),
        "artists" => tags.artists.clone(),
        "artist_sort" => one(tags.artist_sort.as_ref()),
        "album" => one(tags.album.as_ref()),
        "album_sort" => one(tags.album_sort.as_ref()),
        "album_artist" => one(tags.album_artist.as_ref()),
        "album_artists" => tags.album_artists.clone(),
        "album_artist_sort" => one(tags.album_artist_sort.as_ref()),
        "composer_sort" => one(tags.composer_sort.as_ref()),
        "track_number" => one(tags.track_number),
        "track_total" => one(tags.track_total),
        "disc_number" => one(tags.disc_number),
        "disc_total" => one(tags.disc_total),
        "date" => one(tags.date.as_ref()),
        "original_date" => one(tags.original_date.as_ref()),
        "genres" => tags.genres.clone(),
        "composers" => credits(tags, Role::Composer),
        "conductors" => credits(tags, Role::Conductor),
        "performers" => credits(tags, Role::Performer),
        "lyricists" => credits(tags, Role::Lyricist),
        "writers" => credits(tags, Role::Writer),
        "labels" => tags.labels.clone(),
        "catalog_numbers" => tags.catalog_numbers.clone(),
        "comment" => one(tags.comment.as_ref()),
        "has_lyrics" => vec![tags.has_lyrics.to_string()],
        "rating" => one(tags.rating.map(|r| r.half_stars())),
        "compilation" => one(tags.compilation),
        "grouping" => one(tags.grouping.as_ref()),
        "work" => one(tags.work.as_ref()),
        "movement" => one(tags.movement.as_ref()),
        "movement_number" => one(tags.movement_number),
        "movement_total" => one(tags.movement_total),
        "track_gain" => one(rg.track_gain_db),
        "track_peak" => one(rg.track_peak),
        "album_gain" => one(rg.album_gain_db),
        "album_peak" => one(rg.album_peak),
        "r128_track_gain" => one(rg.r128_track_gain),
        "r128_album_gain" => one(rg.r128_album_gain),
        _ => tags
            .extra_values(key.strip_prefix("extra:")?)
            .map(str::to_owned)
            .collect(),
    })
}

fn same(key: &str, expected: &[String], actual: &[String]) -> bool {
    if GAINS.contains(&key) {
        let numbers = |values: &[String]| -> Vec<f64> {
            values.iter().filter_map(|v| v.parse().ok()).collect()
        };
        let (e, a) = (numbers(expected), numbers(actual));
        e.len() == expected.len()
            && e.len() == a.len()
            && e.iter().zip(&a).all(|(e, a)| (e - a).abs() < 1e-6)
    } else {
        expected == actual
    }
}

fn read_options(entry: &Entry) -> ReadOptions {
    ReadOptions {
        legacy_charset: entry.expect().charset,
        ..ReadOptions::default()
    }
}

#[test]
fn tag_fields_read_as_recorded() {
    let test = "tag_fields_read_as_recorded";
    let Some(corpus) = Corpus::load(test) else {
        return;
    };
    let Some(files) = corpus.select(test, SETS, |e| !e.tags.is_empty()) else {
        return;
    };
    let mut problems = Vec::new();
    for entry in &files {
        let path = corpus.path(entry);
        let tags = match read_tags(&path, &read_options(entry)) {
            Ok(file) => file.tags,
            Err(e) => {
                problems.push(format!("{}: cannot read: {e}", entry.path));
                continue;
            }
        };
        for (key, expected) in &entry.expect().tags {
            let expected = expected.list();
            match field(&tags, key) {
                None => problems.push(format!("{}: unknown field `{key}`", entry.path)),
                Some(actual) if !same(key, &expected, &actual) => problems.push(format!(
                    "{}: {key} is {actual:?}, expected {expected:?}",
                    entry.path
                )),
                Some(_) => {}
            }
        }
    }
    assert_no_problems(test, files.len(), &problems);
}

#[test]
fn embedded_pictures_read_as_recorded() {
    let test = "embedded_pictures_read_as_recorded";
    let Some(corpus) = Corpus::load(test) else {
        return;
    };
    let Some(files) = corpus.select(test, SETS, |e| !e.pictures.is_empty()) else {
        return;
    };
    let limits = CoverLimits::default();
    let mut problems = Vec::new();
    for entry in &files {
        let covers = match read_embedded_covers(corpus.path(entry), &limits) {
            Ok(covers) => covers,
            Err(e) => {
                problems.push(format!("{}: cannot read: {e}", entry.path));
                continue;
            }
        };
        let actual: Vec<(Option<u8>, u32, u32)> = covers
            .covers
            .iter()
            .map(|cover| {
                let (width, height) = cover
                    .probe(&limits)
                    .map_or((0, 0), |info| (info.width, info.height));
                (cover.source().picture_type().map(|t| t.0), width, height)
            })
            .collect();
        let expected: Vec<(Option<u8>, u32, u32)> = entry
            .expect()
            .pictures
            .iter()
            .map(|p| (p.kind, p.size, p.size))
            .collect();
        if actual != expected {
            problems.push(format!(
                "{}: pictures (type, width, height) are {actual:?}, expected {expected:?}; skipped {:?}",
                entry.path, covers.skipped
            ));
        }
        // The front cover is chosen when there is one.
        if let Some(front) = expected.iter().position(|p| p.0 == Some(3))
            && covers
                .front()
                .map(|c| c.source().picture_type().map(|t| t.0))
                != Some(Some(3))
        {
            problems.push(format!(
                "{}: picture {front}, the front cover, is not chosen as the cover",
                entry.path
            ));
        }
    }
    assert_no_problems(test, files.len(), &problems);
}

#[test]
fn embedded_lyrics_read_as_recorded() {
    let test = "embedded_lyrics_read_as_recorded";
    let Some(corpus) = Corpus::load(test) else {
        return;
    };
    let Some(files) = corpus.select(test, SETS, |e| e.lyrics.is_some()) else {
        return;
    };
    let mut problems = Vec::new();
    for entry in &files {
        let expected = entry.expect().lyrics.unwrap();
        let found = match read_embedded(&corpus.path(entry)) {
            Ok(found) => found,
            Err(e) => {
                problems.push(format!("{}: cannot read: {e}", entry.path));
                continue;
            }
        };
        let Some(lyrics) = found.first() else {
            problems.push(format!("{}: no lyrics found", entry.path));
            continue;
        };
        let first = lyrics.lines.first().map(|l| l.text.as_str()).unwrap_or("");
        if lyrics.synced != expected.synced
            || lyrics.lines.len() != expected.lines
            || first != expected.first_line
        {
            problems.push(format!(
                "{}: lyrics are synced={} with {} lines starting {first:?}, expected synced={} with {} lines starting {:?}",
                entry.path,
                lyrics.synced,
                lyrics.lines.len(),
                expected.synced,
                expected.lines,
                expected.first_line
            ));
        }
    }
    assert_no_problems(test, files.len(), &problems);
}
