// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing Vorbis comments (FLAC, Ogg Vorbis, Opus, Speex), the reverse of
//! `mapping/vorbis.rs`. Several values are repeated comments.

use lofty::ogg::OggPictureStorage;
use lofty::ogg::tag::VorbisComments;
use lofty::picture::PictureType;

use crate::tags::field::Field;
use crate::tags::mapping::vorbis;
use crate::tags::rating::Rating;

use super::changes::{CoverChange, TagField};
use super::editor::{
    ONLY_OPUS, Pair, PairKind, Skip, TagEditor, pair_fields, rating_values, replaced_fields,
};
use super::id3v2::front_picture;

/// The comment name Picard writes for `field`.
pub(crate) fn key(field: TagField) -> String {
    match field {
        TagField::TrackTotal => "TRACKTOTAL".to_owned(),
        TagField::DiscTotal => "DISCTOTAL".to_owned(),
        TagField::MusicBrainzRecordingId => "MUSICBRAINZ_TRACKID".to_owned(),
        TagField::MusicBrainzTrackId => "MUSICBRAINZ_RELEASETRACKID".to_owned(),
        TagField::Movement => "MOVEMENTNAME".to_owned(),
        TagField::MovementNumber => "MOVEMENT".to_owned(),
        _ => field.picard_name().to_ascii_uppercase(),
    }
}

/// The Vorbis comments of a file, being edited.
#[derive(Debug)]
pub(crate) struct VorbisEditor<'a> {
    tag: &'a mut VorbisComments,
    /// Whether the pictures live in the comments (Ogg) rather than in FLAC
    /// `PICTURE` blocks, which the caller handles.
    pictures: bool,
    /// Whether the file is Ogg Opus, the one format whose comments carry the
    /// R128 gains (RFC 7845).
    opus: bool,
}

impl<'a> VorbisEditor<'a> {
    pub(crate) fn new(tag: &'a mut VorbisComments, pictures: bool) -> Self {
        Self {
            tag,
            pictures,
            opus: false,
        }
    }

    /// Marks the comments as those of an Opus file, so that they take R128 gains.
    pub(crate) fn opus(mut self) -> Self {
        self.opus = true;
        self
    }

    /// Removes the comments for which `remove` holds, keeping the order of the rest.
    fn remove_where(&mut self, remove: impl Fn(&str) -> bool) {
        let items: Vec<(String, String)> = self.tag.take_items().collect();
        for (key, value) in items {
            if !remove(&key) {
                self.tag.push(key, value);
            }
        }
    }

    fn remove_fields(&mut self, fields: &[Field]) {
        self.remove_where(|key| vorbis::field(key).is_some_and(|f| fields.contains(&f)));
    }

    fn push(&mut self, key: &str, values: &[String]) {
        for value in values {
            self.tag.push(key.to_owned(), value.clone());
        }
    }
}

impl TagEditor for VorbisEditor<'_> {
    fn set_field(&mut self, field: TagField, values: &[String]) -> Result<(), Skip> {
        if field.is_r128() && !self.opus {
            return Err(ONLY_OPUS);
        }
        self.remove_fields(&replaced_fields(field));
        self.push(&key(field), values);
        Ok(())
    }

    fn set_custom(&mut self, key: &str, values: &[String]) -> Result<(), Skip> {
        self.remove_where(|k| k.eq_ignore_ascii_case(key));
        self.push(&key.to_ascii_uppercase(), values);
        Ok(())
    }

    fn set_pair(&mut self, kind: PairKind, pair: Pair) -> Result<(), Skip> {
        self.remove_fields(&pair_fields(kind));
        let (number, total) = kind.fields();
        let number_value: Vec<String> = pair.number.iter().map(u32::to_string).collect();
        let total_value: Vec<String> = pair.total.iter().map(u32::to_string).collect();
        self.push(&key(number), &number_value);
        self.push(&key(total), &total_value);
        Ok(())
    }

    fn set_lyrics(&mut self, lyrics: Option<&str>) -> Result<(), Skip> {
        self.remove_fields(&[Field::Lyrics]);
        if let Some(text) = lyrics {
            self.tag.push("LYRICS".to_owned(), text.to_owned());
        }
        Ok(())
    }

    fn set_rating(&mut self, rating: Option<Rating>) -> Result<(), Skip> {
        self.remove_fields(&[Field::FmpsRating, Field::Rating]);
        if let Some(rating) = rating {
            self.tag
                .push("FMPS_RATING".to_owned(), rating_values::fmps(rating));
            if let Some(percent) = rating_values::percent(rating) {
                self.tag.push("RATING".to_owned(), percent);
            }
        }
        Ok(())
    }

    fn set_cover(&mut self, change: &CoverChange) -> Result<(), Skip> {
        if self.pictures {
            edit_pictures(self.tag, change)?;
        }
        Ok(())
    }
}

/// Changes the front cover in a picture store: FLAC `PICTURE` blocks, or Vorbis
/// `METADATA_BLOCK_PICTURE` comments.
pub(crate) fn edit_pictures(
    store: &mut impl OggPictureStorage,
    change: &CoverChange,
) -> Result<(), Skip> {
    match change {
        CoverChange::RemoveAll => drop(store.remove_pictures()),
        CoverChange::RemoveFront => store.remove_picture_type(PictureType::CoverFront),
        CoverChange::SetFront(bytes) => {
            store.remove_picture_type(PictureType::CoverFront);
            store
                .insert_picture(front_picture(bytes), None)
                .map_err(|_| "the picture's size cannot be read")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_reads_back_as_its_field() {
        for field in TagField::ALL {
            assert_eq!(
                vorbis::field(&key(*field)),
                Some(field.field()),
                "{field:?}"
            );
        }
    }
}
