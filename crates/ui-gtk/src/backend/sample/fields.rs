// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Your fields over the invented library: `decade`, `era` and `mood` to start
//! with, kept in memory and put in use for the whole program as the library
//! does it. Expressions are worked out from the sample tracks' rows; the tag
//! `MOOD` is invented from each track's key.

use std::sync::{Arc, Mutex, PoisonError};

use helikodu_model::expression::FieldMap;
use helikodu_model::query::Field;
use helikodu_model::user_fields::{self as model_fields, UserFields};

use super::super::fields::{FieldServices, Template, UserField};
use super::super::{BackendError, LibrarySource, TrackKey, TrackRow, Update};
use super::SampleLibrary;
use crate::i18n::{fill, tr};

/// The moods the sample tracks have; an empty one means none.
const MOODS: &[&str] = &["calm", "calm", "solemn", "", "bright", "late", "restless"];

/// The fields the sample library starts with, as the design draws them.
fn starting_fields() -> Vec<UserField> {
    vec![
        UserField::computed("decade", "{left(year,3)}0s"),
        UserField::computed("era", "{decade or \"unknown\"}"),
        UserField::tag("mood", "MOOD"),
    ]
}

/// The sample library's fields.
#[derive(Debug)]
pub(in crate::backend) struct SampleFields {
    library: Option<Arc<SampleLibrary>>,
    updates: Option<async_channel::Sender<Update>>,
    fields: Mutex<Vec<UserField>>,
}

impl SampleFields {
    /// The starting fields over `library`, put in use.
    pub(in crate::backend) fn new(
        library: Option<Arc<SampleLibrary>>,
        updates: Option<async_channel::Sender<Update>>,
    ) -> Self {
        let fields = if library.is_some() {
            starting_fields()
        } else {
            Vec::new()
        };
        if library.is_some() {
            let (set, _) = UserFields::build_lenient(&fields);
            model_fields::install(set);
        }
        Self {
            library,
            updates,
            fields: Mutex::new(fields),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<UserField>> {
        self.fields.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Puts `fields` in use if they fit together, and keeps them.
    fn put(&self, fields: Vec<UserField>) -> Result<UserFields, BackendError> {
        let set = UserFields::build(&fields).map_err(|e| BackendError(e.to_string()))?;
        *self.lock() = set.definitions().to_vec();
        model_fields::install(set.clone());
        if let Some(updates) = &self.updates {
            let _ = updates.try_send(Update::LibraryChanged);
        }
        Ok(set)
    }
}

/// The values of a sample track, for expressions.
fn values(row: &TrackRow) -> FieldMap {
    let mut map = FieldMap::default()
        .with(Field::Title, row.title.clone())
        .with(Field::Artist, row.artist.clone())
        .with(Field::AlbumArtist, row.artist.clone())
        .with(Field::Album, row.album_title.clone())
        .with(Field::Plays, row.plays.to_string());
    map.set_list(
        Field::Genre,
        row.genre.split(", ").map(str::to_owned).collect(),
    );
    if let Some(year) = row.year {
        map.set(Field::Year, year.to_string());
    }
    if let Some(number) = row.number {
        map.set(Field::Track, number.to_string());
    }
    if let Some(disc) = row.disc {
        map.set(Field::Disc, disc.to_string());
    }
    if let Some(rating) = row.rating {
        map.set(Field::Rating, rating.to_string());
    }
    if let Some(length) = row.duration {
        map.set(Field::Duration, length.as_secs().to_string());
    }
    if let Some(codec) = &row.format.codec {
        map.set(Field::Codec, codec.to_lowercase());
    }
    if let Some(path) = &row.path {
        map.set(Field::Path, path.display().to_string());
    }
    let mood = MOODS[usize::try_from(row.key.0).unwrap_or(0) % MOODS.len()];
    map.set(Field::Tag("MOOD".into()), mood);
    map
}

impl FieldServices for SampleFields {
    fn user_fields(&self) -> Result<Vec<UserField>, BackendError> {
        Ok(self.lock().clone())
    }

    fn save_field(&self, old: Option<&str>, field: UserField) -> Result<UserField, BackendError> {
        let mut fields = self.lock().clone();
        match old.and_then(|old| fields.iter().position(|f| f.name.eq_ignore_ascii_case(old))) {
            Some(at) => fields[at] = field.clone(),
            None => fields.push(field.clone()),
        }
        let set = self.put(fields)?;
        set.get(&field.name)
            .cloned()
            .ok_or_else(|| BackendError(fill(&tr("There is no field called {}."), &[&field.name])))
    }

    fn remove_field(&self, name: &str) -> Result<(), BackendError> {
        let mut fields = self.lock().clone();
        fields.retain(|f| !f.name.eq_ignore_ascii_case(name));
        self.put(fields).map(|_| ())
    }

    fn evaluate(
        &self,
        template: &Template,
        tracks: &[TrackKey],
    ) -> Result<Vec<(TrackKey, String)>, BackendError> {
        let Some(library) = &self.library else {
            return Ok(Vec::new());
        };
        Ok(library
            .track_rows(tracks)?
            .iter()
            .map(|row| (row.key, template.evaluate(&values(row))))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_fields_give_values_and_follow_saves() {
        let library = Arc::new(SampleLibrary::new(4));
        let fields = SampleFields::new(Some(library.clone()), None);
        let names: Vec<String> = fields
            .user_fields()
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["decade", "era", "mood"]);
        let row = library.track_rows(&[TrackKey(1)]).unwrap().remove(0);
        let decade = Template::parse("{decade}").unwrap();
        let values = fields.evaluate(&decade, &[TrackKey(1)]).unwrap();
        let year = row.year.unwrap().to_string();
        assert_eq!(values[0].1, format!("{}0s", &year[..3]));
        fields
            .save_field(Some("mood"), UserField::tag("feel", "FEEL"))
            .unwrap();
        let names: Vec<String> = fields
            .user_fields()
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["decade", "era", "feel"]);
        assert!(fields.remove_field("decade").is_err());
        fields.remove_field("era").unwrap();
        fields.remove_field("decade").unwrap();
    }
}
