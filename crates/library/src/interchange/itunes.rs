// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! iTunes and Apple Music: an exported `Library.xml` (an XML property list).
//!
//! Each entry of `Tracks` gives `Location` (a `file://` URI, often of a Windows
//! or macOS path), `Name`, `Artist`, `Album`, `Track Number`, `Total Time`
//! (milliseconds), `Play Count`, `Play Date UTC`, `Skip Count`, `Skip Date`,
//! `Date Added`, `Rating` (0 to 100; a rating the album passed down, marked
//! `Rating Computed`, is left out) and `Loved` or `Favorited`. Playlists keep
//! their folders. A smart playlist's rules are a binary blob in the file; its
//! entries at the time of the export are imported as a static playlist instead.
//! The library itself (`Master`) and the built-in playlists (`Distinguished
//! Kind`) are left out.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::foreign::{ForeignItem, ForeignLibrary, ForeignPlaylist, ForeignPlaylistKind};
use super::xml_tree::{self, Element};
use super::{ForeignTrack, ImportError};
use crate::db::unix_time::from_unix_seconds;
use crate::stats::Rating;

/// A value of a property list.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Dict(Vec<(String, Value)>),
    Array(Vec<Value>),
    Text(String),
    Integer(i64),
    Date(Option<SystemTime>),
    Bool(bool),
    Other,
}

impl Value {
    fn of(element: &Element) -> Self {
        match element.name.as_str() {
            "dict" => {
                let mut entries = Vec::new();
                let mut key = None;
                for child in &element.children {
                    if child.name == "key" {
                        key = Some(child.text.clone());
                    } else if let Some(k) = key.take() {
                        entries.push((k, Value::of(child)));
                    }
                }
                Value::Dict(entries)
            }
            "array" => Value::Array(element.children.iter().map(Value::of).collect()),
            "string" => Value::Text(element.text.clone()),
            "integer" => element
                .text
                .trim()
                .parse()
                .map_or(Value::Other, Value::Integer),
            "real" => element
                .text
                .trim()
                .parse::<f64>()
                .map_or(Value::Other, |r| Value::Integer(r.round() as i64)),
            "date" => Value::Date(parse_date(element.text.trim())),
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => Value::Other,
        }
    }

    fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn text(&self, key: &str) -> Option<String> {
        match self.get(key) {
            Some(Value::Text(t)) => Some(t.trim().to_owned()).filter(|t| !t.is_empty()),
            _ => None,
        }
    }

    fn integer(&self, key: &str) -> Option<i64> {
        match self.get(key) {
            Some(Value::Integer(n)) => Some(*n),
            _ => None,
        }
    }

    fn date(&self, key: &str) -> Option<SystemTime> {
        match self.get(key) {
            Some(Value::Date(d)) => *d,
            _ => None,
        }
    }

    fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Value::Bool(true)))
    }
}

/// Days from 1970-01-01 to the date `y-m-d` of the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A property-list date, `2009-06-01T19:10:24Z`.
fn parse_date(text: &str) -> Option<SystemTime> {
    let text = text.strip_suffix('Z').unwrap_or(text);
    let (date, time) = text.split_once('T')?;
    let mut date = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (date.next()??, date.next()??, date.next()??);
    let mut time = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, s) = (time.next()??, time.next()??, time.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let seconds = days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60 + s;
    Some(from_unix_seconds(seconds))
}

/// Reads the `Library.xml` at `path`.
pub(super) fn read(path: &Path) -> Result<ForeignLibrary, ImportError> {
    let root = xml_tree::read(path)?;
    let plist = root
        .children
        .iter()
        .find(|c| c.name == "dict")
        .filter(|_| root.name == "plist")
        .map(Value::of);
    let Some(plist) = plist.filter(|p| p.get("Tracks").is_some()) else {
        return Err(ImportError::Format {
            path: path.to_path_buf(),
            reason: "not an iTunes or Music library (Library.xml)".to_owned(),
        });
    };
    Ok(read_plist(&plist))
}

fn track(entry: &Value) -> ForeignTrack {
    let count = |key: &str| {
        entry
            .integer(key)
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(0)
    };
    let rating = if entry.flag("Rating Computed") {
        None
    } else {
        entry
            .integer("Rating")
            .filter(|&r| r > 0)
            .and_then(|r| u8::try_from((r.min(100) + 5) / 10).ok())
            .and_then(Rating::from_half_stars)
    };
    ForeignTrack {
        location: entry.text("Location"),
        title: entry.text("Name"),
        artist: entry.text("Artist"),
        album: entry.text("Album"),
        track_number: entry
            .integer("Track Number")
            .and_then(|n| u32::try_from(n).ok())
            .filter(|&n| n > 0),
        duration: entry
            .integer("Total Time")
            .and_then(|n| u64::try_from(n).ok())
            .filter(|&n| n > 0)
            .map(Duration::from_millis),
        play_count: count("Play Count"),
        skip_count: count("Skip Count"),
        rating,
        loved: entry.flag("Loved") || entry.flag("Favorited"),
        last_played: entry.date("Play Date UTC"),
        last_skipped: entry.date("Skip Date"),
        added: entry.date("Date Added"),
        ..ForeignTrack::default()
    }
}

fn read_plist(plist: &Value) -> ForeignLibrary {
    let mut library = ForeignLibrary::default();
    let mut by_id: HashMap<i64, usize> = HashMap::new();
    if let Some(Value::Dict(tracks)) = plist.get("Tracks") {
        for (_, entry) in tracks {
            // Streams and other remote entries have no file.
            if entry.text("Track Type").is_some_and(|t| t != "File") {
                continue;
            }
            if let Some(id) = entry.integer("Track ID") {
                by_id.insert(id, library.tracks.len());
            }
            library.tracks.push(track(entry));
        }
    }
    let Some(Value::Array(playlists)) = plist.get("Playlists") else {
        return library;
    };
    // Folders by their persistent id: the name and the parent's id.
    let folders: HashMap<String, (String, Option<String>)> = playlists
        .iter()
        .filter(|p| p.flag("Folder"))
        .filter_map(|p| {
            Some((
                p.text("Playlist Persistent ID")?,
                (p.text("Name")?, p.text("Parent Persistent ID")),
            ))
        })
        .collect();
    let folder_path = |mut parent: Option<String>| {
        let mut path = Vec::new();
        while let Some(id) = parent {
            let Some((name, up)) = folders.get(&id) else {
                break;
            };
            if path.len() > 32 {
                break;
            }
            path.insert(0, name.clone());
            parent = up.clone();
        }
        path
    };
    let mut smart = 0usize;
    for playlist in playlists {
        if playlist.flag("Master")
            || playlist.flag("Folder")
            || playlist.get("Distinguished Kind").is_some()
            || matches!(playlist.get("Visible"), Some(Value::Bool(false)))
        {
            continue;
        }
        let Some(name) = playlist.text("Name") else {
            continue;
        };
        let from_rules = playlist.get("Smart Criteria").is_some();
        smart += usize::from(from_rules);
        let items = match playlist.get("Playlist Items") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|i| i.integer("Track ID"))
                .filter_map(|id| by_id.get(&id).copied())
                .map(ForeignItem::Track)
                .collect(),
            _ => Vec::new(),
        };
        library.playlists.push(ForeignPlaylist {
            name,
            folder: folder_path(playlist.text("Parent Persistent ID")),
            kind: ForeignPlaylistKind::Static { items, from_rules },
        });
    }
    if smart > 0 {
        library.notes.push(format!(
            "{smart} smart playlists are imported with the tracks they held, \
             as static playlists: their rules cannot be read from the file"
        ));
    }
    library
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(
            parse_date("1970-01-02T00:00:01Z"),
            Some(from_unix_seconds(86_401))
        );
        assert_eq!(
            parse_date("2009-06-01T19:10:24Z"),
            Some(from_unix_seconds(1_243_883_424))
        );
        assert_eq!(parse_date("yesterday"), None);
    }
}
