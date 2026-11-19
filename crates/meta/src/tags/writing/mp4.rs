// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing MP4 `ilst` atoms, the reverse of `mapping/mp4.rs`: the iTunes atoms
//! where they exist, and freeform `----:com.apple.iTunes:NAME` atoms with Picard's
//! names otherwise. Several values are several data atoms.

use std::borrow::Cow;

use lofty::mp4::{Atom, AtomData, AtomIdent, DataType, Ilst};
use lofty::tag::Accessor;

use crate::tags::field::Field;
use crate::tags::mapping::mp4;
use crate::tags::rating::Rating;

use super::changes::{CoverChange, TagField};
use super::editor::{
    ONLY_OPUS, Pair, PairKind, Skip, TagEditor, pair_fields, rating_values, replaced_fields,
};
use super::id3v2::front_picture;
use super::vorbis;

const ITUNES_MEAN: &str = "com.apple.iTunes";

/// Where a field is written in MP4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// A text atom.
    Text(&'static [u8; 4]),
    /// A 16-bit integer atom (`tmpo`, `©mvi`, `©mvc`).
    Integer(&'static [u8; 4]),
    /// A one-byte flag atom (`cpil`, `shwm`).
    Flag(&'static [u8; 4]),
    /// The track or disc number pair (`trkn`, `disk`).
    Pair,
    /// A freeform atom with this name.
    Freeform(&'static str),
    /// A freeform atom with the Vorbis comment name.
    VorbisName,
}

fn target(field: TagField) -> Target {
    use Target::{Flag, Freeform, Integer, Text, VorbisName};
    match field {
        TagField::Title => Text(b"\xa9nam"),
        TagField::TitleSort => Text(b"sonm"),
        TagField::Artist => Text(b"\xa9ART"),
        TagField::ArtistSort => Text(b"soar"),
        TagField::Album => Text(b"\xa9alb"),
        TagField::AlbumSort => Text(b"soal"),
        TagField::AlbumArtist => Text(b"aART"),
        TagField::AlbumArtistSort => Text(b"soaa"),
        TagField::Date => Text(b"\xa9day"),
        TagField::Genre => Text(b"\xa9gen"),
        TagField::Composer => Text(b"\xa9wrt"),
        TagField::ComposerSort => Text(b"soco"),
        TagField::Comment => Text(b"\xa9cmt"),
        TagField::Grouping => Text(b"\xa9grp"),
        TagField::Work => Text(b"\xa9wrk"),
        TagField::Movement => Text(b"\xa9mvn"),
        TagField::MovementNumber => Integer(b"\xa9mvi"),
        TagField::MovementTotal => Integer(b"\xa9mvc"),
        TagField::ShowMovement => Flag(b"shwm"),
        TagField::Bpm => Integer(b"tmpo"),
        TagField::Compilation => Flag(b"cpil"),
        TagField::EncodedBy => Text(b"\xa9too"),
        TagField::Copyright => Text(b"cprt"),
        TagField::Director => Text(b"\xa9dir"),
        TagField::TrackNumber
        | TagField::TrackTotal
        | TagField::DiscNumber
        | TagField::DiscTotal => Target::Pair,
        TagField::ReleaseType => Freeform("MusicBrainz Album Type"),
        TagField::ReleaseStatus => Freeform("MusicBrainz Album Status"),
        TagField::ReleaseCountry => Freeform("MusicBrainz Album Release Country"),
        TagField::MusicBrainzRecordingId => Freeform("MusicBrainz Track Id"),
        TagField::MusicBrainzTrackId => Freeform("MusicBrainz Release Track Id"),
        TagField::MusicBrainzAlbumId => Freeform("MusicBrainz Album Id"),
        TagField::MusicBrainzArtistId => Freeform("MusicBrainz Artist Id"),
        TagField::MusicBrainzAlbumArtistId => Freeform("MusicBrainz Album Artist Id"),
        TagField::MusicBrainzReleaseGroupId => Freeform("MusicBrainz Release Group Id"),
        TagField::MusicBrainzWorkId => Freeform("MusicBrainz Work Id"),
        TagField::MusicBrainzDiscId => Freeform("MusicBrainz Disc Id"),
        TagField::AcoustidId => Freeform("Acoustid Id"),
        TagField::Key => Freeform("initialkey"),
        TagField::ReplayGainTrackGain => Freeform("replaygain_track_gain"),
        TagField::ReplayGainTrackPeak => Freeform("replaygain_track_peak"),
        TagField::ReplayGainAlbumGain => Freeform("replaygain_album_gain"),
        TagField::ReplayGainAlbumPeak => Freeform("replaygain_album_peak"),
        _ => VorbisName,
    }
}

fn fourcc_ident(code: &[u8; 4]) -> AtomIdent<'static> {
    AtomIdent::Fourcc(*code)
}

fn freeform_ident(name: &str) -> AtomIdent<'static> {
    AtomIdent::Freeform {
        mean: Cow::Borrowed(ITUNES_MEAN),
        name: Cow::Owned(name.to_owned()),
    }
}

/// The table field an atom is read as, as the reader decides it.
fn atom_field(ident: &AtomIdent<'_>) -> Option<Field> {
    match ident {
        AtomIdent::Fourcc(code) => {
            let name: String = code.iter().map(|&c| char::from(c)).collect();
            match name.as_str() {
                "gnre" => Some(Field::Genre),
                "trkn" => Some(Field::TrackNumber),
                "disk" => Some(Field::DiscNumber),
                _ => mp4::atom_field(&name),
            }
        }
        AtomIdent::Freeform { name, .. } => mp4::freeform_field(name),
    }
}

/// The `ilst` of an MP4 file, being edited.
#[derive(Debug)]
pub(crate) struct Mp4Editor<'a> {
    tag: &'a mut Ilst,
}

impl<'a> Mp4Editor<'a> {
    pub(crate) fn new(tag: &'a mut Ilst) -> Self {
        Self { tag }
    }

    fn remove_fields(&mut self, fields: &[Field]) {
        // `trkn` and `disk` are written through their own accessors.
        self.tag.retain(|atom| {
            let ident = atom.ident();
            let pair = matches!(ident, AtomIdent::Fourcc(c) if c == b"trkn" || c == b"disk");
            pair || atom_field(ident).is_none_or(|f| !fields.contains(&f))
        });
    }

    fn put(&mut self, ident: AtomIdent<'static>, data: Vec<AtomData>) {
        if let Some(atom) = Atom::from_collection(ident, data) {
            self.tag.replace_atom(atom);
        }
    }

    fn put_text(&mut self, ident: AtomIdent<'static>, values: &[String]) {
        let data = values.iter().cloned().map(AtomData::UTF8).collect();
        self.put(ident, data);
    }

    fn put_integer(&mut self, code: &[u8; 4], value: Option<u32>) {
        let Some(value) = value else {
            return;
        };
        let value = u16::try_from(value).unwrap_or(u16::MAX);
        self.put(
            fourcc_ident(code),
            vec![AtomData::Unknown {
                code: DataType::BeSignedInteger,
                data: value.to_be_bytes().to_vec(),
            }],
        );
    }
}

impl TagEditor for Mp4Editor<'_> {
    fn set_field(&mut self, field: TagField, values: &[String]) -> Result<(), Skip> {
        if field.is_r128() {
            return Err(ONLY_OPUS);
        }
        self.remove_fields(&replaced_fields(field));
        if values.is_empty() {
            return Ok(());
        }
        match target(field) {
            Target::Text(code) => self.put_text(fourcc_ident(code), values),
            Target::Integer(code) => {
                self.put_integer(code, values[0].parse().ok());
            }
            Target::Flag(code) => self.put(
                fourcc_ident(code),
                vec![AtomData::Unknown {
                    code: DataType::BeSignedInteger,
                    data: vec![u8::from(values[0] == "1")],
                }],
            ),
            Target::Freeform(name) => self.put_text(freeform_ident(name), values),
            Target::VorbisName => self.put_text(freeform_ident(&vorbis::key(field)), values),
            Target::Pair => return Err("written with its pair"),
        }
        Ok(())
    }

    fn set_custom(&mut self, key: &str, values: &[String]) -> Result<(), Skip> {
        self.tag.retain(|atom| match atom.ident() {
            AtomIdent::Freeform { name, .. } => !name.eq_ignore_ascii_case(key),
            AtomIdent::Fourcc(_) => true,
        });
        self.put_text(freeform_ident(key), values);
        Ok(())
    }

    fn set_pair(&mut self, kind: PairKind, pair: Pair) -> Result<(), Skip> {
        self.remove_fields(&pair_fields(kind));
        match kind {
            PairKind::Track => {
                self.tag.remove_track();
                self.tag.remove_track_total();
                if let Some(n) = pair.number {
                    self.tag.set_track(n);
                }
                if let Some(total) = pair.total {
                    self.tag.set_track_total(total);
                }
            }
            PairKind::Disc => {
                self.tag.remove_disk();
                self.tag.remove_disk_total();
                if let Some(n) = pair.number {
                    self.tag.set_disk(n);
                }
                if let Some(total) = pair.total {
                    self.tag.set_disk_total(total);
                }
            }
            PairKind::Movement => {
                self.put_integer(b"\xa9mvi", pair.number);
                self.put_integer(b"\xa9mvc", pair.total);
            }
        }
        Ok(())
    }

    fn set_lyrics(&mut self, lyrics: Option<&str>) -> Result<(), Skip> {
        self.remove_fields(&[Field::Lyrics]);
        if let Some(text) = lyrics {
            self.put_text(fourcc_ident(b"\xa9lyr"), &[text.to_owned()]);
        }
        Ok(())
    }

    fn set_rating(&mut self, rating: Option<Rating>) -> Result<(), Skip> {
        self.remove_fields(&[Field::FmpsRating, Field::Rating]);
        let Some(rating) = rating else {
            return Ok(());
        };
        self.put_text(
            freeform_ident("FMPS_Rating"),
            &[rating_values::fmps(rating)],
        );
        if let Some(percent) = rating_values::percent(rating) {
            self.put_text(freeform_ident("RATING"), &[percent]);
        }
        Ok(())
    }

    fn set_play_count(&mut self, count: Option<u64>) -> Result<(), Skip> {
        self.set_custom(
            rating_values::FMPS_PLAYCOUNT,
            &rating_values::play_count(count),
        )
    }

    fn set_cover(&mut self, change: &CoverChange) -> Result<(), Skip> {
        // `covr` pictures have no types: the front cover is every picture.
        self.tag.remove_pictures();
        if let CoverChange::SetFront(bytes) = change {
            self.tag.insert_picture(front_picture(bytes));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_target_reads_back_as_its_field() {
        for field in TagField::ALL {
            let read = match target(*field) {
                Target::Text(code) | Target::Integer(code) | Target::Flag(code) => {
                    atom_field(&fourcc_ident(code))
                }
                Target::Freeform(name) => mp4::freeform_field(name),
                Target::VorbisName => mp4::freeform_field(&vorbis::key(*field)),
                Target::Pair => continue,
            };
            assert_eq!(read, Some(field.field()), "{field:?}");
        }
    }
}
