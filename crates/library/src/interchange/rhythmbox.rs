// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rhythmbox: `rhythmdb.xml` and the `playlists.xml` beside it.
//!
//! Each `entry type="song"` gives `location`, `title`, `artist`, `album`,
//! `track-number`, `duration` (seconds), `play-count`, `last-played` and
//! `first-seen` (seconds since the epoch) and `rating` (0 to 5; 0 is not rated).
//! Rhythmbox keeps no skip count. Static playlists list locations; automatic
//! playlists hold a query tree that becomes query text where each rule has a
//! counterpart (see [`automatic_query`]); the play queue is left out.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use super::foreign::{ForeignItem, ForeignLibrary, ForeignPlaylist, ForeignPlaylistKind};
use super::xml_tree::{self, Element};
use super::{ForeignTrack, ImportError};
use crate::db::unix_time::from_unix_seconds;
use crate::stats::Rating;

/// Reads `rhythmdb.xml` at `path` and the `playlists.xml` beside it.
pub(super) fn read(path: &Path) -> Result<ForeignLibrary, ImportError> {
    let root = xml_tree::read(path)?;
    if root.name != "rhythmdb" {
        return Err(ImportError::Format {
            path: path.to_path_buf(),
            reason: "not a Rhythmbox library (rhythmdb.xml)".to_owned(),
        });
    }
    let mut library = ForeignLibrary::default();
    let mut by_location: HashMap<String, usize> = HashMap::new();
    for entry in root.children_named("entry") {
        if entry.attribute("type") != Some("song") {
            continue;
        }
        let track = track(entry);
        if let Some(location) = &track.location {
            by_location.insert(location.clone(), library.tracks.len());
        }
        library.tracks.push(track);
    }
    let playlists = path.with_file_name("playlists.xml");
    if playlists.is_file() {
        let root = xml_tree::read(&playlists)?;
        for playlist in root.children_named("playlist") {
            if let Some(p) = read_playlist(playlist, &by_location) {
                library.playlists.push(p);
            }
        }
    }
    Ok(library)
}

fn number(entry: &Element, name: &str) -> Option<u64> {
    entry.child_text(name).and_then(|t| t.parse().ok())
}

fn time(entry: &Element, name: &str) -> Option<std::time::SystemTime> {
    number(entry, name)
        .filter(|&s| s > 0)
        .and_then(|s| i64::try_from(s).ok())
        .map(from_unix_seconds)
}

/// A rating of 0 to 5 stars, to the half star; 0 is not rated.
pub(super) fn stars(value: f64) -> Option<Rating> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    Rating::from_half_stars((value.min(5.0) * 2.0).round() as u8)
}

fn track(entry: &Element) -> ForeignTrack {
    let text = |name: &str| entry.child_text(name).map(str::to_owned);
    ForeignTrack {
        location: text("location"),
        title: text("title"),
        artist: text("artist"),
        album: text("album"),
        track_number: number(entry, "track-number")
            .and_then(|n| u32::try_from(n).ok())
            .filter(|&n| n > 0),
        duration: number(entry, "duration")
            .filter(|&d| d > 0)
            .map(Duration::from_secs),
        play_count: number(entry, "play-count").unwrap_or(0),
        rating: entry
            .child_text("rating")
            .and_then(|r| r.parse::<f64>().ok())
            .and_then(stars),
        last_played: time(entry, "last-played"),
        added: time(entry, "first-seen"),
        ..ForeignTrack::default()
    }
}

fn read_playlist(
    playlist: &Element,
    by_location: &HashMap<String, usize>,
) -> Option<ForeignPlaylist> {
    let name = playlist.attribute("name")?.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let kind = match playlist.attribute("type") {
        Some("static") => ForeignPlaylistKind::Static {
            items: playlist
                .children_named("location")
                .map(|l| l.text.trim())
                .filter(|l| !l.is_empty())
                .map(|l| match by_location.get(l) {
                    Some(&i) => ForeignItem::Track(i),
                    None => ForeignItem::Other(Box::new(ForeignTrack {
                        location: Some(l.to_owned()),
                        ..ForeignTrack::default()
                    })),
                })
                .collect(),
            from_rules: false,
        },
        Some("automatic") => match automatic_query(playlist) {
            Ok(query) => ForeignPlaylistKind::Smart(query),
            Err(reason) => ForeignPlaylistKind::Unmapped(reason),
        },
        // The play queue.
        _ => return None,
    };
    Some(ForeignPlaylist {
        name,
        folder: Vec::new(),
        kind,
    })
}

/// Text in quotes for the query language.
pub(super) fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The query-language field of a Rhythmbox property.
fn field(prop: &str) -> Option<&'static str> {
    let prop = prop.strip_suffix("-folded").unwrap_or(prop);
    Some(match prop {
        "title" => "title",
        "artist" => "artist",
        "album" => "album",
        "album-artist" => "albumartist",
        "genre" => "genre",
        "composer" => "composer",
        "rating" => "rating",
        "play-count" => "plays",
        "last-played" => "lastplayed",
        "first-seen" => "added",
        "duration" => "duration",
        "bitrate" => "bitrate",
        "track-number" => "track",
        "disc-number" => "disc",
        "location" => "path",
        "date" => "year",
        _ => return None,
    })
}

fn is_text(field: &str) -> bool {
    matches!(
        field,
        "title" | "artist" | "album" | "albumartist" | "genre" | "composer" | "path"
    )
}

/// An age in seconds as the query language writes it: days, or hours.
fn age(seconds: u64) -> String {
    if seconds.is_multiple_of(86_400) {
        format!("{}d", seconds / 86_400)
    } else {
        format!("{}h", seconds.div_ceil(3600))
    }
}

/// The year of a GLib Julian day (day 1 is 1 January of year 1).
fn julian_year(day: i64) -> i64 {
    // Days since 1970-01-01, then the civil-from-days algorithm.
    let z = day - 719_163 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(month <= 2)
}

/// One rule as a query term, `None` for a rule that always holds.
fn rule(element: &Element) -> Result<Option<String>, String> {
    let unmapped = || {
        format!(
            "the rule \"{} {}\" has no counterpart in the query language",
            element.attribute("prop").unwrap_or(""),
            element.name
        )
    };
    let prop = element.attribute("prop").unwrap_or("");
    let value = element.text.trim();
    if prop == "type" {
        return if value == "song" {
            Ok(None)
        } else {
            Err(unmapped())
        };
    }
    let field = field(prop).ok_or_else(unmapped)?;
    let number = || value.parse::<f64>().map_err(|_| unmapped());
    let whole = || value.parse::<u64>().map_err(|_| unmapped());
    let term = match (element.name.as_str(), is_text(field)) {
        ("equals", true) => format!("{field}={}", quote(value)),
        ("not-equal", true) => format!("{field}!={}", quote(value)),
        ("like", true) => format!("{field}:{}", quote(value)),
        ("not-like", true) => format!("-{field}:{}", quote(value)),
        ("prefix", true) => format!("{field}~{}", quote(&format!("^{}", regex::escape(value)))),
        ("suffix", true) => format!("{field}~{}", quote(&format!("{}$", regex::escape(value)))),
        ("current-time-within", false) if matches!(field, "lastplayed" | "added") => {
            format!("{field}<{}", age(whole()?))
        }
        ("current-time-not-within", false) if matches!(field, "lastplayed" | "added") => {
            format!("{field}>{}", age(whole()?))
        }
        ("year-equals", false) if field == "year" => {
            format!(
                "year={}",
                julian_year(value.parse().map_err(|_| unmapped())?)
            )
        }
        ("year-greater", false) if field == "year" => {
            format!(
                "year>{}",
                julian_year(value.parse().map_err(|_| unmapped())?)
            )
        }
        ("year-less", false) if field == "year" => {
            format!(
                "year<{}",
                julian_year(value.parse().map_err(|_| unmapped())?)
            )
        }
        (op @ ("equals" | "greater" | "less"), false)
            if !matches!(field, "lastplayed" | "added" | "year") =>
        {
            let sign = match op {
                "equals" => "=",
                "greater" => ">",
                _ => "<",
            };
            let n = number()?;
            let value = if field == "duration" {
                format!("{}s", n.round() as i64)
            } else if field == "rating" {
                format!("{}", (n * 2.0).round() / 2.0)
            } else {
                format!("{}", n.round() as i64)
            };
            format!("{field}{sign}{value}")
        }
        _ => return Err(unmapped()),
    };
    Ok(Some(term))
}

/// The terms of a `conjunction`: groups split by `disjunction`, joined by `OR`.
fn conjunction(element: &Element) -> Result<String, String> {
    let mut groups: Vec<Vec<String>> = vec![Vec::new()];
    for child in &element.children {
        match child.name.as_str() {
            "disjunction" => groups.push(Vec::new()),
            "subquery" | "conjunction" => {
                let inner = match child.child("conjunction") {
                    Some(c) if child.name == "subquery" => conjunction(c)?,
                    _ => conjunction(child)?,
                };
                if !inner.is_empty()
                    && let Some(group) = groups.last_mut()
                {
                    group.push(format!("({inner})"));
                }
            }
            _ => {
                if let (Some(term), Some(group)) = (rule(child)?, groups.last_mut()) {
                    group.push(term);
                }
            }
        }
    }
    let groups: Vec<String> = groups
        .into_iter()
        .filter(|g| !g.is_empty())
        .map(|g| g.join(" "))
        .collect();
    Ok(match groups.len() {
        0 => String::new(),
        1 => groups.into_iter().next().unwrap_or_default(),
        _ => groups
            .iter()
            .map(|g| format!("({g})"))
            .collect::<Vec<_>>()
            .join(" OR "),
    })
}

/// The sort key of a Rhythmbox `sort-key`.
fn sort_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "Title" => "title",
        "Artist" => "artist",
        "Album" => "album",
        "Genre" => "genre",
        "Rating" => "rating",
        "PlayCount" => "plays",
        "LastPlayed" => "lastplayed",
        "FirstSeen" => "added",
        "Duration" => "duration",
        "Year" | "Date" => "year",
        "Track" => "track",
        "Bitrate" => "bitrate",
        "Location" => "path",
        "Composer" => "composer",
        "Random" | "RandomByAge" | "RandomByRating" => "random",
        _ => return None,
    })
}

/// The query text of an automatic playlist: its rules, its order and its limit.
/// Fails, saying why, when a rule has no counterpart.
pub(super) fn automatic_query(playlist: &Element) -> Result<String, String> {
    let mut parts = Vec::new();
    if let Some(c) = playlist.child("conjunction") {
        let rules = conjunction(c)?;
        if !rules.is_empty() {
            parts.push(rules);
        }
    }
    if let Some(key) = playlist.attribute("sort-key").filter(|k| !k.is_empty()) {
        let by = sort_key(key).ok_or_else(|| {
            format!("the order \"{key}\" has no counterpart in the query language")
        })?;
        let desc = playlist.attribute("sort-direction") == Some("1") && by != "random";
        parts.push(format!("sort:{by}{}", if desc { " desc" } else { "" }));
    }
    let limit = |name: &str| {
        playlist
            .attribute(name)
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&n| n > 0)
    };
    if let Some(n) = limit("limit-count") {
        parts.push(format!("limit:{n}"));
    } else if let Some(mb) = limit("limit-size") {
        parts.push(format!("limit:{mb}MB"));
    } else if let Some(seconds) = limit("limit-time") {
        parts.push(format!("limit:{}min", seconds.div_ceil(60)));
    }
    if parts.is_empty() {
        return Err("the playlist has no rules".to_owned());
    }
    Ok(parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn julian_days_give_years() {
        // 1 January 1970 and 31 December 1999.
        assert_eq!(julian_year(719_163), 1970);
        assert_eq!(julian_year(730_119), 1999);
        assert_eq!(julian_year(730_120), 2000);
    }

    #[test]
    fn automatic_playlists_become_query_text() {
        let xml = br#"<playlist name="Top" type="automatic" sort-key="PlayCount" sort-direction="1" limit-count="25">
            <conjunction>
              <equals prop="type">song</equals>
              <greater prop="rating">3</greater>
              <like prop="artist-folded">miles "d"</like>
              <current-time-within prop="last-played">604800</current-time-within>
              <disjunction/>
              <subquery><conjunction><equals prop="genre">Jazz</equals><prefix prop="title">So</prefix></conjunction></subquery>
            </conjunction>
          </playlist>"#;
        let playlist = xml_tree::parse(xml).unwrap();
        assert_eq!(
            automatic_query(&playlist).unwrap(),
            r#"(rating>3 artist:"miles \"d\"" lastplayed<7d) OR ((genre="Jazz" title~"^So")) sort:plays desc limit:25"#
        );
        let odd = xml_tree::parse(
            br#"<playlist name="x" type="automatic"><conjunction><greater prop="mtime">5</greater></conjunction></playlist>"#,
        )
        .unwrap();
        assert!(automatic_query(&odd).is_err());
    }
}
