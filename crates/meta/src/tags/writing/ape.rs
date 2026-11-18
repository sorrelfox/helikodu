// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing APEv2 items (WavPack, Monkey's Audio, Musepack, APE tags on MP3), the
//! reverse of `mapping/ape.rs`. Several values are joined with NUL characters.

use lofty::ape::{ApeItem, ApeTag};
use lofty::tag::ItemValue;

use crate::tags::field::Field;
use crate::tags::mapping::ape;
use crate::tags::rating::Rating;

use super::changes::{CoverChange, TagField};
use super::editor::{
    ONLY_OPUS, Pair, PairKind, Skip, TagEditor, pair_fields, rating_values, replaced_fields,
};
use super::id3v2::front_picture;
use super::vorbis;

/// The item key Picard writes for `field`: the APEv2 names where they differ,
/// otherwise the Vorbis comment name.
pub(crate) fn key(field: TagField) -> String {
    let key = match field {
        TagField::Title => "Title",
        TagField::Artist => "Artist",
        TagField::Album => "Album",
        TagField::AlbumArtist => "Album Artist",
        TagField::TrackNumber => "Track",
        TagField::DiscNumber => "Disc",
        TagField::Date => "Year",
        TagField::Genre => "Genre",
        TagField::Composer => "Composer",
        TagField::Comment => "Comment",
        TagField::Remixer => "MixArtist",
        TagField::Website => "Weblink",
        TagField::Copyright => "Copyright",
        TagField::Language => "Language",
        _ => return vorbis::key(field),
    };
    key.to_owned()
}

const FRONT_COVER_KEY: &str = "Cover Art (Front)";

/// An APEv2 tag, being edited.
#[derive(Debug)]
pub(crate) struct ApeEditor<'a> {
    tag: &'a mut ApeTag,
}

impl<'a> ApeEditor<'a> {
    pub(crate) fn new(tag: &'a mut ApeTag) -> Self {
        Self { tag }
    }

    fn remove_where(&mut self, remove: impl Fn(&str) -> bool) {
        let keys: Vec<String> = (&*self.tag)
            .into_iter()
            .map(|item| item.key().to_owned())
            .filter(|key| remove(key))
            .collect();
        for key in keys {
            self.tag.remove(&key);
        }
    }

    fn remove_fields(&mut self, fields: &[Field]) {
        self.remove_where(|key| ape::field(key).is_some_and(|f| fields.contains(&f)));
    }

    fn insert(&mut self, key: &str, values: &[String]) -> Result<(), Skip> {
        if values.is_empty() {
            return Ok(());
        }
        let item = ApeItem::new(key.to_owned(), ItemValue::Text(values.join("\0")))
            .map_err(|_| "not a valid APEv2 key")?;
        self.tag.insert(item);
        Ok(())
    }
}

impl TagEditor for ApeEditor<'_> {
    fn set_field(&mut self, field: TagField, values: &[String]) -> Result<(), Skip> {
        if field.is_r128() {
            return Err(ONLY_OPUS);
        }
        self.remove_fields(&replaced_fields(field));
        self.insert(&key(field), values)
    }

    fn set_custom(&mut self, key: &str, values: &[String]) -> Result<(), Skip> {
        self.remove_where(|k| k.eq_ignore_ascii_case(key));
        self.insert(key, values)
    }

    fn set_pair(&mut self, kind: PairKind, pair: Pair) -> Result<(), Skip> {
        self.remove_fields(&pair_fields(kind));
        let (number, total) = kind.fields();
        match pair.joined() {
            Some(joined) => self.insert(&key(number), &[joined]),
            None => {
                let total_value: Vec<String> = pair.total.iter().map(u32::to_string).collect();
                self.insert(&key(total), &total_value)
            }
        }
    }

    fn set_lyrics(&mut self, lyrics: Option<&str>) -> Result<(), Skip> {
        self.remove_fields(&[Field::Lyrics]);
        match lyrics {
            Some(text) => self.insert("Lyrics", &[text.to_owned()]),
            None => Ok(()),
        }
    }

    fn set_rating(&mut self, rating: Option<Rating>) -> Result<(), Skip> {
        self.remove_fields(&[Field::FmpsRating, Field::Rating]);
        let Some(rating) = rating else {
            return Ok(());
        };
        self.insert("FMPS_RATING", &[rating_values::fmps(rating)])?;
        match rating_values::percent(rating) {
            Some(percent) => self.insert("RATING", &[percent]),
            None => Ok(()),
        }
    }

    fn set_cover(&mut self, change: &CoverChange) -> Result<(), Skip> {
        match change {
            CoverChange::RemoveAll => self.remove_where(|k| {
                k.get(..9)
                    .is_some_and(|p| p.eq_ignore_ascii_case("Cover Art"))
            }),
            CoverChange::RemoveFront => self.tag.remove(FRONT_COVER_KEY),
            CoverChange::SetFront(bytes) => {
                let picture = front_picture(bytes);
                let item = ApeItem::new(
                    FRONT_COVER_KEY.to_owned(),
                    ItemValue::Binary(picture.as_ape_bytes()),
                )
                .map_err(|_| "not a valid APEv2 key")?;
                self.tag.insert(item);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_reads_back_as_its_field() {
        for field in TagField::ALL {
            assert_eq!(ape::field(&key(*field)), Some(field.field()), "{field:?}");
        }
    }
}
