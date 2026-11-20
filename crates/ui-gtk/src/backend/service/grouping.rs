// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track table's groups over the running service: the library groups the
//! sorted list by a field, a label, one of the user's fields or an expression.

use helikodu_model::expression::Template;
use helikodu_model::query::Field;
use helikodu_service::library::GroupBy;

use super::{BackendError, ServiceLibrary, TrackKey, error, track_id};
use crate::backend::{TrackGroup, TrackGrouping};

/// What the library groups by for `by`.
fn group_by(by: &TrackGrouping) -> Result<GroupBy, BackendError> {
    Ok(match by {
        TrackGrouping::AlbumArtist => GroupBy::Field(Field::AlbumArtist),
        TrackGrouping::Album => GroupBy::Field(Field::Album),
        TrackGrouping::Genre => GroupBy::Field(Field::Genre),
        TrackGrouping::Year => GroupBy::Field(Field::Year),
        TrackGrouping::Format => GroupBy::Field(Field::Codec),
        TrackGrouping::Label => GroupBy::Field(Field::Label),
        TrackGrouping::Field(name) => GroupBy::Field(
            Field::from_name(name)
                .ok_or_else(|| BackendError(format!("there is no field {name}")))?,
        ),
        TrackGrouping::Expression(text) => {
            GroupBy::Expression(Template::parse(text).map_err(error)?)
        }
    })
}

impl ServiceLibrary {
    pub(super) fn groups(
        &self,
        keys: &[TrackKey],
        by: &TrackGrouping,
    ) -> Result<Vec<TrackGroup>, BackendError> {
        let music = self.library()?;
        let ids: Vec<_> = keys.iter().map(|key| track_id(*key)).collect();
        let groups = music.group_tracks(&group_by(by)?, &ids).map_err(error)?;
        Ok(groups
            .into_iter()
            .map(|group| TrackGroup {
                value: group.value,
                keys: group.tracks.iter().map(|id| TrackKey(id.get())).collect(),
            })
            .collect())
    }

    pub(super) fn field_names(&self) -> Result<Vec<String>, BackendError> {
        Ok(self
            .library()?
            .user_fields()
            .map_err(error)?
            .into_iter()
            .map(|field| field.name)
            .collect())
    }
}
