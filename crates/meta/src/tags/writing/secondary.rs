// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The small tags kept beside a file's main tag: RIFF `INFO` in WAV, the text
//! chunks of AIFF, and ID3v1 on MP3 and other files. They are only edited when
//! the file already has them, and only in the fields they can hold; changes they
//! cannot take are left to the main tag.

use lofty::id3::v1::{GENRES, Id3v1Tag};
use lofty::iff::aiff::AiffTextChunks;
use lofty::iff::wav::RiffInfoList;

use crate::tags::field::Field;
use crate::tags::mapping::riff;
use crate::tags::rating::Rating;

use super::changes::{CoverChange, TagField};
use super::editor::{Pair, PairKind, Skip, TagEditor, pair_fields, replaced_fields};

const NOT_HELD: Skip = "the tag cannot hold it";

/// The `INFO` chunk ID of `field`, the reverse of `mapping/riff.rs`.
fn riff_key(field: TagField) -> Option<&'static str> {
    Some(match field {
        TagField::Title => "INAM",
        TagField::Artist => "IART",
        TagField::Album => "IPRD",
        TagField::Date => "ICRD",
        TagField::Genre => "IGNR",
        TagField::Comment => "ICMT",
        TagField::TrackNumber => "ITRK",
        TagField::TrackTotal => "IFRM",
        TagField::Copyright => "ICOP",
        TagField::Composer => "IMUS",
        TagField::Lyricist => "IWRI",
        TagField::Engineer => "IENG",
        TagField::Producer => "IPRO",
        TagField::Language => "ILNG",
        TagField::Bpm => "IBPM",
        _ => return None,
    })
}

/// A RIFF `INFO` list, being edited.
#[derive(Debug)]
pub(crate) struct RiffEditor<'a> {
    tag: &'a mut RiffInfoList,
}

impl<'a> RiffEditor<'a> {
    pub(crate) fn new(tag: &'a mut RiffInfoList) -> Self {
        Self { tag }
    }

    fn remove_fields(&mut self, fields: &[Field]) {
        let keys: Vec<String> = (&*self.tag)
            .into_iter()
            .map(|(key, _)| key.to_owned())
            .filter(|key| riff::field(key).is_some_and(|f| fields.contains(&f)))
            .collect();
        for key in keys {
            while self.tag.remove(&key).is_some() {}
        }
    }

    fn insert(&mut self, key: &str, value: Option<String>) {
        if let Some(value) = value {
            self.tag.insert(key.to_owned(), value);
        }
    }
}

impl TagEditor for RiffEditor<'_> {
    fn set_field(&mut self, field: TagField, values: &[String]) -> Result<(), Skip> {
        let key = riff_key(field).ok_or(NOT_HELD)?;
        self.remove_fields(&replaced_fields(field));
        self.insert(key, (!values.is_empty()).then(|| values.join("; ")));
        Ok(())
    }

    fn set_custom(&mut self, _key: &str, _values: &[String]) -> Result<(), Skip> {
        Err(NOT_HELD)
    }

    fn set_pair(&mut self, kind: PairKind, pair: Pair) -> Result<(), Skip> {
        if kind != PairKind::Track {
            return Err(NOT_HELD);
        }
        self.remove_fields(&pair_fields(kind));
        self.insert("ITRK", pair.number.map(|n| n.to_string()));
        self.insert("IFRM", pair.total.map(|n| n.to_string()));
        Ok(())
    }

    fn set_lyrics(&mut self, _lyrics: Option<&str>) -> Result<(), Skip> {
        Err(NOT_HELD)
    }

    fn set_rating(&mut self, _rating: Option<Rating>) -> Result<(), Skip> {
        Err(NOT_HELD)
    }

    fn set_cover(&mut self, _change: &CoverChange) -> Result<(), Skip> {
        Err(NOT_HELD)
    }
}

/// The text chunks of an AIFF file, being edited: the name, the author, the
/// copyright and the comments.
#[derive(Debug)]
pub(crate) struct AiffEditor<'a> {
    tag: &'a mut AiffTextChunks,
}

impl<'a> AiffEditor<'a> {
    pub(crate) fn new(tag: &'a mut AiffTextChunks) -> Self {
        Self { tag }
    }
}

impl TagEditor for AiffEditor<'_> {
    fn set_field(&mut self, field: TagField, values: &[String]) -> Result<(), Skip> {
        let value = (!values.is_empty()).then(|| values.join("; "));
        match field {
            TagField::Title => self.tag.name = value,
            TagField::Artist => self.tag.author = value,
            TagField::Copyright => self.tag.copyright = value,
            // The comment moves to the main tag, which can hold it fully.
            TagField::Comment => {
                self.tag.annotations = None;
                self.tag.comments = None;
                return Err(NOT_HELD);
            }
            _ => return Err(NOT_HELD),
        }
        Ok(())
    }

    fn set_custom(&mut self, _key: &str, _values: &[String]) -> Result<(), Skip> {
        Err(NOT_HELD)
    }

    fn set_pair(&mut self, _kind: PairKind, _pair: Pair) -> Result<(), Skip> {
        Err(NOT_HELD)
    }

    fn set_lyrics(&mut self, _lyrics: Option<&str>) -> Result<(), Skip> {
        Err(NOT_HELD)
    }

    fn set_rating(&mut self, _rating: Option<Rating>) -> Result<(), Skip> {
        Err(NOT_HELD)
    }

    fn set_cover(&mut self, _change: &CoverChange) -> Result<(), Skip> {
        Err(NOT_HELD)
    }
}

/// An ID3v1 tag, being edited: title, artist, album, year, comment, track and
/// genre (when it is one of the numbered genres).
#[derive(Debug)]
pub(crate) struct Id3v1Editor<'a> {
    tag: &'a mut Id3v1Tag,
}

impl<'a> Id3v1Editor<'a> {
    pub(crate) fn new(tag: &'a mut Id3v1Tag) -> Self {
        Self { tag }
    }
}

impl TagEditor for Id3v1Editor<'_> {
    fn set_field(&mut self, field: TagField, values: &[String]) -> Result<(), Skip> {
        let value = (!values.is_empty()).then(|| values.join("; "));
        match field {
            TagField::Title => self.tag.title = value,
            TagField::Artist => self.tag.artist = value,
            TagField::Album => self.tag.album = value,
            TagField::Comment => self.tag.comment = value,
            TagField::Date => {
                self.tag.year = value.and_then(|v| v.get(..4).and_then(|y| y.parse().ok()));
            }
            TagField::Genre => {
                self.tag.genre = values.first().and_then(|genre| {
                    GENRES
                        .iter()
                        .position(|g| g.eq_ignore_ascii_case(genre))
                        .and_then(|i| u8::try_from(i).ok())
                });
            }
            _ => return Err(NOT_HELD),
        }
        Ok(())
    }

    fn set_custom(&mut self, _key: &str, _values: &[String]) -> Result<(), Skip> {
        Err(NOT_HELD)
    }

    fn set_pair(&mut self, kind: PairKind, pair: Pair) -> Result<(), Skip> {
        if kind != PairKind::Track {
            return Err(NOT_HELD);
        }
        self.tag.track_number = pair.number.and_then(|n| u8::try_from(n).ok());
        Ok(())
    }

    fn set_lyrics(&mut self, _lyrics: Option<&str>) -> Result<(), Skip> {
        Err(NOT_HELD)
    }

    fn set_rating(&mut self, _rating: Option<Rating>) -> Result<(), Skip> {
        Err(NOT_HELD)
    }

    fn set_cover(&mut self, _change: &CoverChange) -> Result<(), Skip> {
        Err(NOT_HELD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_riff_key_reads_back_as_its_field() {
        for field in TagField::ALL {
            if let Some(key) = riff_key(*field) {
                assert_eq!(riff::field(key), Some(field.field()), "{field:?}");
            }
        }
    }
}
