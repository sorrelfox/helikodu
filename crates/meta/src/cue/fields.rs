// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tags of a virtual track: the track's fields merged with the album's.
//!
//! | Sheet | Track tags |
//! |---|---|
//! | album `TITLE` | `album` |
//! | album `PERFORMER` | `album_artist`, and `artist` when the track has none |
//! | track `TITLE`, `PERFORMER` | `title`, `artist` |
//! | `SONGWRITER` (track, else album) | a [`Role::Writer`] credit |
//! | `TRACK nn` | `track_number`; `track_total` is the number of audio tracks |
//! | `CATALOG`, `ISRC` | `extra` `barcode`, `isrc` |
//! | `REM DATE`, `GENRE`, `COMMENT` | `date`, `genres`, `comment` |
//! | `REM DISCNUMBER`, `TOTALDISCS` | `disc_number`, `disc_total` |
//! | `REM REPLAYGAIN_ALBUM_*` (album), `REPLAYGAIN_TRACK_*` (track) | `replay_gain` |
//! | `REM COMPOSER`, `ARRANGER`, `LYRICIST`, `CONDUCTOR` | credits |
//! | `REM LABEL`, `CATALOGNUMBER`, `ORIGINALDATE`, `MUSICBRAINZ_*` | their fields |
//! | any other `REM` (`DISCID`, …) | `extra`, the key in lower case |
//!
//! A track's `REM` lines override the album's.

use crate::tags::{Credit, Role, TrackTags};

use super::sheet::{CueSheet, CueTrack, Remark};

/// The longest text value taken into the tags, in bytes.
const MAX_VALUE_LEN: usize = 4096;

/// The tags of `track`, with the album fields of `sheet`.
pub(crate) fn track_tags(sheet: &CueSheet, track: &CueTrack, track_total: u32) -> TrackTags {
    let mut tags = TrackTags {
        title: text(track.title.as_deref()),
        album: text(sheet.title.as_deref()),
        track_number: Some(track.number),
        track_total: Some(track_total),
        ..TrackTags::default()
    };
    if let Some(album_artist) = text(sheet.performer.as_deref()) {
        tags.album_artists = vec![album_artist.clone()];
        tags.album_artist = Some(album_artist);
    }
    if let Some(artist) = text(track.performer.as_deref()).or_else(|| tags.album_artist.clone()) {
        tags.artists = vec![artist.clone()];
        tags.artist = Some(artist);
    }
    if let Some(writer) = text(track.songwriter.as_deref().or(sheet.songwriter.as_deref())) {
        tags.credits.push(Credit {
            role: Role::Writer,
            name: writer,
            instrument: None,
        });
    }
    if let Some(catalog) = text(sheet.catalog.as_deref()) {
        tags.extra.push(("barcode".into(), catalog));
    }
    if let Some(isrc) = text(track.isrc.as_deref()) {
        tags.extra.push(("isrc".into(), isrc));
    }
    apply_remarks(&mut tags, &sheet.remarks, Scope::Album);
    apply_remarks(&mut tags, &track.remarks, Scope::Track);
    tags
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Album,
    Track,
}

fn apply_remarks(tags: &mut TrackTags, remarks: &[Remark], scope: Scope) {
    // Credits and lists from the track replace the album's rather than add to them.
    let mut replaced: Vec<&str> = Vec::new();
    for remark in remarks {
        let Some(value) = text(Some(&remark.value)) else {
            continue;
        };
        let key = remark.key.as_str();
        let first_here = scope == Scope::Track && !replaced.contains(&key);
        if first_here {
            replaced.push(key);
        }
        match key {
            "DATE" | "YEAR" => tags.date = Some(value),
            "ORIGINALDATE" | "ORIGINALYEAR" => tags.original_date = Some(value),
            "GENRE" => {
                if first_here {
                    tags.genres.clear();
                }
                tags.genres.push(value);
            }
            "COMMENT" => tags.comment = Some(value),
            "TITLE" => tags.title = Some(value),
            "ALBUMARTIST" => {
                tags.album_artists = vec![value.clone()];
                tags.album_artist = Some(value);
            }
            "DISCNUMBER" | "DISC" => {
                let (number, total) = number_pair(&value);
                tags.disc_number = number.or(tags.disc_number);
                tags.disc_total = total.or(tags.disc_total);
            }
            "TOTALDISCS" | "DISCTOTAL" => tags.disc_total = number_pair(&value).0,
            "REPLAYGAIN_ALBUM_GAIN" => tags.replay_gain.album_gain_db = gain(&value),
            "REPLAYGAIN_ALBUM_PEAK" => tags.replay_gain.album_peak = gain(&value),
            // A track gain given for the whole sheet is the gain of the whole file.
            "REPLAYGAIN_TRACK_GAIN" if scope == Scope::Track => {
                tags.replay_gain.track_gain_db = gain(&value);
            }
            "REPLAYGAIN_TRACK_PEAK" if scope == Scope::Track => {
                tags.replay_gain.track_peak = gain(&value);
            }
            "REPLAYGAIN_TRACK_GAIN" | "REPLAYGAIN_TRACK_PEAK" => {}
            "COMPOSER" => credit(tags, Role::Composer, value, first_here),
            "ARRANGER" => credit(tags, Role::Arranger, value, first_here),
            "LYRICIST" => credit(tags, Role::Lyricist, value, first_here),
            "CONDUCTOR" => credit(tags, Role::Conductor, value, first_here),
            "LABEL" => {
                if first_here {
                    tags.labels.clear();
                }
                tags.labels.push(value);
            }
            "CATALOGNUMBER" => {
                if first_here {
                    tags.catalog_numbers.clear();
                }
                tags.catalog_numbers.push(value);
            }
            "UPC_EAN" => extra(tags, "barcode", value),
            "MUSICBRAINZ_ALBUMID" => tags.musicbrainz.release = Some(value),
            "MUSICBRAINZ_RELEASEGROUPID" => tags.musicbrainz.release_group = Some(value),
            "MUSICBRAINZ_TRACKID" => tags.musicbrainz.recording = Some(value),
            "MUSICBRAINZ_RELEASETRACKID" => tags.musicbrainz.release_track = Some(value),
            _ => extra(tags, &key.to_ascii_lowercase(), value),
        }
    }
}

fn credit(tags: &mut TrackTags, role: Role, name: String, replace: bool) {
    if replace {
        tags.credits.retain(|c| c.role != role);
    }
    tags.credits.push(Credit {
        role,
        name,
        instrument: None,
    });
}

/// Sets `key` in `extra`, replacing an earlier value.
fn extra(tags: &mut TrackTags, key: &str, value: String) {
    tags.extra.retain(|(k, _)| k != key);
    tags.extra.push((key.to_owned(), value));
}

/// Trimmed, non-empty, bounded text.
fn text(value: Option<&str>) -> Option<String> {
    let value = crate::tags::trim_text(value?);
    if value.is_empty() {
        return None;
    }
    Some(value[..value.floor_char_boundary(MAX_VALUE_LEN)].to_owned())
}

/// `n` or `n/total`.
fn number_pair(value: &str) -> (Option<u32>, Option<u32>) {
    let (n, total) = match value.split_once('/') {
        Some((n, t)) => (n, Some(t)),
        None => (value, None),
    };
    (
        n.trim().parse().ok(),
        total.and_then(|t| t.trim().parse().ok()),
    )
}

/// A gain such as `-7.89 dB` or a peak such as `0.988`.
fn gain(value: &str) -> Option<f64> {
    let number = value.trim();
    let number = number
        .strip_suffix("dB")
        .or_else(|| number.strip_suffix("db"))
        .or_else(|| number.strip_suffix("DB"))
        .unwrap_or(number)
        .trim();
    number.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gains_and_numbers() {
        assert_eq!(gain("-7.89 dB"), Some(-7.89));
        assert_eq!(gain("+1.5dB"), Some(1.5));
        assert_eq!(gain("0.977203"), Some(0.977203));
        assert_eq!(gain("NaN"), None);
        assert_eq!(gain("loud"), None);
        assert_eq!(number_pair("1/2"), (Some(1), Some(2)));
        assert_eq!(number_pair("3"), (Some(3), None));
        assert_eq!(number_pair("x"), (None, None));
    }
}
