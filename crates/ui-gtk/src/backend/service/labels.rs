// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Labels over the running service: the list, how each stands for a selection,
//! changes with their undo, and what a label was given to.

use std::collections::HashMap;

use helikodu_model::query::{Expr, Field, Operator, Query, Value};
use helikodu_service::library::{
    AlbumId, LabelChange, LabelError, LabelSummary, LabelTarget, LabelUndo as Undo, LibraryError,
};

use super::{BackendError, ServiceLibrary, TrackKey, error, lock, track_id};
use crate::backend::{
    ArtistRow, LabelChoice, LabelChoices, LabelContents, LabelDone, LabelEdit, LabelEditError,
    LabelFor, LabelKey, LabelRow, LabelState, LabelUndo,
};

fn label_key(summary: &LabelSummary) -> LabelKey {
    LabelKey(u64::try_from(summary.id.get()).unwrap_or(0))
}

fn label_row(summary: &LabelSummary) -> LabelRow {
    LabelRow {
        key: label_key(summary),
        name: summary.name.clone(),
        tracks: summary.tracks,
        own_tracks: summary.own_tracks,
        albums: summary.albums,
        artists: summary.artists,
    }
}

impl ServiceLibrary {
    /// The artist of `key`, by name, for a label target.
    fn artist_named(&self, key: crate::backend::ArtistKey) -> Result<String, BackendError> {
        let known = lock(&self.artist_rows)
            .iter()
            .find(|a| a.key == key)
            .map(|a| a.name.clone());
        match known {
            Some(name) => Ok(name),
            None => self
                .artist_list(false)?
                .into_iter()
                .find(|a| a.key == key)
                .map(|a| a.name)
                .ok_or_else(|| BackendError(crate::i18n::tr("There is no such artist."))),
        }
    }

    fn label_target(&self, target: &LabelFor) -> Result<LabelTarget, BackendError> {
        Ok(match target {
            LabelFor::Tracks(keys) => {
                LabelTarget::Tracks(keys.iter().map(|key| track_id(*key)).collect())
            }
            LabelFor::Album(album) => {
                LabelTarget::Album(AlbumId::from_raw(i64::try_from(album.0).unwrap_or(-1)))
            }
            LabelFor::Artist(artist) => LabelTarget::Artist(self.artist_named(*artist)?),
        })
    }

    pub(super) fn label_rows(&self) -> Result<Vec<LabelRow>, BackendError> {
        let music = self.library()?;
        Ok(music
            .labels()
            .map_err(error)?
            .iter()
            .map(label_row)
            .collect())
    }

    pub(super) fn choices(&self, target: &LabelFor) -> Result<LabelChoices, BackendError> {
        let music = self.library()?;
        let marks = music
            .label_marks(&self.label_target(target)?)
            .map_err(error)?;
        let labels = music
            .labels()
            .map_err(error)?
            .iter()
            .map(|summary| {
                let mark = marks.labels.iter().find(|m| m.id == summary.id);
                let (own, album, artist) =
                    mark.map_or((0, 0, 0), |m| (m.own, m.from_album, m.from_artist));
                LabelChoice {
                    key: label_key(summary),
                    name: summary.name.clone(),
                    tracks: summary.tracks,
                    state: LabelState::of(marks.items, own, album, artist),
                }
            })
            .collect();
        Ok(LabelChoices {
            items: marks.items,
            labels,
        })
    }

    pub(super) fn edit_labels(&self, edit: &LabelEdit) -> Result<LabelDone, LabelEditError> {
        let music = self.library()?;
        let change = match edit {
            LabelEdit::Give { target, name } => LabelChange::Give {
                target: self.label_target(target)?,
                name: name.clone(),
            },
            LabelEdit::Take { target, name } => LabelChange::Take {
                target: self.label_target(target)?,
                name: name.clone(),
            },
            LabelEdit::Rename { name, new_name } => LabelChange::Rename {
                name: name.clone(),
                new_name: new_name.clone(),
            },
            LabelEdit::Merge { from, into } => LabelChange::Merge {
                from: from.clone(),
                into: into.clone(),
            },
            LabelEdit::Delete { name } => LabelChange::Delete { name: name.clone() },
        };
        match music.change_labels(&change) {
            Ok((count, undo)) => Ok(LabelDone {
                count,
                undo: LabelUndo::step(undo),
            }),
            Err(LibraryError::Label(LabelError::Exists(name))) => Err(LabelEditError::Exists(name)),
            Err(other) => Err(LabelEditError::Failed(error(other))),
        }
    }

    pub(super) fn undo_label_edits(&self, undo: &LabelUndo) -> Result<(), BackendError> {
        let music = self.library()?;
        let mut all = Undo::default();
        for step in undo.steps::<Undo>() {
            all.absorb(step.clone());
        }
        music.undo_labels(&all).map_err(error)
    }

    fn label_named(&self, key: LabelKey) -> Result<Option<LabelSummary>, BackendError> {
        let music = self.library()?;
        Ok(music
            .labels()
            .map_err(error)?
            .into_iter()
            .find(|l| label_key(l) == key))
    }

    pub(super) fn contents(&self, key: LabelKey) -> Result<Option<LabelContents>, BackendError> {
        let music = self.library()?;
        let Some(summary) = self.label_named(key)? else {
            return Ok(None);
        };
        let Some(items) = music.label_items_of(&summary.name).map_err(error)? else {
            return Ok(None);
        };
        let artists = if items.artists.is_empty() {
            Vec::new()
        } else {
            let by_name: HashMap<String, ArtistRow> = self
                .artist_list(false)?
                .into_iter()
                .map(|a| (a.name.to_lowercase(), a))
                .collect();
            items
                .artists
                .iter()
                .filter_map(|name| by_name.get(&name.to_lowercase()).cloned())
                .collect()
        };
        let mut albums = Vec::with_capacity(items.albums.len());
        for album in &items.albums {
            if let Some(detail) = music.album(*album).map_err(error)? {
                self.remember(std::slice::from_ref(&detail.summary));
                albums.push(Self::row(&detail.summary));
            }
        }
        let keys: Vec<TrackKey> = items.tracks.iter().map(|id| TrackKey(id.get())).collect();
        let tracks = crate::backend::LibrarySource::track_rows(self, &keys)?;
        Ok(Some(LabelContents {
            row: label_row(&summary),
            artists,
            albums,
            tracks,
        }))
    }

    pub(super) fn label_tracks(&self, key: LabelKey) -> Result<Vec<TrackKey>, BackendError> {
        let music = self.library()?;
        let Some(summary) = self.label_named(key)? else {
            return Ok(Vec::new());
        };
        let query = Query::new(Expr::compare(
            Field::Label,
            Operator::Equals,
            Value::Text(summary.name),
        ));
        let ids = music.query_track_ids(&query, 0).map_err(error)?;
        Ok(ids.iter().map(|id| TrackKey(id.get())).collect())
    }
}
