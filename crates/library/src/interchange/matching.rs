// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Taking entries of another library to the tracks of this one: by path first,
//! then by tags.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::Connection;

use super::foreign::ForeignTrack;
use super::uri::{PathMapping, local_path};
use crate::db::{path_from_blob, path_to_blob};
use crate::search::fold;

/// The most a length may differ for a match by tags, in milliseconds.
const LENGTH_TOLERANCE_MS: i64 = 2000;

/// A track of the library that entries can match.
#[derive(Debug, Clone)]
struct Candidate {
    track: i64,
    identity: i64,
    album: Option<String>,
    track_no: Option<i64>,
    duration_ms: Option<i64>,
}

/// How an entry matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Match {
    /// The track and its identity, found by path (`by_path`) or by tags.
    Track {
        track: i64,
        identity: i64,
        by_path: bool,
    },
    /// Tracks of this many identities fit the tags.
    Ambiguous(usize),
    /// Nothing fits.
    None,
}

/// The tracks of the library by path and by folded artist and title.
#[derive(Debug, Default)]
pub(super) struct LibraryIndex {
    by_path: HashMap<Vec<u8>, (i64, i64)>,
    by_tags: HashMap<(String, String), Vec<Candidate>>,
}

fn folded(text: Option<&str>) -> Option<String> {
    text.map(fold).filter(|t| !t.is_empty())
}

impl LibraryIndex {
    /// Reads the index of every track of the library.
    pub(super) fn load(c: &Connection) -> rusqlite::Result<Self> {
        let mut index = Self::default();
        let mut statement = c.prepare(
            "SELECT t.id, t.identity_id, d.path, f.name, t.sub_index, t.title_fold,
                    t.artist, t.main_artist, a.title, t.track_no, t.duration_ms
             FROM tracks t JOIN files f ON f.id = t.file_id JOIN dirs d ON d.id = f.dir_id
             LEFT JOIN albums a ON a.id = t.album_id
             ORDER BY t.is_copy, t.sub_index, t.id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let track: i64 = row.get(0)?;
            let identity: i64 = row.get(1)?;
            let path = path_from_blob(row.get(2)?).join(path_from_blob(row.get(3)?));
            index
                .by_path
                .entry(path_to_blob(&path).to_vec())
                .or_insert((track, identity));
            let title: Option<String> = row.get(5)?;
            let Some(title) = title.filter(|t| !t.is_empty()) else {
                continue;
            };
            let candidate = Candidate {
                track,
                identity,
                album: folded(row.get::<_, Option<String>>(8)?.as_deref()),
                track_no: row.get(9)?,
                duration_ms: row.get(10)?,
            };
            let artist: Option<String> = row.get(6)?;
            let main: Option<String> = row.get(7)?;
            let artist = folded(artist.as_deref());
            let main = folded(main.as_deref());
            for name in [&artist, &main].into_iter().flatten() {
                let list = index
                    .by_tags
                    .entry((name.clone(), title.clone()))
                    .or_default();
                if !list.iter().any(|c| c.track == candidate.track) {
                    list.push(candidate.clone());
                }
            }
        }
        Ok(index)
    }

    /// The track of the file at `path`, with its identity.
    pub(super) fn at_path(&self, path: &std::path::Path) -> Option<(i64, i64)> {
        self.by_path.get(path_to_blob(path)).copied()
    }

    /// The match of `entry` under `mappings`.
    pub(super) fn find(&self, entry: &ForeignTrack, mappings: &[PathMapping]) -> Match {
        if let Some(path) = entry
            .location
            .as_deref()
            .and_then(|l| local_path(l, mappings))
            && let Some((track, identity)) = self.at_path(&path)
        {
            return Match::Track {
                track,
                identity,
                by_path: true,
            };
        }
        self.by_tags(entry)
    }

    fn by_tags(&self, entry: &ForeignTrack) -> Match {
        let (Some(artist), Some(title)) = (
            folded(entry.artist.as_deref()),
            folded(entry.title.as_deref()),
        ) else {
            return Match::None;
        };
        let Some(candidates) = self.by_tags.get(&(artist, title)) else {
            return Match::None;
        };
        let album = folded(entry.album.as_deref());
        let track_no = entry.track_number.map(i64::from);
        let duration = entry
            .duration
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        let fits = |c: &&Candidate| {
            let album_differs = matches!((&album, &c.album), (Some(a), Some(b)) if a != b);
            let number_differs = matches!((track_no, c.track_no), (Some(a), Some(b)) if a != b);
            let length_differs = matches!(
                (duration, c.duration_ms),
                (Some(a), Some(b)) if (a - b).abs() > LENGTH_TOLERANCE_MS
            );
            !album_differs && !number_differs && !length_differs
        };
        let fitting: Vec<&Candidate> = candidates.iter().filter(fits).collect();
        let mut identities: Vec<i64> = fitting.iter().map(|c| c.identity).collect();
        identities.sort_unstable();
        identities.dedup();
        match (identities.len(), fitting.first()) {
            (1, Some(first)) => Match::Track {
                track: first.track,
                identity: first.identity,
                by_path: false,
            },
            (0, _) | (_, None) => Match::None,
            (n, _) => Match::Ambiguous(n),
        }
    }
}

/// Words for an entry in a report: its artist and title, or its location.
pub(super) fn describe(entry: &ForeignTrack) -> String {
    match (&entry.artist, &entry.title) {
        (Some(artist), Some(title)) => format!("{artist} – {title}"),
        (None, Some(title)) => title.clone(),
        _ => entry
            .location
            .as_deref()
            .and_then(|l| local_path(l, &[]))
            .map_or_else(
                || entry.location.clone().unwrap_or_default(),
                |p: PathBuf| p.to_string_lossy().into_owned(),
            ),
    }
}
