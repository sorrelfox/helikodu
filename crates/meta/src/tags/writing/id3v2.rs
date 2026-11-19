// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing ID3v2 frames, the reverse of `mapping/id3v2.rs`.
//!
//! Text values go to UTF-8 frames (UTF-16 in ID3v2.3); several values are joined
//! with NUL characters (genres with `; ` in ID3v2.3). Dates go to timestamp frames. In ID3v2.3, which has no
//! `TSOT`, `TSOP`, `TSOA`, `TSST`, `TMOO`, `TDRL` or `TMCL`, those fields go to
//! `TXXX` frames under their Vorbis comment names, which read back the same.

use std::borrow::Cow;
use std::str::FromStr;

use lofty::TextEncoding;
use lofty::id3::v2::{
    CommentFrame, ExtendedTextFrame, Frame, FrameId, Id3v2Tag, KeyValueFrame, PopularimeterFrame,
    TextInformationFrame, TimestampFrame, UniqueFileIdentifierFrame, UnsynchronizedTextFrame,
};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::items::{Lang, Timestamp, UNKNOWN_LANGUAGE};

use crate::covers::PictureFormat;
use crate::tags::field::Field;
use crate::tags::mapping::id3v2;
use crate::tags::rating::Rating;

use super::changes::{CoverChange, TagField};
use super::editor::{
    ONLY_OPUS, Pair, PairKind, Skip, TagEditor, pair_fields, rating_values, replaced_fields,
};

const MUSICBRAINZ_UFID_OWNER: &str = "http://musicbrainz.org";

/// Where a field is written in ID3v2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// A text frame.
    Text(&'static str),
    /// A timestamp frame (`TDRC`, `TDOR`, `TDRL`).
    Timestamp(&'static str),
    /// A `TXXX` frame with this description.
    UserText(&'static str),
    /// A `COMM` frame without a description.
    Comment,
    /// The MusicBrainz `UFID` frame.
    Ufid,
    /// A role in the `TIPL` frame.
    Involved(&'static str),
    /// The `TMCL` frame (performers with an instrument; the others go to
    /// `TXXX:PERFORMER`).
    Musicians,
}

/// Where `field` goes, as Picard writes it.
fn target(field: TagField, v23: bool, has_grp1: bool) -> Target {
    use Target::{Comment, Involved, Musicians, Text, Timestamp, Ufid, UserText};
    match field {
        TagField::Title => Text("TIT2"),
        TagField::TitleSort if v23 => UserText("TITLESORT"),
        TagField::TitleSort => Text("TSOT"),
        TagField::Subtitle => Text("TIT3"),
        TagField::Artist => Text("TPE1"),
        TagField::Artists => UserText("ARTISTS"),
        TagField::ArtistSort if v23 => UserText("ARTISTSORT"),
        TagField::ArtistSort => Text("TSOP"),
        TagField::Album => Text("TALB"),
        TagField::AlbumSort if v23 => UserText("ALBUMSORT"),
        TagField::AlbumSort => Text("TSOA"),
        TagField::AlbumArtist => Text("TPE2"),
        TagField::AlbumArtists => UserText("ALBUMARTISTS"),
        TagField::AlbumArtistSort => Text("TSO2"),
        TagField::TrackNumber => Text("TRCK"),
        TagField::TrackTotal => UserText("TOTALTRACKS"),
        TagField::DiscNumber => Text("TPOS"),
        TagField::DiscTotal => UserText("TOTALDISCS"),
        TagField::DiscSubtitle if v23 => UserText("DISCSUBTITLE"),
        TagField::DiscSubtitle => Text("TSST"),
        TagField::Date => Timestamp("TDRC"),
        TagField::OriginalDate => Timestamp("TDOR"),
        TagField::ReleaseDate if v23 => UserText("RELEASEDATE"),
        TagField::ReleaseDate => Timestamp("TDRL"),
        TagField::Genre => Text("TCON"),
        TagField::Composer => Text("TCOM"),
        TagField::ComposerSort => Text("TSOC"),
        TagField::Conductor => Text("TPE3"),
        TagField::Lyricist => Text("TEXT"),
        TagField::Writer => UserText("WRITER"),
        TagField::Arranger => Involved("arranger"),
        TagField::Remixer => Text("TPE4"),
        TagField::Producer => Involved("producer"),
        TagField::Engineer => Involved("engineer"),
        TagField::Mixer => Involved("mix"),
        TagField::DjMixer => Involved("DJ-mix"),
        TagField::Performer => Musicians,
        TagField::Label => Text("TPUB"),
        TagField::CatalogNumber => UserText("CATALOGNUMBER"),
        TagField::Barcode => UserText("BARCODE"),
        TagField::Isrc => Text("TSRC"),
        TagField::Asin => UserText("ASIN"),
        TagField::ReleaseType => UserText("MusicBrainz Album Type"),
        TagField::ReleaseStatus => UserText("MusicBrainz Album Status"),
        TagField::ReleaseCountry => UserText("MusicBrainz Album Release Country"),
        TagField::Media => Text("TMED"),
        TagField::Script => UserText("SCRIPT"),
        TagField::Language => Text("TLAN"),
        TagField::MusicBrainzRecordingId => Ufid,
        TagField::MusicBrainzTrackId => UserText("MusicBrainz Release Track Id"),
        TagField::MusicBrainzAlbumId => UserText("MusicBrainz Album Id"),
        TagField::MusicBrainzArtistId => UserText("MusicBrainz Artist Id"),
        TagField::MusicBrainzAlbumArtistId => UserText("MusicBrainz Album Artist Id"),
        TagField::MusicBrainzReleaseGroupId => UserText("MusicBrainz Release Group Id"),
        TagField::MusicBrainzWorkId => UserText("MusicBrainz Work Id"),
        TagField::MusicBrainzDiscId => UserText("MusicBrainz Disc Id"),
        TagField::AcoustidId => UserText("Acoustid Id"),
        TagField::Comment => Comment,
        TagField::Compilation => Text("TCMP"),
        // iTunes keeps the grouping in GRP1 and the work in TIT1; Picard writes the
        // grouping to TIT1 unless the file already follows iTunes.
        TagField::Grouping if has_grp1 => Text("GRP1"),
        TagField::Grouping => Text("TIT1"),
        TagField::Work if has_grp1 => Text("TIT1"),
        TagField::Work => UserText("WORK"),
        TagField::Movement => Text("MVNM"),
        TagField::MovementNumber => Text("MVIN"),
        TagField::MovementTotal => UserText("MOVEMENTTOTAL"),
        TagField::ShowMovement => UserText("SHOWMOVEMENT"),
        TagField::Bpm => Text("TBPM"),
        TagField::Key => Text("TKEY"),
        TagField::Mood if v23 => UserText("MOOD"),
        TagField::Mood => Text("TMOO"),
        TagField::EncodedBy => Text("TENC"),
        TagField::EncoderSettings => Text("TSSE"),
        TagField::Copyright => Text("TCOP"),
        TagField::License => UserText("LICENSE"),
        TagField::Website => UserText("WEBSITE"),
        TagField::Director => UserText("DIRECTOR"),
        TagField::OriginalAlbum => Text("TOAL"),
        TagField::OriginalArtist => Text("TOPE"),
        TagField::ReplayGainTrackGain => UserText("replaygain_track_gain"),
        TagField::ReplayGainTrackPeak => UserText("replaygain_track_peak"),
        TagField::ReplayGainAlbumGain => UserText("replaygain_album_gain"),
        TagField::ReplayGainAlbumPeak => UserText("replaygain_album_peak"),
        // Refused before this is asked; R128 gains belong to Opus.
        TagField::R128TrackGain => UserText("R128_TRACK_GAIN"),
        TagField::R128AlbumGain => UserText("R128_ALBUM_GAIN"),
    }
}

fn frame_id(id: &'static str) -> FrameId<'static> {
    FrameId::Valid(Cow::Borrowed(id))
}

/// The table field an existing frame is read as, as the reader decides it.
fn frame_field(frame: &Frame<'_>, has_grp1: bool) -> Option<Field> {
    match frame {
        Frame::Text(f) => match f.id().as_str() {
            "TIT1" if has_grp1 => Some(Field::Work),
            "TIT1" => Some(Field::Grouping),
            id => id3v2::frame_field(id),
        },
        Frame::Timestamp(f) => id3v2::frame_field(f.id().as_str()),
        Frame::UserText(f) => id3v2::user_text_field(f.description.trim()),
        Frame::Comment(f) => {
            let description = f.description.trim();
            (description.is_empty() || description == "ID3v1 Comment").then_some(Field::Comment)
        }
        Frame::UniqueFileIdentifier(f) => {
            (f.owner == MUSICBRAINZ_UFID_OWNER).then_some(Field::MbRecordingId)
        }
        _ => None,
    }
}

/// The ID3v2 tag of a file, being edited.
#[derive(Debug)]
pub(crate) struct Id3Editor<'a> {
    tag: &'a mut Id3v2Tag,
    v23: bool,
    has_grp1: bool,
}

impl<'a> Id3Editor<'a> {
    /// Edits `tag`, which is written as ID3v2.3 when `v23` is set.
    pub(crate) fn new(tag: &'a mut Id3v2Tag, v23: bool) -> Self {
        let has_grp1 = (&*tag).into_iter().any(|f| f.id_str() == "GRP1");
        Self { tag, v23, has_grp1 }
    }

    /// Removes the frames read as one of `fields`, and those of the `TIPL` and
    /// `TMCL` frames.
    fn remove_fields(&mut self, fields: &[Field]) {
        let has_grp1 = self.has_grp1;
        self.tag
            .retain(|frame| frame_field(frame, has_grp1).is_none_or(|f| !fields.contains(&f)));
        if fields.contains(&Field::Performer) {
            self.tag.retain(|frame| frame.id_str() != "TMCL");
        }
        let mut people = Vec::new();
        self.tag.retain(|frame| match frame {
            Frame::KeyValue(f) if f.id().as_str() == "TIPL" => {
                people = f.key_value_pairs.iter().cloned().map(owned_pair).collect();
                false
            }
            _ => true,
        });
        people.retain(|(role, _)| {
            id3v2::involvement_field(role).is_none_or(|f| !fields.contains(&f))
        });
        if !people.is_empty() {
            self.push_people(people);
        }
    }

    fn push_people(&mut self, people: Vec<(Cow<'static, str>, Cow<'static, str>)>) {
        self.tag.insert(Frame::KeyValue(KeyValueFrame::new(
            frame_id("TIPL"),
            TextEncoding::UTF8,
            people,
        )));
    }

    fn push_text(&mut self, id: &'static str, values: &[String]) {
        // The tag library writes several ID3v2.3 genres as `(A)B`, which other
        // readers take for a genre reference; a `;` list reads back everywhere.
        let separator = if self.v23 && id == "TCON" { "; " } else { "\0" };
        self.tag.insert(Frame::Text(TextInformationFrame::new(
            frame_id(id),
            TextEncoding::UTF8,
            values.join(separator),
        )));
    }

    fn push_user_text(&mut self, description: &str, values: &[String]) {
        self.tag.insert(Frame::UserText(ExtendedTextFrame::new(
            TextEncoding::UTF8,
            description.to_owned(),
            values.join("\0"),
        )));
    }

    /// The language of the first frame with `id`, to keep it on a rewritten frame.
    fn language_of(&self, id: &str) -> Lang {
        (&*self.tag)
            .into_iter()
            .find_map(|frame| match frame {
                Frame::Comment(f) if id == "COMM" => Some(f.language),
                Frame::UnsynchronizedText(f) if id == "USLT" => Some(f.language),
                _ => None,
            })
            .unwrap_or(UNKNOWN_LANGUAGE)
    }

    fn write(&mut self, target: Target, values: &[String]) {
        match target {
            Target::Text(id) => self.push_text(id, values),
            Target::UserText(description) => self.push_user_text(description, values),
            Target::Timestamp(id) => {
                let value = &values[0];
                match Timestamp::from_str(value) {
                    Ok(timestamp) => self.tag.insert(Frame::Timestamp(TimestampFrame::new(
                        frame_id(id),
                        TextEncoding::UTF8,
                        timestamp,
                    ))),
                    Err(_) => self.tag.insert(Frame::Text(TextInformationFrame::new(
                        frame_id(id),
                        TextEncoding::UTF8,
                        value.clone(),
                    ))),
                };
            }
            Target::Comment => {
                let language = self.language_of("COMM");
                self.tag.insert(Frame::Comment(CommentFrame::new(
                    TextEncoding::UTF8,
                    language,
                    String::new(),
                    values.join("\0"),
                )));
            }
            Target::Ufid => {
                self.tag
                    .insert(Frame::UniqueFileIdentifier(UniqueFileIdentifierFrame::new(
                        MUSICBRAINZ_UFID_OWNER,
                        values[0].as_bytes().to_vec(),
                    )));
            }
            Target::Involved(role) => {
                let mut people: Vec<_> = (&*self.tag)
                    .into_iter()
                    .find_map(|frame| match frame {
                        Frame::KeyValue(f) if f.id().as_str() == "TIPL" => {
                            Some(f.key_value_pairs.iter().cloned().map(owned_pair).collect())
                        }
                        _ => None,
                    })
                    .unwrap_or_default();
                people.extend(
                    values
                        .iter()
                        .map(|name| (Cow::Borrowed(role), Cow::Owned(name.clone()))),
                );
                self.tag.retain(|frame| frame.id_str() != "TIPL");
                self.push_people(people);
            }
            Target::Musicians => {
                let mut musicians = Vec::new();
                let mut others = Vec::new();
                for value in values {
                    match split_performer(value) {
                        Some((name, instrument)) if !self.v23 => musicians.push((
                            Cow::Owned(instrument.to_owned()),
                            Cow::Owned(name.to_owned()),
                        )),
                        _ => others.push(value.clone()),
                    }
                }
                if !musicians.is_empty() {
                    self.tag.insert(Frame::KeyValue(KeyValueFrame::new(
                        frame_id("TMCL"),
                        TextEncoding::UTF8,
                        musicians,
                    )));
                }
                if !others.is_empty() {
                    self.push_user_text("PERFORMER", &others);
                }
            }
        }
    }
}

fn owned_pair((a, b): (Cow<'_, str>, Cow<'_, str>)) -> (Cow<'static, str>, Cow<'static, str>) {
    (Cow::Owned(a.into_owned()), Cow::Owned(b.into_owned()))
}

/// `Name (instrument)` split, as the reader splits a performer.
fn split_performer(text: &str) -> Option<(&str, &str)> {
    let inner = text.strip_suffix(')')?;
    let open = inner.rfind(" (")?;
    let (name, instrument) = (&inner[..open], &inner[open + 2..]);
    (!name.trim().is_empty() && !instrument.trim().is_empty()).then_some((name, instrument))
}

impl TagEditor for Id3Editor<'_> {
    fn set_field(&mut self, field: TagField, values: &[String]) -> Result<(), Skip> {
        if field.is_r128() {
            return Err(ONLY_OPUS);
        }
        self.remove_fields(&replaced_fields(field));
        if field == TagField::Date {
            // ID3v2.3 date parts that the tag library does not fold into TDRC.
            self.tag
                .retain(|f| !matches!(f.id_str(), "TDAT" | "TIME" | "TRDA"));
        }
        if !values.is_empty() {
            self.write(target(field, self.v23, self.has_grp1), values);
        }
        Ok(())
    }

    fn set_custom(&mut self, key: &str, values: &[String]) -> Result<(), Skip> {
        self.tag.retain(|frame| match frame {
            Frame::UserText(f) => !f.description.trim().eq_ignore_ascii_case(key),
            _ => true,
        });
        if !values.is_empty() {
            self.push_user_text(key, values);
        }
        Ok(())
    }

    fn set_pair(&mut self, kind: PairKind, pair: Pair) -> Result<(), Skip> {
        self.remove_fields(&pair_fields(kind));
        let (number, total) = kind.fields();
        match pair.joined() {
            Some(joined) => self.write(target(number, self.v23, self.has_grp1), &[joined]),
            None => {
                if let Some(total_value) = pair.total {
                    let target = target(total, self.v23, self.has_grp1);
                    self.write(target, &[total_value.to_string()]);
                }
            }
        }
        Ok(())
    }

    fn set_lyrics(&mut self, lyrics: Option<&str>) -> Result<(), Skip> {
        let language = self.language_of("USLT");
        self.tag.retain(|frame| match frame {
            Frame::UnsynchronizedText(_) => false,
            // Removing the lyrics removes the synchronized ones too.
            _ => lyrics.is_some() || frame.id_str() != "SYLT",
        });
        if let Some(text) = lyrics {
            self.tag
                .insert(Frame::UnsynchronizedText(UnsynchronizedTextFrame::new(
                    TextEncoding::UTF8,
                    language,
                    String::new(),
                    text.to_owned(),
                )));
        }
        Ok(())
    }

    fn set_rating(&mut self, rating: Option<Rating>) -> Result<(), Skip> {
        let mut counter = 0;
        self.tag.retain(|frame| match frame {
            Frame::Popularimeter(f) if f.email.eq_ignore_ascii_case(rating_values::POPM_EMAIL) => {
                counter = f.counter;
                false
            }
            Frame::UserText(f) => !matches!(
                id3v2::user_text_field(f.description.trim()),
                Some(Field::FmpsRating | Field::Rating)
            ),
            _ => true,
        });
        let value = rating.and_then(rating_values::popm);
        // A frame without a rating (0) still carries the play count.
        if value.is_some() || counter > 0 {
            self.tag
                .insert(Frame::Popularimeter(PopularimeterFrame::new(
                    rating_values::POPM_EMAIL,
                    value.unwrap_or(0),
                    counter,
                )));
        }
        if let Some(rating) = rating {
            self.push_user_text("FMPS_Rating", &[rating_values::fmps(rating)]);
        }
        Ok(())
    }

    fn set_play_count(&mut self, count: Option<u64>) -> Result<(), Skip> {
        let mut value = 0;
        self.tag.retain(|frame| match frame {
            Frame::Popularimeter(f) if f.email.eq_ignore_ascii_case(rating_values::POPM_EMAIL) => {
                value = f.rating;
                false
            }
            _ => true,
        });
        let counter = count.unwrap_or(0);
        if value > 0 || counter > 0 {
            self.tag
                .insert(Frame::Popularimeter(PopularimeterFrame::new(
                    rating_values::POPM_EMAIL,
                    value,
                    counter,
                )));
        }
        self.set_custom(
            rating_values::FMPS_PLAYCOUNT,
            &rating_values::play_count(count),
        )
    }

    fn set_cover(&mut self, change: &CoverChange) -> Result<(), Skip> {
        match change {
            CoverChange::RemoveAll => self.tag.retain(|f| !matches!(f, Frame::Picture(_))),
            CoverChange::RemoveFront => self.tag.remove_picture_type(PictureType::CoverFront),
            CoverChange::SetFront(bytes) => {
                self.tag.remove_picture_type(PictureType::CoverFront);
                self.tag.insert_picture(front_picture(bytes));
            }
        }
        Ok(())
    }
}

/// A front cover picture of `bytes` (checked to be JPEG or PNG).
pub(crate) fn front_picture(bytes: &[u8]) -> Picture {
    let mime = match PictureFormat::sniff(bytes) {
        Some(PictureFormat::Png) => MimeType::Png,
        _ => MimeType::Jpeg,
    };
    Picture::unchecked(bytes.to_vec())
        .pic_type(PictureType::CoverFront)
        .mime_type(mime)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_target_reads_back_as_its_field() {
        for v23 in [false, true] {
            for has_grp1 in [false, true] {
                for field in TagField::ALL {
                    let read = match target(*field, v23, has_grp1) {
                        Target::Text("TIT1") => Some(if has_grp1 {
                            Field::Work
                        } else {
                            Field::Grouping
                        }),
                        Target::Text(id) | Target::Timestamp(id) => id3v2::frame_field(id),
                        Target::UserText(d) => id3v2::user_text_field(d),
                        Target::Involved(role) => id3v2::involvement_field(role),
                        Target::Comment | Target::Ufid | Target::Musicians => continue,
                    };
                    assert_eq!(read, Some(field.field()), "{field:?} v23={v23}");
                }
            }
        }
    }

    #[test]
    fn performers_split_like_the_reader() {
        assert_eq!(split_performer("Jaco (bass)"), Some(("Jaco", "bass")));
        assert_eq!(split_performer("Prince"), None);
    }
}
