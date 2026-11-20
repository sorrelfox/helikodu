// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The invented labels: given to some tracks, albums and artists of the sample,
//! with the names the design shows, kept in memory. A label of an album counts
//! for its tracks, one of an artist for the tracks of their albums. Each change
//! keeps the labels as they were before it, which its undo puts back.

use std::collections::BTreeSet;

use super::SampleLibrary;
use crate::backend::{
    AlbumKey, BackendError, LabelChoice, LabelChoices, LabelContents, LabelDone, LabelEdit,
    LabelEditError, LabelFor, LabelKey, LabelRow, LabelState, LabelUndo, TrackKey,
};
use crate::i18n::tr;

/// One label: what it was given to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Label {
    key: u64,
    name: String,
    tracks: BTreeSet<TrackKey>,
    albums: BTreeSet<AlbumKey>,
    /// Artists, by their index.
    artists: BTreeSet<usize>,
}

impl Label {
    fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.albums.is_empty() && self.artists.is_empty()
    }
}

/// Every label of the sample.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SampleLabels {
    next: u64,
    labels: Vec<Label>,
}

/// A label's name as labels are told apart: without case.
fn fold(name: &str) -> String {
    name.to_lowercase()
}

/// `name` as a label: trimmed, with one space between words.
fn clean(name: &str) -> Result<String, LabelEditError> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        Err(LabelEditError::Failed(BackendError(tr(
            "A label needs a name.",
        ))))
    } else {
        Ok(name)
    }
}

/// The names of the design's labels, with what each is given: (every n-th track
/// from the first, album positions, artist positions).
const INVENTED: &[(&str, usize, &[u64], &[usize])] = &[
    ("after hours", 61, &[7], &[]),
    ("evening walk", 113, &[], &[]),
    ("favourite", 0, &[], &[]),
    ("for the car", 83, &[11, 23], &[]),
    ("late night", 0, &[1, 2], &[]),
    ("live", 151, &[17], &[]),
    ("night drive", 127, &[], &[]),
    ("night shift", 0, &[3], &[]),
    ("road trip", 97, &[1, 5], &[]),
    ("to sort", 41, &[], &[]),
    ("winter", 139, &[29], &[]),
    ("work", 67, &[13, 19], &[4]),
];

impl SampleLibrary {
    fn labels_now(&self) -> std::sync::MutexGuard<'_, SampleLabels> {
        self.labels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Gives the design's labels to tracks, albums and artists of the sample.
    pub(super) fn add_sample_labels(&self) {
        let tracks = self.tracks.len() as u64;
        let albums = self.albums.len() as u64;
        let mut state = SampleLabels::default();
        for (name, every, on_albums, on_artists) in INVENTED {
            state.next += 1;
            let mut label = Label {
                key: state.next,
                name: (*name).to_owned(),
                tracks: BTreeSet::new(),
                albums: on_albums
                    .iter()
                    .filter(|a| **a <= albums)
                    .map(|a| AlbumKey(*a))
                    .collect(),
                artists: on_artists
                    .iter()
                    .filter(|a| **a < self.artists.len())
                    .copied()
                    .collect(),
            };
            if *every > 0 {
                // From the twelfth album or so on, clear of the first albums.
                let mut key = 100 + state.next * 7;
                while key <= tracks {
                    label.tracks.insert(TrackKey(key));
                    key += *every as u64;
                }
            }
            match *name {
                // The artist of the first album, and a few tracks of their others.
                "favourite" => {
                    if let Some(first) = self.albums.first() {
                        label.artists.insert(first.artist);
                    }
                }
                // Fourteen tracks one by one, the artist of the third album.
                "late night" => {
                    for album in self.albums.iter().skip(10).take(14) {
                        if let Some(track) = album.tracks.first() {
                            label.tracks.insert(track.key);
                        }
                    }
                    if let Some(third) = self.albums.get(2) {
                        label.artists.insert(third.artist);
                    }
                }
                // Part of the tracks of the third album have this one too.
                "night drive" => {
                    if let Some(third) = self.albums.get(2) {
                        label
                            .tracks
                            .extend(third.tracks.iter().take(2).map(|t| t.key));
                    }
                }
                _ => {}
            }
            if !label.is_empty() {
                state.labels.push(label);
            }
        }
        *self.labels_now() = state;
    }

    /// The tracks of an album.
    fn album_tracks(&self, album: AlbumKey) -> impl Iterator<Item = TrackKey> + '_ {
        self.get(album)
            .into_iter()
            .flat_map(|a| a.tracks.iter().map(|t| t.key))
    }

    /// Every track that has `label`.
    fn tracks_of(&self, label: &Label) -> BTreeSet<TrackKey> {
        let mut tracks = label.tracks.clone();
        for album in &label.albums {
            tracks.extend(self.album_tracks(*album));
        }
        for (i, album) in self.albums.iter().enumerate() {
            if label.artists.contains(&album.artist) {
                tracks.extend(self.album_tracks(AlbumKey(i as u64 + 1)));
            }
        }
        tracks
    }

    fn label_row(&self, label: &Label) -> LabelRow {
        LabelRow {
            key: LabelKey(label.key),
            name: label.name.clone(),
            tracks: self.tracks_of(label).len(),
            own_tracks: label.tracks.len(),
            albums: label.albums.len(),
            artists: label.artists.len(),
        }
    }

    pub(super) fn sample_labels(&self) -> Vec<LabelRow> {
        let state = self.labels_now().clone();
        let mut rows: Vec<LabelRow> = state.labels.iter().map(|l| self.label_row(l)).collect();
        rows.sort_by(|a, b| fold(&a.name).cmp(&fold(&b.name)).then(a.key.cmp(&b.key)));
        rows
    }

    pub(super) fn sample_label_choices(&self, target: &LabelFor) -> LabelChoices {
        let state = self.labels_now().clone();
        let (items, marks): (usize, Vec<[usize; 3]>) = match target {
            LabelFor::Tracks(keys) => {
                let keys: BTreeSet<TrackKey> = keys
                    .iter()
                    .copied()
                    .filter(|k| self.track_row_unpatched(*k).is_some())
                    .collect();
                let marks = state
                    .labels
                    .iter()
                    .map(|label| {
                        let mut n = [0; 3];
                        for key in &keys {
                            let Some(row) = self.track_row_unpatched(*key) else {
                                continue;
                            };
                            let artist = self.get(row.album).map(|a| a.artist);
                            if label.tracks.contains(key) {
                                n[0] += 1;
                            } else if label.albums.contains(&row.album) {
                                n[1] += 1;
                            } else if artist.is_some_and(|a| label.artists.contains(&a)) {
                                n[2] += 1;
                            }
                        }
                        n
                    })
                    .collect();
                (keys.len(), marks)
            }
            LabelFor::Album(album) => {
                let known = self.get(*album).is_some();
                let marks = state
                    .labels
                    .iter()
                    .map(|l| [usize::from(known && l.albums.contains(album)), 0, 0])
                    .collect();
                (usize::from(known), marks)
            }
            LabelFor::Artist(artist) => {
                let index = Self::artist_index(*artist).filter(|i| *i < self.artists.len());
                let marks = state
                    .labels
                    .iter()
                    .map(|l| {
                        [
                            usize::from(index.is_some_and(|i| l.artists.contains(&i))),
                            0,
                            0,
                        ]
                    })
                    .collect();
                (usize::from(index.is_some()), marks)
            }
        };
        let mut labels: Vec<LabelChoice> = state
            .labels
            .iter()
            .zip(marks)
            .map(|(label, [own, album, artist])| LabelChoice {
                key: LabelKey(label.key),
                name: label.name.clone(),
                tracks: self.tracks_of(label).len(),
                state: LabelState::of(items, own, album, artist),
            })
            .collect();
        labels.sort_by(|a, b| fold(&a.name).cmp(&fold(&b.name)).then(a.key.cmp(&b.key)));
        LabelChoices { items, labels }
    }

    pub(super) fn sample_change_label(
        &self,
        edit: &LabelEdit,
    ) -> Result<LabelDone, LabelEditError> {
        let mut state = self.labels_now();
        let before = state.clone();
        let find = |state: &SampleLabels, name: &str| {
            state
                .labels
                .iter()
                .position(|l| fold(&l.name) == fold(name))
        };
        let unknown =
            |name: &str| LabelEditError::Failed(BackendError(format!("there is no label {name}")));
        let mut count = 0;
        match edit {
            LabelEdit::Give { target, name } | LabelEdit::Take { target, name } => {
                let give = matches!(edit, LabelEdit::Give { .. });
                let name = clean(name)?;
                let index = match find(&state, &name) {
                    Some(index) => {
                        if give {
                            state.labels[index].name.clone_from(&name);
                        }
                        index
                    }
                    None if give => {
                        state.next += 1;
                        let key = state.next;
                        state.labels.push(Label {
                            key,
                            name: name.clone(),
                            tracks: BTreeSet::new(),
                            albums: BTreeSet::new(),
                            artists: BTreeSet::new(),
                        });
                        state.labels.len() - 1
                    }
                    None => return Err(unknown(&name)),
                };
                let label = &mut state.labels[index];
                let mut apply = |changed: bool| count += usize::from(changed);
                match target {
                    LabelFor::Tracks(keys) => {
                        for key in keys {
                            if self.track_row_unpatched(*key).is_some() {
                                apply(if give {
                                    label.tracks.insert(*key)
                                } else {
                                    label.tracks.remove(key)
                                });
                            }
                        }
                    }
                    LabelFor::Album(album) => apply(if give {
                        label.albums.insert(*album)
                    } else {
                        label.albums.remove(album)
                    }),
                    LabelFor::Artist(artist) => {
                        let Some(index) = Self::artist_index(*artist) else {
                            return Err(LabelEditError::Failed(BackendError(tr(
                                "There is no such artist.",
                            ))));
                        };
                        apply(if give {
                            label.artists.insert(index)
                        } else {
                            label.artists.remove(&index)
                        });
                    }
                }
                if label.is_empty() {
                    state.labels.remove(index);
                }
            }
            LabelEdit::Rename { name, new_name } => {
                let new_name = clean(new_name)?;
                let index = find(&state, name).ok_or_else(|| unknown(name))?;
                if let Some(other) = find(&state, &new_name)
                    && other != index
                {
                    return Err(LabelEditError::Exists(state.labels[other].name.clone()));
                }
                state.labels[index].name = new_name;
            }
            LabelEdit::Merge { from, into } => {
                let source = find(&state, from).ok_or_else(|| unknown(from))?;
                let target = find(&state, into).ok_or_else(|| unknown(into))?;
                if source == target {
                    return Err(LabelEditError::Failed(BackendError(tr(
                        "A label cannot be merged into itself.",
                    ))));
                }
                let gone = state.labels[source].clone();
                let label = &mut state.labels[target];
                label.tracks.extend(gone.tracks);
                label.albums.extend(gone.albums);
                label.artists.extend(gone.artists);
                state.labels.remove(source);
            }
            LabelEdit::Delete { name } => {
                let index = find(&state, name).ok_or_else(|| unknown(name))?;
                state.labels.remove(index);
            }
        }
        Ok(LabelDone {
            count,
            undo: LabelUndo::step(before),
        })
    }

    pub(super) fn sample_undo_labels(&self, undo: &LabelUndo) {
        // Each step holds every label as it was; the first, as it was before them all.
        if let Some(first) = undo.steps::<SampleLabels>().next() {
            *self.labels_now() = first.clone();
        }
    }

    pub(super) fn sample_label_contents(&self, key: LabelKey) -> Option<LabelContents> {
        let label = self
            .labels_now()
            .labels
            .iter()
            .find(|l| l.key == key.0)
            .cloned()?;
        let mut artists: Vec<_> = label
            .artists
            .iter()
            .filter(|i| **i < self.artists.len())
            .map(|i| self.artist_row(*i))
            .collect();
        artists.sort_by(|a, b| a.sort_name.cmp(&b.sort_name));
        Some(LabelContents {
            row: self.label_row(&label),
            artists,
            albums: label
                .albums
                .iter()
                .filter_map(|a| self.get(*a).map(|a| a.row.clone()))
                .collect(),
            tracks: label.tracks.iter().filter_map(|t| self.track(*t)).collect(),
        })
    }

    /// Every label of the track `key`, given to it or to its album or its
    /// album's artist, in name order: what the track table's Labels column, the
    /// facets and `label:` read.
    pub(super) fn labels_of_track(&self, key: TrackKey) -> Vec<String> {
        let Some(row) = self.track_row_unpatched(key) else {
            return Vec::new();
        };
        let artist = self.get(row.album).map(|a| a.artist);
        let mut names: Vec<String> = self
            .labels_now()
            .labels
            .iter()
            .filter(|l| {
                l.tracks.contains(&key)
                    || l.albums.contains(&row.album)
                    || artist.is_some_and(|a| l.artists.contains(&a))
            })
            .map(|l| l.name.clone())
            .collect();
        names.sort_by_key(|n| fold(n));
        names
    }

    pub(super) fn sample_label_track_keys(&self, key: LabelKey) -> Vec<TrackKey> {
        let label = self
            .labels_now()
            .labels
            .iter()
            .find(|l| l.key == key.0)
            .cloned();
        label.map_or_else(Vec::new, |l| self.tracks_of(&l).into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::LibrarySource;

    #[test]
    fn the_sample_labels_and_their_undo() {
        let library = SampleLibrary::new(300);
        let rows = library.labels().unwrap();
        assert_eq!(rows.len(), INVENTED.len());
        let late = rows.iter().find(|r| r.name == "late night").unwrap();
        assert_eq!((late.artists, late.albums, late.own_tracks), (1, 2, 14));
        let contents = library.label_contents(late.key).unwrap().unwrap();
        assert_eq!(contents.artists.len(), 1);
        assert_eq!(contents.albums.len(), 2);
        assert_eq!(contents.tracks.len(), 14);
        let keys = library.label_track_keys(late.key).unwrap();
        assert_eq!(keys.len(), late.tracks);
        assert!(
            keys.iter()
                .all(|k| library.labels_of_track(*k).contains(&late.name))
        );

        let tracks: Vec<TrackKey> = library.albums[2].tracks.iter().map(|t| t.key).collect();
        let target = LabelFor::Tracks(tracks.iter().take(3).copied().collect());
        let state = |name: &str| {
            library
                .label_choices(&target)
                .unwrap()
                .labels
                .into_iter()
                .find(|l| l.name == name)
                .map(|l| l.state)
        };
        assert_eq!(state("night drive"), Some(LabelState::Some));
        assert_eq!(state("night shift"), Some(LabelState::FromAlbum));
        assert_eq!(state("late night"), Some(LabelState::FromArtist));
        let mut undo = LabelUndo::default();
        for edit in [
            LabelEdit::Give {
                target: target.clone(),
                name: "night drive".into(),
            },
            LabelEdit::Give {
                target: target.clone(),
                name: "fresh".into(),
            },
            LabelEdit::Delete {
                name: "winter".into(),
            },
        ] {
            undo.absorb(library.change_label(&edit).unwrap().undo);
        }
        assert_eq!(state("night drive"), Some(LabelState::All));
        assert_eq!(library.labels().unwrap().len(), INVENTED.len());
        library.undo_labels(&undo).unwrap();
        assert_eq!(library.labels().unwrap(), rows);
        assert!(matches!(
            library.change_label(&LabelEdit::Rename {
                name: "live".into(),
                new_name: "WORK".into(),
            }),
            Err(LabelEditError::Exists(name)) if name == "work"
        ));
    }
}
