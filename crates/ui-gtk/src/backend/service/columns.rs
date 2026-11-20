// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The labels and the user's fields over the running service: the extra columns
//! of the track table, read for the rows it fetches, and the words of the search
//! completion.

use std::collections::HashMap;

use helikodu_model::library::TrackId;
use helikodu_model::query::{Limit, Query};
use helikodu_service::library::FieldDefinition;

use super::{BackendError, ServiceLibrary, TrackKey, error, track_id};
use crate::backend::columns::FEW_VALUES;
use crate::backend::{ExtraColumns, ExtraValues, FieldWords, Vocabulary, YourField};

/// How many tracks the example value of a computed field is looked for in.
const EXAMPLE_TRACKS: u32 = 50;

impl ServiceLibrary {
    pub(super) fn fields_defined(&self) -> Result<Vec<YourField>, BackendError> {
        let music = self.library()?;
        Ok(music
            .user_fields()
            .map_err(error)?
            .into_iter()
            .map(|field| YourField {
                name: field.name,
                tag: match field.definition {
                    FieldDefinition::Tag { key } => Some(key),
                    FieldDefinition::Computed { .. } => None,
                },
            })
            .collect())
    }

    /// The labels and the fields of `columns` for `keys`: one read for the labels
    /// and one per field, for the batch of rows the table fetches.
    pub(super) fn column_values(
        &self,
        keys: &[TrackKey],
        columns: &ExtraColumns,
    ) -> Result<Vec<(TrackKey, ExtraValues)>, BackendError> {
        let music = self.library()?;
        let ids: Vec<TrackId> = keys.iter().map(|key| track_id(*key)).collect();
        let mut values: HashMap<TrackId, ExtraValues> = HashMap::with_capacity(ids.len());
        if columns.labels {
            for (id, labels) in music.track_labels(&ids).map_err(error)? {
                values.entry(id).or_default().labels = labels.join(", ");
            }
        }
        for name in &columns.fields {
            // A field removed meanwhile shows nothing.
            let Ok(found) = music.field_values(name.as_str(), &ids) else {
                continue;
            };
            for (id, value) in found {
                values.entry(id).or_default().fields.push((*name, value));
            }
        }
        Ok(keys
            .iter()
            .filter_map(|key| Some((*key, values.remove(&track_id(*key))?)))
            .collect())
    }

    pub(super) fn completion_words(&self) -> Result<Vocabulary, BackendError> {
        let music = self.library()?;
        let labels = music
            .labels()
            .map_err(error)?
            .into_iter()
            .map(|label| (label.name, label.tracks))
            .collect();
        let mut fields = Vec::new();
        let mut sample: Option<Vec<TrackId>> = None;
        for field in self.fields_defined()? {
            let words = match &field.tag {
                Some(key) => {
                    let values: Vec<String> = music
                        .tag_values(key, FEW_VALUES)
                        .map_err(error)?
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(value, _)| value)
                        .collect();
                    FieldWords {
                        example: values.first().cloned(),
                        values,
                        field,
                    }
                }
                None => {
                    if sample.is_none() {
                        let first = Query {
                            limit: Some(Limit::tracks(EXAMPLE_TRACKS)),
                            ..Query::default()
                        };
                        sample = Some(music.query_track_ids(&first, 0).map_err(error)?);
                    }
                    let example = music
                        .field_values(&field.name, sample.as_deref().unwrap_or_default())
                        .ok()
                        .and_then(|values| {
                            values
                                .into_iter()
                                .map(|(_, value)| value)
                                .find(|value| !value.trim().is_empty())
                        });
                    FieldWords {
                        field,
                        example,
                        values: Vec::new(),
                    }
                }
            };
            fields.push(words);
        }
        Ok(Vocabulary { labels, fields })
    }
}
