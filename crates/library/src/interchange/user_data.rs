// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Helikodu's own user data as a JSON file: ratings, play counts, history,
//! playlists and smart playlists, Favourites, saved searches, labels and the
//! user's computed and custom fields, keyed by path and tags so that it can be
//! read into another library.
//!
//! ```json
//! {
//!   "format": "helikodu-user-data",
//!   "version": 1,
//!   "exported_at": 1763550000,
//!   "tracks": [{
//!     "path": "/home/me/Music/Album/01.flac",
//!     "title": "Song", "artist": "Artist", "album": "Album",
//!     "track_number": 1, "duration_ms": 215000,
//!     "play_count": 12, "skip_count": 1,
//!     "first_played": 1700000000, "last_played": 1763000000, "last_skipped": null,
//!     "rating": 8, "loved": false, "added": 1690000000,
//!     "history": [{"started_at": 1763000000, "played_ms": 215000, "counted": true,
//!                  "context": null}]
//!   }],
//!   "playlists": [
//!     {"name": "Road", "folder": ["Trips"], "kind": "static",
//!      "items": [{"path": "/home/me/Music/Album/01.flac", "title": "Song",
//!                 "artist": "Artist", "album": "Album", "duration_ms": 215000}]},
//!     {"name": "Top rated", "folder": [], "kind": "smart", "query": "rating>=4"}
//!   ],
//!   "favourites": [{"path": "/home/me/Music/Album/01.flac", "title": "Song"}],
//!   "saved_searches": [{"name": "Jazz", "query": "genre:jazz"}],
//!   "labels": [{"name": "late night",
//!               "tracks": [{"path": "/home/me/Music/Album/01.flac", "title": "Song",
//!                           "artist": "Artist", "album": "Album",
//!                           "duration_ms": 215000}],
//!               "albums": [{"album_artist": "Artist", "album": "Album"}],
//!               "artists": ["Artist"]}],
//!   "fields": [{"name": "decade", "kind": "computed",
//!               "definition": "{left(year, 3)}0s"},
//!              {"name": "mood", "kind": "custom", "definition": "MOOD"}]
//! }
//! ```
//!
//! Times are seconds since the Unix epoch; `rating` is in half stars, 0 to 10
//! (absent when not rated). A path that is not valid UTF-8 is given as
//! `path_bytes`, an array of its bytes, instead of `path`. Only tracks with
//! statistics, a rating or history are listed. Reading it back matches each entry
//! by path, then by tags, like an import from another player; counts take the
//! higher of the two values, so that reading a file into the library it came
//! from changes nothing.
//!
//! `labels` lists each label with the tracks (path and tags, as above), the
//! albums (album artist, `null` for various artists, and title) and the artists
//! (by name) it was given to; reading it back gives the label to those that
//! match and takes no label away. Albums match by album artist and title, and
//! artists by name, both in any case and with or without accents. `fields`
//! lists the computed fields (`definition` an expression) and the custom fields
//! (`definition` a tag key) in their order; reading it back defines those the
//! library has no field of that name for, and keeps the library's own definition
//! where it has one.
//!
//! The `version` changes only when a reader of an older version would read the
//! file wrongly. Keys added later are left out of older files, and their readers
//! read such a file as if the key were empty: `labels` and `fields` came after
//! the first files of version 1 and are read as empty when missing.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::ImportError;
use super::foreign::{
    ForeignAlbum, ForeignItem, ForeignLabel, ForeignLibrary, ForeignListen, ForeignPlaylist,
    ForeignPlaylistKind, ForeignTrack,
};
use crate::UserField;
use crate::db::path_from_blob;
use crate::db::unix_time::{from_millis, from_unix_seconds};
use crate::stats::Rating;

/// The `format` of the file.
pub const USER_DATA_FORMAT: &str = "helikodu-user-data";
/// The `version` of the file this build writes and reads.
pub const USER_DATA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Item {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path_bytes: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Listen {
    started_at: i64,
    played_ms: u64,
    counted: bool,
    #[serde(default)]
    context: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct TrackEntry {
    #[serde(flatten)]
    item: Item,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    track_number: Option<u32>,
    #[serde(default)]
    play_count: u64,
    #[serde(default)]
    skip_count: u64,
    #[serde(default)]
    first_played: Option<i64>,
    #[serde(default)]
    last_played: Option<i64>,
    #[serde(default)]
    last_skipped: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rating: Option<u8>,
    #[serde(default)]
    loved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    added: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<Listen>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PlaylistEntry {
    name: String,
    #[serde(default)]
    folder: Vec<String>,
    kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    items: Vec<Item>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SavedEntry {
    name: String,
    query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AlbumEntry {
    #[serde(default)]
    album_artist: Option<String>,
    album: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LabelEntry {
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tracks: Vec<Item>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    albums: Vec<AlbumEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artists: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FieldEntry {
    name: String,
    kind: String,
    definition: String,
}

/// `user_fields.kind` of a computed field, and of a custom field.
pub(super) const FIELD_COMPUTED: i64 = 0;
pub(super) const FIELD_CUSTOM: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Document {
    format: String,
    version: u32,
    #[serde(default)]
    exported_at: i64,
    #[serde(default)]
    tracks: Vec<TrackEntry>,
    #[serde(default)]
    playlists: Vec<PlaylistEntry>,
    #[serde(default)]
    favourites: Vec<Item>,
    #[serde(default)]
    saved_searches: Vec<SavedEntry>,
    #[serde(default)]
    labels: Vec<LabelEntry>,
    #[serde(default)]
    fields: Vec<FieldEntry>,
}

/// An export of the user data, ready to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDataExport {
    document: Document,
}

impl UserDataExport {
    /// The tracks listed.
    pub fn tracks(&self) -> usize {
        self.document.tracks.len()
    }

    /// The history entries listed.
    pub fn history(&self) -> usize {
        self.document.tracks.iter().map(|t| t.history.len()).sum()
    }

    /// The playlists listed (smart ones included).
    pub fn playlists(&self) -> usize {
        self.document.playlists.len()
    }

    /// The favourites listed.
    pub fn favourites(&self) -> usize {
        self.document.favourites.len()
    }

    /// The saved searches listed.
    pub fn saved_searches(&self) -> usize {
        self.document.saved_searches.len()
    }

    /// The labels listed.
    pub fn labels(&self) -> usize {
        self.document.labels.len()
    }

    /// The tracks, albums and artists given a label, one for each label given.
    pub fn label_items(&self) -> usize {
        self.document
            .labels
            .iter()
            .map(|l| l.tracks.len() + l.albums.len() + l.artists.len())
            .sum()
    }

    /// The computed and custom fields listed.
    pub fn fields(&self) -> usize {
        self.document.fields.len()
    }

    /// The JSON text.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.document).unwrap_or_default()
    }
}

fn path_item(path: &Path) -> Item {
    match path.to_str() {
        Some(text) => Item {
            path: Some(text.to_owned()),
            ..Item::default()
        },
        None => Item {
            path_bytes: Some(path.as_os_str().as_bytes().to_vec()),
            ..Item::default()
        },
    }
}

/// The location of an item as a `file://` URI, so that any bytes survive.
fn location(item: &Item) -> Option<String> {
    let bytes = match (&item.path, &item.path_bytes) {
        (Some(path), _) => path.as_bytes().to_vec(),
        (None, Some(bytes)) => bytes.clone(),
        (None, None) => return None,
    };
    if !bytes.starts_with(b"/") {
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    let mut uri = String::from("file://");
    for b in bytes {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    Some(uri)
}

fn foreign(item: &Item) -> ForeignTrack {
    ForeignTrack {
        location: location(item),
        title: item.title.clone(),
        artist: item.artist.clone(),
        album: item.album.clone(),
        duration: item.duration_ms.map(Duration::from_millis),
        ..ForeignTrack::default()
    }
}

// --- Export -------------------------------------------------------------------------

/// The item of identity `identity`: the path and tags of its main track.
fn identity_item(c: &Connection, identity: i64) -> rusqlite::Result<Option<(Item, Option<u32>)>> {
    c.prepare_cached(
        "SELECT d.path, f.name, t.title, t.artist, a.title, t.duration_ms, t.track_no
         FROM tracks t JOIN files f ON f.id = t.file_id JOIN dirs d ON d.id = f.dir_id
         LEFT JOIN albums a ON a.id = t.album_id
         WHERE t.identity_id = ?1
         ORDER BY f.missing_since IS NOT NULL, t.is_copy, t.id LIMIT 1",
    )?
    .query_row([identity], |row| {
        let path = path_from_blob(row.get(0)?).join(path_from_blob(row.get(1)?));
        let mut item = path_item(&path);
        item.title = row.get(2)?;
        item.artist = row.get(3)?;
        item.album = row.get(4)?;
        item.duration_ms = row
            .get::<_, Option<i64>>(5)?
            .and_then(|d| u64::try_from(d).ok());
        let track_no = row
            .get::<_, Option<i64>>(6)?
            .and_then(|n| u32::try_from(n).ok());
        Ok((item, track_no))
    })
    .optional()
}

fn export_tracks(c: &Connection) -> rusqlite::Result<Vec<TrackEntry>> {
    let identities: Vec<i64> = c
        .prepare(
            "SELECT identity_id FROM stats
             WHERE play_count > 0 OR skip_count > 0 OR rating IS NOT NULL OR loved IS NOT NULL
                OR first_played IS NOT NULL
             UNION SELECT identity_id FROM history
             ORDER BY 1",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stats = c.prepare_cached(
        "SELECT play_count, skip_count, first_played, last_played, last_skipped, rating, loved
         FROM stats WHERE identity_id = ?1",
    )?;
    let mut history = c.prepare_cached(
        "SELECT started_at, played_ms, counted, context FROM history
         WHERE identity_id = ?1 ORDER BY started_at, id",
    )?;
    let mut added = c.prepare_cached("SELECT min(added_at) FROM tracks WHERE identity_id = ?1")?;
    let mut entries = Vec::with_capacity(identities.len());
    for identity in identities {
        let Some((item, track_number)) = identity_item(c, identity)? else {
            continue;
        };
        let mut entry = TrackEntry {
            item,
            track_number,
            added: added.query_row([identity], |row| row.get(0))?,
            ..TrackEntry::default()
        };
        stats
            .query_row([identity], |row| {
                entry.play_count = u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0);
                entry.skip_count = u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0);
                entry.first_played = row.get(2)?;
                entry.last_played = row.get(3)?;
                entry.last_skipped = row.get(4)?;
                entry.rating = row
                    .get::<_, Option<i64>>(5)?
                    .and_then(|r| u8::try_from(r).ok());
                entry.loved = row.get::<_, Option<i64>>(6)? == Some(1);
                Ok(())
            })
            .optional()?;
        entry.history = history
            .query_map([identity], |row| {
                Ok(Listen {
                    started_at: row.get(0)?,
                    played_ms: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                    counted: row.get(2)?,
                    context: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        entries.push(entry);
    }
    Ok(entries)
}

/// An item as stored: its identity, the kind and bytes of its reference, and
/// the tags it was added with.
type StoredItem = (Option<i64>, i64, Option<Vec<u8>>, Item);

/// A playlist as stored: id, parent, name, kind, query and whether it is built in.
type PlaylistRow = (i64, Option<i64>, String, i64, Option<String>, bool);

/// The items of the playlist `id`.
fn playlist_items(c: &Connection, id: i64) -> rusqlite::Result<Vec<Item>> {
    let rows: Vec<StoredItem> = c
        .prepare_cached(
            "SELECT identity_id, ref_kind, missing_ref, title, artist, album, duration_ms
             FROM playlist_items WHERE playlist_id = ?1 ORDER BY position",
        )?
        .query_map([id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                Item {
                    title: row.get(3)?,
                    artist: row.get(4)?,
                    album: row.get(5)?,
                    duration_ms: row
                        .get::<_, Option<i64>>(6)?
                        .and_then(|d| u64::try_from(d).ok()),
                    ..Item::default()
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut items = Vec::with_capacity(rows.len());
    for (identity, kind, reference, stored) in rows {
        let item = match identity.map(|i| identity_item(c, i)).transpose()?.flatten() {
            Some((item, _)) => item,
            None => {
                let mut item = stored;
                if let Some(bytes) = reference.filter(|_| kind == crate::playlists::REF_PATH) {
                    let path = path_item(&path_from_blob(bytes));
                    item.path = path.path;
                    item.path_bytes = path.path_bytes;
                }
                item
            }
        };
        items.push(item);
    }
    Ok(items)
}

fn export_playlists(c: &Connection) -> rusqlite::Result<(Vec<PlaylistEntry>, Vec<Item>)> {
    let rows: Vec<PlaylistRow> = c
        .prepare(
            "SELECT id, parent_id, name, kind, query, builtin != 0 FROM playlists
             ORDER BY parent_id IS NOT NULL, position, id",
        )?
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let folders: std::collections::HashMap<i64, (String, Option<i64>)> = rows
        .iter()
        .filter(|r| r.3 == crate::playlists::FOLDER)
        .map(|r| (r.0, (r.2.clone(), r.1)))
        .collect();
    let path_of = |mut parent: Option<i64>| {
        let mut path = Vec::new();
        while let Some(id) = parent {
            let Some((name, up)) = folders.get(&id) else {
                break;
            };
            if path.len() > 64 {
                break;
            }
            path.insert(0, name.clone());
            parent = *up;
        }
        path
    };
    let mut playlists = Vec::new();
    let mut favourites = Vec::new();
    for (id, parent, name, kind, query, builtin) in &rows {
        if *builtin {
            favourites = playlist_items(c, *id)?;
            continue;
        }
        let entry = match *kind {
            crate::playlists::SMART => PlaylistEntry {
                name: name.clone(),
                folder: path_of(*parent),
                kind: "smart".to_owned(),
                items: Vec::new(),
                query: query.clone(),
            },
            crate::playlists::STATIC => PlaylistEntry {
                name: name.clone(),
                folder: path_of(*parent),
                kind: "static".to_owned(),
                items: playlist_items(c, *id)?,
                query: None,
            },
            _ => continue,
        };
        playlists.push(entry);
    }
    Ok((playlists, favourites))
}

fn export_labels(c: &Connection) -> rusqlite::Result<Vec<LabelEntry>> {
    let labels: Vec<(i64, String)> = c
        .prepare("SELECT id, name FROM labels ORDER BY name COLLATE HK_LOCALE, id")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut entries = Vec::with_capacity(labels.len());
    for (id, name) in labels {
        let identities: Vec<i64> = c
            .prepare_cached(
                "SELECT identity_id FROM label_tracks WHERE label_id = ?1 ORDER BY identity_id",
            )?
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut tracks = Vec::with_capacity(identities.len());
        for identity in identities {
            if let Some((item, _)) = identity_item(c, identity)? {
                tracks.push(item);
            }
        }
        // Albums that are in the library now, so that they have a title.
        let albums = c
            .prepare_cached(
                "SELECT al.artist, al.title FROM label_albums la
                 JOIN albums al ON al.album_key = la.album_key
                 WHERE la.label_id = ?1 AND al.title IS NOT NULL
                 ORDER BY al.artist_sort, al.title_sort, al.id",
            )?
            .query_map([id], |row| {
                Ok(AlbumEntry {
                    album_artist: row.get(0)?,
                    album: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let artists = c
            .prepare_cached(
                "SELECT coalesce((SELECT p.name FROM people p
                                  WHERE p.fold = lr.artist_fold ORDER BY p.id LIMIT 1),
                                 lr.artist_fold)
                 FROM label_artists lr WHERE lr.label_id = ?1
                 ORDER BY 1 COLLATE HK_LOCALE",
            )?
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let entry = LabelEntry {
            name,
            tracks,
            albums,
            artists,
        };
        if !(entry.tracks.is_empty() && entry.albums.is_empty() && entry.artists.is_empty()) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn export_fields(c: &Connection) -> rusqlite::Result<Vec<FieldEntry>> {
    c.prepare("SELECT name, kind, definition FROM user_fields ORDER BY position, name")?
        .query_map([], |row| {
            Ok(FieldEntry {
                name: row.get(0)?,
                kind: match row.get::<_, i64>(1)? {
                    FIELD_CUSTOM => "custom",
                    _ => "computed",
                }
                .to_owned(),
                definition: row.get(2)?,
            })
        })?
        .collect()
}

/// The user data of the library behind `c`.
pub(super) fn export(c: &Connection, now: i64) -> rusqlite::Result<UserDataExport> {
    let tracks = export_tracks(c)?;
    let (playlists, favourites) = export_playlists(c)?;
    let saved_searches = c
        .prepare("SELECT name, query FROM saved_searches ORDER BY position, id")?
        .query_map([], |row| {
            Ok(SavedEntry {
                name: row.get(0)?,
                query: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let labels = export_labels(c)?;
    let fields = export_fields(c)?;
    Ok(UserDataExport {
        document: Document {
            format: USER_DATA_FORMAT.to_owned(),
            version: USER_DATA_VERSION,
            exported_at: now,
            tracks,
            playlists,
            favourites,
            saved_searches,
            labels,
            fields,
        },
    })
}

// --- Import -------------------------------------------------------------------------

/// Reads an export file into the form the importers share.
pub(super) fn read(path: &Path) -> Result<ForeignLibrary, ImportError> {
    let bytes = std::fs::read(path).map_err(|source| ImportError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse(&bytes).map_err(|reason| ImportError::Format {
        path: path.to_path_buf(),
        reason,
    })
}

/// Reads export text into the form the importers share.
pub(super) fn parse(bytes: &[u8]) -> Result<ForeignLibrary, String> {
    let document: Document =
        serde_json::from_slice(bytes).map_err(|e| format!("not a user data file: {e}"))?;
    if document.format != USER_DATA_FORMAT {
        return Err(format!(
            "not a user data file: the format is \"{}\"",
            document.format
        ));
    }
    if document.version > USER_DATA_VERSION {
        return Err(format!(
            "the file has version {}; this version of Helikodu reads up to version {}",
            document.version, USER_DATA_VERSION
        ));
    }
    let mut library = ForeignLibrary::default();
    for entry in &document.tracks {
        let mut track = foreign(&entry.item);
        track.track_number = entry.track_number;
        track.play_count = entry.play_count;
        track.skip_count = entry.skip_count;
        track.first_played = entry.first_played.map(from_unix_seconds);
        track.last_played = entry.last_played.map(from_unix_seconds);
        track.last_skipped = entry.last_skipped.map(from_unix_seconds);
        track.rating = entry.rating.and_then(Rating::from_half_stars);
        track.loved = entry.loved;
        track.added = entry.added.map(from_unix_seconds);
        track.history = entry
            .history
            .iter()
            .map(|l| ForeignListen {
                started_at: from_unix_seconds(l.started_at),
                played: from_millis(i64::try_from(l.played_ms).unwrap_or(i64::MAX)),
                counted: l.counted,
                context: l.context.clone(),
            })
            .collect();
        library.tracks.push(track);
    }
    let other = |item: &Item| ForeignItem::Other(Box::new(foreign(item)));
    for playlist in &document.playlists {
        let kind = match (playlist.kind.as_str(), &playlist.query) {
            ("smart", Some(query)) => ForeignPlaylistKind::Smart(query.clone()),
            ("static", _) => ForeignPlaylistKind::Static {
                items: playlist.items.iter().map(other).collect(),
                from_rules: false,
            },
            _ => ForeignPlaylistKind::Unmapped(format!(
                "the kind \"{}\" is not known",
                playlist.kind
            )),
        };
        library.playlists.push(ForeignPlaylist {
            name: playlist.name.clone(),
            folder: playlist.folder.clone(),
            kind,
        });
    }
    library.favourites = document.favourites.iter().map(other).collect();
    library.saved_searches = document
        .saved_searches
        .iter()
        .map(|s| (s.name.clone(), s.query.clone()))
        .collect();
    library.labels = document
        .labels
        .iter()
        .map(|label| ForeignLabel {
            name: label.name.clone(),
            tracks: label.tracks.iter().map(foreign).collect(),
            albums: label
                .albums
                .iter()
                .map(|a| ForeignAlbum {
                    album_artist: a.album_artist.clone(),
                    album: a.album.clone(),
                })
                .collect(),
            artists: label.artists.clone(),
        })
        .collect();
    for field in &document.fields {
        match field.kind.as_str() {
            "computed" => library
                .fields
                .push(UserField::computed(&field.name, &field.definition)),
            "custom" => library
                .fields
                .push(UserField::tag(&field.name, &field.definition)),
            other => library.notes.push(format!(
                "the field {} was left out: the kind \"{other}\" is not known",
                field.name
            )),
        }
    }
    Ok(library)
}

/// Writes `export` to `path`, replacing the file only once it is complete.
pub(super) fn write(export: &UserDataExport, path: &Path) -> Result<(), ImportError> {
    crate::playlists::write_atomically(path, export.to_json().as_bytes()).map_err(|source| {
        ImportError::Io {
            path: PathBuf::from(path),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_keep_their_bytes() {
        let item = Item {
            path_bytes: Some(b"/m/\xffa b.mp3".to_vec()),
            ..Item::default()
        };
        let uri = location(&item).unwrap();
        assert_eq!(uri, "file:///m/%FFa%20b.mp3");
        assert_eq!(super::super::uri::decode(&uri), b"/m/\xffa b.mp3");
    }

    #[test]
    fn other_formats_are_refused() {
        assert!(parse(br#"{"format": "else", "version": 1}"#).is_err());
        assert!(parse(br#"{"format": "helikodu-user-data", "version": 99}"#).is_err());
        let empty = parse(br#"{"format": "helikodu-user-data", "version": 1}"#).unwrap();
        assert!(empty.tracks.is_empty());
    }
}
