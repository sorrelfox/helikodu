// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The invented playlists: static and smart playlists in folders, pins, the
//! lock, grouping, tidying and an undo history, kept in memory.
//!
//! A smart playlist's tracks are not found by its rules: the sample picks a
//! stable set of tracks whose size follows the rules' text, which is enough
//! for screenshots and tests.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use super::{Random, SampleLibrary};
use crate::backend::{
    AlbumKey, ArtistKey, BackendError, PinKey, PinRow, PlaylistCleanup, PlaylistKey, PlaylistRow,
    PlaylistType, Query, SmartTrack, TrackKey, TrackRow,
};

/// One entry of a static playlist.
#[derive(Debug, Clone)]
pub(super) enum Entry {
    Track(TrackKey),
    Missing(Box<TrackRow>),
}

/// What an edit changes, to undo it.
#[derive(Debug, Clone)]
struct Snapshot {
    name: String,
    entries: Vec<Entry>,
    query: Option<Query>,
}

#[derive(Debug, Clone)]
pub(super) struct SamplePlaylist {
    pub(super) key: PlaylistKey,
    pub(super) name: String,
    pub(super) entries: Vec<Entry>,
    pub(super) modified: SystemTime,
    kind: PlaylistType,
    parent: Option<PlaylistKey>,
    locked: bool,
    grouped: bool,
    from_file: bool,
    query: Option<Query>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl SamplePlaylist {
    fn new(key: PlaylistKey, name: &str, kind: PlaylistType, parent: Option<PlaylistKey>) -> Self {
        Self {
            key,
            name: name.to_owned(),
            entries: Vec::new(),
            modified: SystemTime::now(),
            kind,
            parent,
            locked: false,
            grouped: false,
            from_file: false,
            query: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub(super) fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
    }

    pub(super) fn set_grouped(&mut self, grouped: bool) {
        self.grouped = grouped;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            name: self.name.clone(),
            entries: self.entries.clone(),
            query: self.query.clone(),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.name = snapshot.name;
        self.entries = snapshot.entries;
        self.query = snapshot.query;
        self.modified = SystemTime::now();
    }
}

#[derive(Debug, Default)]
pub(super) struct Playlists {
    pub(super) next: u64,
    pub(super) lists: Vec<SamplePlaylist>,
    pins: Vec<PinKey>,
}

impl Playlists {
    fn add(&mut self, name: &str, kind: PlaylistType, parent: Option<PlaylistKey>) -> PlaylistKey {
        self.next += 1;
        let key = PlaylistKey(self.next);
        self.lists
            .push(SamplePlaylist::new(key, name, kind, parent));
        key
    }

    fn get(&mut self, key: PlaylistKey) -> Result<&mut SamplePlaylist, BackendError> {
        self.lists
            .iter_mut()
            .find(|p| p.key == key)
            .ok_or_else(|| BackendError("no such playlist".into()))
    }

    /// The playlists in the sidebar's order, with their depth.
    fn tree(&self) -> Vec<(&SamplePlaylist, usize)> {
        let mut out = Vec::new();
        let mut stack: Vec<(&SamplePlaylist, usize)> = self
            .lists
            .iter()
            .filter(|p| p.parent.is_none())
            .rev()
            .map(|p| (p, 0))
            .collect();
        while let Some((list, depth)) = stack.pop() {
            stack.extend(
                self.lists
                    .iter()
                    .filter(|p| p.parent == Some(list.key))
                    .rev()
                    .map(|p| (p, depth + 1)),
            );
            out.push((list, depth));
        }
        out
    }
}

/// The text of the sample's smart playlist rules.
fn parse(text: &str) -> Query {
    helikodu_service::library::parse_query(text).unwrap_or_default()
}

impl SampleLibrary {
    /// Folders, static and smart playlists and pins as the design shows them.
    pub(super) fn add_sample_playlists(&self, r: &mut Random) {
        if self.tracks.is_empty() {
            return;
        }
        let total = self.tracks.len() as u64;
        let mut entries = |count: usize| -> Vec<Entry> {
            (0..count.min(self.tracks.len()))
                .map(|_| Entry::Track(TrackKey(1 + r.next() % total)))
                .collect()
        };
        let mut lists = self.lists();
        let favourites = lists.add("Favourites", PlaylistType::Static, None);
        let workouts = lists.add("Workouts", PlaylistType::Folder, None);
        let intervals = lists.add("Intervals", PlaylistType::Static, Some(workouts));
        let long_run = lists.add("Long run", PlaylistType::Static, Some(workouts));
        let evenings = lists.add("Evenings out", PlaylistType::Folder, None);
        let late = lists.add("Late evening", PlaylistType::Static, None);
        let road = lists.add("Road trip", PlaylistType::Static, None);
        let recent = lists.add("Recently added", PlaylistType::Smart, None);
        let hires = lists.add("Unrated hi-res", PlaylistType::Smart, None);
        let _ = evenings;
        for (key, count) in [
            (favourites, 214),
            (intervals, 32),
            (long_run, 58),
            (late, 48),
            (road, 41),
        ] {
            let filled = entries(count);
            if let Ok(list) = lists.get(key) {
                list.entries = filled;
            }
        }
        if let Ok(list) = lists.get(late) {
            // Runs of one album's tracks, so that the groups show.
            let mut at = 1 + (total / 3);
            list.entries = (0..48)
                .map(|i| {
                    if i % 4 == 0 && i > 0 {
                        at += 9;
                    }
                    let key = TrackKey(1 + (at + i as u64 % 4) % total);
                    Entry::Track(key)
                })
                .collect();
            if list.entries.len() > 4 {
                list.entries[4] = Entry::Missing(Box::new(TrackRow {
                    title: "Kopu Light".into(),
                    artist: "Mari Kask".into(),
                    album_title: "Letters from Hiiumaa".into(),
                    duration: Some(Duration::from_secs(326)),
                    missing: true,
                    ..TrackRow::default()
                }));
            }
            list.grouped = true;
        }
        if let Ok(list) = lists.get(road) {
            list.from_file = true;
        }
        if let Ok(list) = lists.get(recent) {
            list.query = Some(parse("added<3m sort:added desc limit:120"));
        }
        if let Ok(list) = lists.get(hires) {
            list.query = Some(parse(
                "rating=0 samplerate>=88200 (codec:flac OR codec:dsd) added<12m \
                 sort:added desc limit:100",
            ));
        }
        lists.pins = vec![PinKey::Album(AlbumKey(1)), PinKey::Artist(ArtistKey(1))];
    }

    /// The tracks the sample says the rules `query` select.
    fn sample_matches(&self, query: &Query) -> Vec<TrackKey> {
        let total = self.tracks.len() as u64;
        if total == 0 {
            return Vec::new();
        }
        let text = helikodu_service::library::print_query(query);
        let spread = text
            .bytes()
            .fold(7_u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
        let wanted = 40 + spread % 80;
        let limit = query.limit.map_or(u64::MAX, |l| u64::from(l.amount));
        (0..wanted.min(limit).min(total))
            .map(|i| TrackKey(1 + (spread.wrapping_add(i * 37)) % total))
            .collect()
    }

    fn entry_row(&self, entry: &Entry) -> TrackRow {
        match entry {
            Entry::Track(k) => self.track(*k).unwrap_or_default(),
            Entry::Missing(t) => (**t).clone(),
        }
    }

    pub(super) fn sample_playlists(&self) -> Vec<PlaylistRow> {
        let lists = self.lists();
        lists
            .tree()
            .into_iter()
            .map(|(p, depth)| {
                let (items, duration, dead) = match p.kind {
                    PlaylistType::Folder => (
                        lists
                            .lists
                            .iter()
                            .filter(|c| c.parent == Some(p.key))
                            .count(),
                        Duration::ZERO,
                        0,
                    ),
                    PlaylistType::Smart => {
                        let tracks =
                            self.sample_matches(p.query.as_ref().unwrap_or(&Query::default()));
                        let duration = tracks
                            .iter()
                            .filter_map(|k| self.track(*k).and_then(|t| t.duration))
                            .sum();
                        (tracks.len(), duration, 0)
                    }
                    PlaylistType::Static => (
                        p.entries.len(),
                        p.entries
                            .iter()
                            .filter_map(|e| self.entry_row(e).duration)
                            .sum(),
                        p.entries
                            .iter()
                            .filter(|e| matches!(e, Entry::Missing(_)))
                            .count(),
                    ),
                };
                PlaylistRow {
                    key: p.key,
                    name: p.name.clone(),
                    items,
                    duration,
                    modified: Some(p.modified),
                    kind: p.kind,
                    parent: p.parent,
                    depth,
                    locked: p.locked,
                    grouped: p.grouped,
                    from_file: p.from_file,
                    dead,
                    can_undo: !p.undo.is_empty(),
                    can_redo: !p.redo.is_empty(),
                }
            })
            .collect()
    }

    pub(super) fn sample_playlist_items(
        &self,
        key: PlaylistKey,
    ) -> Result<Vec<TrackRow>, BackendError> {
        let entries = self.lists().get(key)?.entries.clone();
        Ok(entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut row = self.entry_row(e);
                row.entry = Some(i as u64);
                row.from_context = true;
                row
            })
            .collect())
    }

    pub(super) fn sample_create(
        &self,
        name: &str,
        kind: PlaylistType,
        parent: Option<PlaylistKey>,
        query: Option<Query>,
    ) -> PlaylistKey {
        let mut lists = self.lists();
        let key = lists.add(name, kind, parent);
        if let Ok(list) = lists.get(key) {
            list.query = query;
        }
        key
    }

    pub(super) fn sample_delete(&self, key: PlaylistKey) {
        let mut lists = self.lists();
        let parent = lists
            .lists
            .iter()
            .find(|p| p.key == key)
            .and_then(|p| p.parent);
        for list in &mut lists.lists {
            if list.parent == Some(key) {
                list.parent = parent;
            }
        }
        lists.lists.retain(|p| p.key != key);
        lists.pins.retain(|p| *p != PinKey::Playlist(key));
    }

    /// Changes the static playlist `key` as an edit that can be undone.
    pub(super) fn edit(
        &self,
        key: PlaylistKey,
        change: impl FnOnce(&mut SamplePlaylist) -> Result<(), BackendError>,
    ) -> Result<(), BackendError> {
        let mut lists = self.lists();
        let list = lists.get(key)?;
        if list.locked {
            return Err(BackendError("the playlist is locked".into()));
        }
        let before = list.snapshot();
        change(list)?;
        list.undo.push(before);
        list.redo.clear();
        list.modified = SystemTime::now();
        Ok(())
    }

    pub(super) fn sample_rename(&self, key: PlaylistKey, name: &str) -> Result<(), BackendError> {
        let mut lists = self.lists();
        let list = lists.get(key)?;
        let before = list.snapshot();
        list.name = name.to_owned();
        list.undo.push(before);
        list.redo.clear();
        Ok(())
    }

    pub(super) fn sample_move(
        &self,
        key: PlaylistKey,
        parent: Option<PlaylistKey>,
        position: Option<usize>,
    ) -> Result<(), BackendError> {
        let mut lists = self.lists();
        if let Some(folder) = parent {
            let mut at = Some(folder);
            while let Some(f) = at {
                if f == key {
                    return Err(BackendError("a folder cannot go into itself".into()));
                }
                at = lists
                    .lists
                    .iter()
                    .find(|p| p.key == f)
                    .and_then(|p| p.parent);
            }
            if lists.get(folder)?.kind != PlaylistType::Folder {
                return Err(BackendError("not a folder".into()));
            }
        }
        let index = lists
            .lists
            .iter()
            .position(|p| p.key == key)
            .ok_or_else(|| BackendError("no such playlist".into()))?;
        let mut list = lists.lists.remove(index);
        list.parent = parent;
        let siblings: Vec<usize> = lists
            .lists
            .iter()
            .enumerate()
            .filter(|(_, p)| p.parent == parent)
            .map(|(i, _)| i)
            .collect();
        let at = match position.and_then(|p| siblings.get(p)) {
            Some(i) => *i,
            None => siblings.last().map_or(lists.lists.len(), |i| i + 1),
        };
        lists.lists.insert(at, list);
        Ok(())
    }

    pub(super) fn sample_duplicate(
        &self,
        key: PlaylistKey,
        name: &str,
    ) -> Result<PlaylistKey, BackendError> {
        let (kind, parent, entries, query) = {
            let mut lists = self.lists();
            let list = lists.get(key)?;
            (
                list.kind,
                list.parent,
                list.entries.clone(),
                list.query.clone(),
            )
        };
        let entries = match kind {
            PlaylistType::Smart => self
                .sample_matches(query.as_ref().unwrap_or(&Query::default()))
                .into_iter()
                .map(Entry::Track)
                .collect(),
            _ => entries,
        };
        let mut lists = self.lists();
        let copy = lists.add(name, PlaylistType::Static, parent);
        lists.get(copy)?.entries = entries;
        Ok(copy)
    }

    pub(super) fn sample_flag(
        &self,
        key: PlaylistKey,
        set: impl FnOnce(&mut SamplePlaylist),
    ) -> Result<(), BackendError> {
        set(self.lists().get(key)?);
        Ok(())
    }

    pub(super) fn sample_playlist_history(
        &self,
        key: PlaylistKey,
        redo: bool,
    ) -> Result<bool, BackendError> {
        let mut lists = self.lists();
        let list = lists.get(key)?;
        if list.locked {
            return Err(BackendError("the playlist is locked".into()));
        }
        let step = if redo {
            list.redo.pop()
        } else {
            list.undo.pop()
        };
        let Some(step) = step else { return Ok(false) };
        let now = list.snapshot();
        if redo {
            list.undo.push(now);
        } else {
            list.redo.push(now);
        }
        list.restore(step);
        Ok(true)
    }

    fn cleanup_positions(entries: &[Entry]) -> (Vec<usize>, Vec<usize>) {
        let mut seen = BTreeSet::new();
        let mut duplicates = Vec::new();
        let mut missing = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            match entry {
                Entry::Track(k) if !seen.insert(*k) => duplicates.push(i),
                Entry::Track(_) => {}
                Entry::Missing(_) => missing.push(i),
            }
        }
        (duplicates, missing)
    }

    pub(super) fn sample_cleanup(&self, key: PlaylistKey) -> Result<PlaylistCleanup, BackendError> {
        let entries = self.lists().get(key)?.entries.clone();
        let (duplicates, missing) = Self::cleanup_positions(&entries);
        Ok(PlaylistCleanup {
            duplicates: duplicates.len(),
            missing: missing.len(),
        })
    }

    pub(super) fn sample_tidy(
        &self,
        key: PlaylistKey,
        duplicates: bool,
    ) -> Result<usize, BackendError> {
        let mut removed = 0;
        self.edit(key, |p| {
            let (dups, missing) = Self::cleanup_positions(&p.entries);
            let gone: BTreeSet<usize> = if duplicates { dups } else { missing }
                .into_iter()
                .collect();
            removed = gone.len();
            let mut i = 0;
            p.entries.retain(|_| {
                let keep = !gone.contains(&i);
                i += 1;
                keep
            });
            Ok(())
        })?;
        Ok(removed)
    }

    pub(super) fn sample_shuffle(&self, key: PlaylistKey) -> Result<(), BackendError> {
        self.edit(key, |p| {
            let n = p.entries.len();
            for i in (1..n).rev() {
                let j = (i * 7919 + 13) % (i + 1);
                p.entries.swap(i, j);
            }
            Ok(())
        })
    }

    pub(super) fn sample_set_query(
        &self,
        key: PlaylistKey,
        query: &Query,
    ) -> Result<(), BackendError> {
        let mut lists = self.lists();
        let list = lists.get(key)?;
        if list.locked {
            return Err(BackendError("the playlist is locked".into()));
        }
        let before = list.snapshot();
        list.query = Some(query.clone());
        list.undo.push(before);
        list.redo.clear();
        list.modified = SystemTime::now();
        Ok(())
    }

    pub(super) fn sample_query(&self, key: PlaylistKey) -> Result<Query, BackendError> {
        Ok(self.lists().get(key)?.query.clone().unwrap_or_default())
    }

    pub(super) fn sample_smart_tracks(
        &self,
        key: PlaylistKey,
    ) -> Result<Vec<SmartTrack>, BackendError> {
        let query = self.sample_query(key)?;
        let now = SystemTime::now();
        Ok(self
            .sample_matches(&query)
            .into_iter()
            .enumerate()
            .filter_map(|(i, k)| {
                Some(SmartTrack {
                    row: self.track(k)?,
                    added: now.checked_sub(Duration::from_secs(86_400 * (i as u64 / 3 + 9))),
                })
            })
            .collect())
    }

    pub(super) fn sample_match_count(&self, query: &Query) -> (usize, usize) {
        (self.sample_matches(query).len(), self.tracks.len())
    }

    pub(super) fn sample_pins(&self) -> Vec<PinRow> {
        let pins = self.lists().pins.clone();
        let names = self.sample_playlists();
        pins.into_iter()
            .filter_map(|key| {
                Some(match key {
                    PinKey::Album(album) => {
                        let row = &self.get(album)?.row;
                        PinRow {
                            key,
                            name: row.title.clone(),
                            cover: row.cover,
                            cover_seed: row.cover_seed,
                            playlist: None,
                        }
                    }
                    PinKey::Artist(artist) => PinRow {
                        key,
                        name: self
                            .artists
                            .get(usize::try_from(artist.0).ok()?.checked_sub(1)?)?
                            .clone(),
                        cover: None,
                        cover_seed: artist.0,
                        playlist: None,
                    },
                    PinKey::Playlist(list) => {
                        let row = names.iter().find(|p| p.key == list)?;
                        PinRow {
                            key,
                            name: row.name.clone(),
                            cover: None,
                            cover_seed: 0,
                            playlist: Some(row.kind),
                        }
                    }
                })
            })
            .collect()
    }

    pub(super) fn sample_pin(&self, key: PinKey, on: bool) {
        let mut lists = self.lists();
        lists.pins.retain(|p| *p != key);
        if on {
            lists.pins.push(key);
        }
    }
}
