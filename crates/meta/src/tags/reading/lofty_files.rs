// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading through lofty, one concrete tag type at a time, so that the frames and
//! keys are seen as the file stores them (frame encodings, `POPM` e-mail fields,
//! `TXXX` descriptions, freeform atom names) and mapped by this crate's tables.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::time::Duration;

use lofty::aac::AacFile;
use lofty::ape::{ApeFile, ApeTag};
use lofty::config::{ParseOptions, ParsingMode};
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v1::{GENRES, Id3v1Tag};
use lofty::id3::v2::{Frame, Id3v2Tag};
use lofty::iff::aiff::{AiffFile, AiffTextChunks};
use lofty::iff::wav::{RiffInfoList, WavFile};
use lofty::mp4::{AtomData, AtomIdent, Ilst, Mp4Codec, Mp4File};
use lofty::mpeg::{Layer, MpegFile};
use lofty::musepack::MpcFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OpusFile, SpeexFile, VorbisFile};
use lofty::probe::Probe;
use lofty::properties::FileProperties;
use lofty::tag::{Accessor, ItemValue};
use lofty::wavpack::WavPackFile;

use super::TagError;
use super::text::TextFixer;
use crate::tags::builder::TagBuilder;
use crate::tags::field::Field;
use crate::tags::mapping::{ape, id3v2, mp4, riff, vorbis};
use crate::tags::rating;
use crate::tags::record::{AudioProperties, TrackTags};

/// Reads the tags and properties of `file`, or says why lofty cannot.
pub(super) fn read(
    file: File,
    fixer: &mut TextFixer,
) -> Result<(TrackTags, AudioProperties), TagError> {
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(0))?;
    let file_type = Probe::new(&mut reader)
        .guess_file_type()?
        .file_type()
        .ok_or(TagError::UnsupportedFormat)?;
    reader.seek(SeekFrom::Start(0))?;

    let mut builder = TagBuilder::new();
    let audio = match file_type {
        FileType::Mpeg => {
            let f: MpegFile = parse(&mut reader)?;
            id3v2_tag(&mut builder, fixer, f.id3v2());
            ape_tag(&mut builder, fixer, f.ape());
            id3v1_tag(&mut builder, fixer, f.id3v1());
            let codec = match f.properties().layer() {
                Layer::Layer1 => "mp1",
                Layer::Layer2 => "mp2",
                Layer::Layer3 => "mp3",
            };
            properties(file_properties(f.properties()), codec, "mpeg")
        }
        FileType::Aac => {
            let f: AacFile = parse(&mut reader)?;
            id3v2_tag(&mut builder, fixer, f.id3v2());
            id3v1_tag(&mut builder, fixer, f.id3v1());
            properties(file_properties(f.properties()), "aac", "adts")
        }
        FileType::Flac => {
            let f: FlacFile = parse(&mut reader)?;
            vorbis_tag(&mut builder, fixer, f.vorbis_comments());
            id3v2_tag(&mut builder, fixer, f.id3v2());
            properties(file_properties(f.properties()), "flac", "flac")
        }
        FileType::Opus => {
            let f: OpusFile = parse(&mut reader)?;
            vorbis_tag(&mut builder, fixer, Some(f.vorbis_comments()));
            properties(file_properties(f.properties()), "opus", "ogg")
        }
        FileType::Vorbis => {
            let f: VorbisFile = parse(&mut reader)?;
            vorbis_tag(&mut builder, fixer, Some(f.vorbis_comments()));
            properties(file_properties(f.properties()), "vorbis", "ogg")
        }
        FileType::Speex => {
            let f: SpeexFile = parse(&mut reader)?;
            vorbis_tag(&mut builder, fixer, Some(f.vorbis_comments()));
            properties(file_properties(f.properties()), "speex", "ogg")
        }
        FileType::Mp4 => {
            let f: Mp4File = parse(&mut reader)?;
            mp4_tag(&mut builder, fixer, f.ilst());
            let codec = match f.properties().codec() {
                Some(Mp4Codec::AAC) => "aac",
                Some(Mp4Codec::ALAC) => "alac",
                Some(Mp4Codec::MP3) => "mp3",
                Some(Mp4Codec::FLAC) => "flac",
                _ => "unknown",
            };
            properties(file_properties(f.properties()), codec, "mp4")
        }
        FileType::Ape => {
            let f: ApeFile = parse(&mut reader)?;
            ape_tag(&mut builder, fixer, f.ape());
            id3v2_tag(&mut builder, fixer, f.id3v2());
            id3v1_tag(&mut builder, fixer, f.id3v1());
            properties(file_properties(f.properties()), "ape", "ape")
        }
        FileType::Mpc => {
            let f: MpcFile = parse(&mut reader)?;
            ape_tag(&mut builder, fixer, f.ape());
            id3v2_tag(&mut builder, fixer, f.id3v2());
            id3v1_tag(&mut builder, fixer, f.id3v1());
            properties(file_properties(f.properties()), "musepack", "mpc")
        }
        FileType::WavPack => {
            let f: WavPackFile = parse(&mut reader)?;
            ape_tag(&mut builder, fixer, f.ape());
            id3v1_tag(&mut builder, fixer, f.id3v1());
            properties(file_properties(f.properties()), "wavpack", "wavpack")
        }
        FileType::Wav => {
            let f: WavFile = parse(&mut reader)?;
            id3v2_tag(&mut builder, fixer, f.id3v2());
            riff_tag(&mut builder, fixer, f.riff_info());
            properties(file_properties(f.properties()), "pcm", "wav")
        }
        FileType::Aiff => {
            let f: AiffFile = parse(&mut reader)?;
            id3v2_tag(&mut builder, fixer, f.id3v2());
            aiff_tag(&mut builder, fixer, f.text_chunks());
            properties(file_properties(f.properties()), "pcm", "aiff")
        }
        _ => return Err(TagError::UnsupportedFormat),
    };
    Ok((builder.finish(), audio))
}

fn parse<F: AudioFile, R: Read + Seek>(reader: &mut R) -> Result<F, TagError> {
    let options = ParseOptions::new()
        .read_cover_art(false)
        .parsing_mode(ParsingMode::BestAttempt);
    F::read_from(reader, options).map_err(|e| TagError::Parse(e.to_string()))
}

/// The generic properties of any lofty file type.
fn file_properties<P: Clone + Into<FileProperties>>(p: &P) -> FileProperties {
    p.clone().into()
}

fn properties(p: FileProperties, codec: &str, container: &str) -> AudioProperties {
    let positive = |v: Option<u32>| v.filter(|v| *v > 0);
    AudioProperties {
        duration: Some(p.duration()).filter(|d| *d > Duration::ZERO),
        sample_rate: positive(p.sample_rate()),
        channels: p.channels().filter(|c| *c > 0).map(u16::from),
        bit_depth: p.bit_depth().filter(|b| *b > 0).map(u32::from),
        bitrate_kbps: positive(p.audio_bitrate()).or(positive(p.overall_bitrate())),
        codec: Some(codec.to_owned()),
        container: Some(container.to_owned()),
    }
}

fn id3v2_tag(b: &mut TagBuilder, fixer: &mut TextFixer, tag: Option<&Id3v2Tag>) {
    let Some(tag) = tag else {
        return;
    };
    b.begin_tag();
    fixer.begin_tag(tag.into_iter().flat_map(frame_texts));
    // iTunes keeps the grouping in GRP1 and the work in TIT1; Picard does the same
    // when asked to, and otherwise writes the grouping to TIT1.
    let has_grp1 = tag.into_iter().any(|f| f.id_str() == "GRP1");
    for frame in tag {
        match frame {
            Frame::Text(f) => {
                let id = f.id().as_str();
                let field = match id {
                    "TIT1" if has_grp1 => Some(Field::Work),
                    "TIT1" => Some(Field::Grouping),
                    _ => id3v2::frame_field(id),
                };
                for value in fixer.values(&f.value) {
                    match field {
                        Some(field) => b.apply(field, &value),
                        None => b.apply_unknown(id, &value),
                    }
                }
            }
            Frame::UserText(f) => {
                let description = fixer.fix(&f.description).into_owned();
                let field = id3v2::user_text_field(description.trim());
                for value in fixer.values(&f.content) {
                    match field {
                        Some(field) => b.apply(field, &value),
                        None => b.apply_unknown(&description, &value),
                    }
                }
            }
            Frame::Comment(f) => {
                let description = f.description.trim();
                let field = if description.is_empty() || description == "ID3v1 Comment" {
                    Some(Field::Comment)
                } else if junk_comment(description) {
                    None
                } else {
                    let key = format!("comment:{}", fixer.fix(description));
                    for value in fixer.values(&f.content) {
                        b.apply_unknown(&key, &value);
                    }
                    None
                };
                if let Some(field) = field {
                    let content = fixer.fix(&f.content).into_owned();
                    b.apply(field, &content);
                }
            }
            Frame::UnsynchronizedText(f) => {
                if !f.content.trim().is_empty() {
                    b.mark_lyrics();
                }
            }
            Frame::Binary(f) if f.id().as_str() == "SYLT" => b.mark_lyrics(),
            Frame::Popularimeter(f) => b.set_popm_rating(rating::from_popm(&f.email, f.rating)),
            Frame::KeyValue(f) => {
                let musicians = f.id().as_str() == "TMCL";
                for (key, name) in f.key_value_pairs.iter() {
                    let name = fixer.fix(name).into_owned();
                    if musicians {
                        let instrument = fixer.fix(key).into_owned();
                        b.add_credit(Field::Performer, &name, Some(&instrument));
                    } else if let Some(field) = id3v2::involvement_field(key) {
                        b.apply(field, &name);
                    }
                }
            }
            Frame::UniqueFileIdentifier(f) => {
                if f.owner == "http://musicbrainz.org"
                    && let Ok(id) = std::str::from_utf8(&f.identifier)
                {
                    b.apply(Field::MbRecordingId, id);
                }
            }
            Frame::Timestamp(f) => {
                let field = match f.id().as_str() {
                    "TDRC" => Field::Date,
                    "TDOR" => Field::OriginalDate,
                    "TDRL" => Field::ReleaseDate,
                    _ => continue,
                };
                b.apply(field, &f.timestamp.to_string());
            }
            _ => {}
        }
    }
}

/// Comments with a description that players use for their own data.
fn junk_comment(description: &str) -> bool {
    ["iTun", "Songs-DB", "MusicMatch", "c0"]
        .iter()
        .any(|p| description.starts_with(p))
}

/// The text of a frame that the charset detector should see.
fn frame_texts<'a>(frame: &'a Frame<'_>) -> Vec<&'a str> {
    match frame {
        Frame::Text(f) => vec![&*f.value],
        Frame::UserText(f) => vec![&*f.content],
        Frame::Comment(f) => vec![&*f.content],
        Frame::KeyValue(f) => f.key_value_pairs.iter().map(|(_, v)| &**v).collect(),
        _ => Vec::new(),
    }
}

fn id3v1_tag(b: &mut TagBuilder, fixer: &mut TextFixer, tag: Option<&Id3v1Tag>) {
    let Some(tag) = tag else {
        return;
    };
    b.begin_tag();
    let texts = [&tag.title, &tag.artist, &tag.album, &tag.comment];
    fixer.begin_tag(texts.iter().filter_map(|t| t.as_deref()));
    for (field, text) in [Field::Title, Field::Artist, Field::Album, Field::Comment]
        .into_iter()
        .zip(texts)
    {
        if let Some(text) = text {
            let text = fixer.fix(text).into_owned();
            b.apply(field, &text);
        }
    }
    if let Some(year) = tag.year {
        b.apply(Field::Year, &year.to_string());
    }
    if let Some(track) = tag.track_number {
        b.apply(Field::TrackNumber, &track.to_string());
    }
    if let Some(genre) = tag.genre.and_then(|g| GENRES.get(usize::from(g))) {
        b.apply(Field::Genre, genre);
    }
}

fn ape_tag(b: &mut TagBuilder, fixer: &mut TextFixer, tag: Option<&ApeTag>) {
    let Some(tag) = tag else {
        return;
    };
    b.begin_tag();
    fixer.begin_tag(tag.into_iter().filter_map(|item| match item.value() {
        ItemValue::Text(t) => Some(t.as_str()),
        _ => None,
    }));
    for item in tag {
        let ItemValue::Text(text) = item.value() else {
            continue;
        };
        let key = item.key();
        let field = ape::field(key);
        for value in fixer.values(text) {
            match field {
                Some(field) => b.apply(field, &value),
                None => b.apply_unknown(key, &value),
            }
        }
    }
}

fn vorbis_tag(b: &mut TagBuilder, fixer: &mut TextFixer, tag: Option<&VorbisComments>) {
    let Some(tag) = tag else {
        return;
    };
    b.begin_tag();
    fixer.begin_tag(tag.items().map(|(_, v)| v));
    for (key, value) in tag.items() {
        let value = fixer.fix(value).into_owned();
        match vorbis::field(key) {
            Some(field) => b.apply(field, &value),
            None => b.apply_unknown(key, &value),
        }
    }
}

fn mp4_tag(b: &mut TagBuilder, fixer: &mut TextFixer, tag: Option<&Ilst>) {
    let Some(tag) = tag else {
        return;
    };
    b.begin_tag();
    fixer.begin_tag(tag.into_iter().flat_map(|atom| {
        atom.data().filter_map(|d| match d {
            AtomData::UTF8(s) | AtomData::UTF16(s) => Some(s.as_str()),
            _ => None,
        })
    }));
    if let Some(track) = tag.track() {
        b.apply(Field::TrackNumber, &track.to_string());
    }
    if let Some(total) = tag.track_total() {
        b.apply(Field::TotalTracks, &total.to_string());
    }
    if let Some(disc) = tag.disk() {
        b.apply(Field::DiscNumber, &disc.to_string());
    }
    if let Some(total) = tag.disk_total() {
        b.apply(Field::TotalDiscs, &total.to_string());
    }
    for atom in tag {
        let (field, unknown_key) = match atom.ident() {
            AtomIdent::Fourcc(code) => {
                let name: String = code.iter().map(|&c| char::from(c)).collect();
                match name.as_str() {
                    "trkn" | "disk" => continue,
                    "gnre" => {
                        for data in atom.data() {
                            if let Some(genre) = id3_genre_index(data) {
                                b.apply(Field::Genre, genre);
                            }
                        }
                        continue;
                    }
                    _ => (mp4::atom_field(&name), name),
                }
            }
            AtomIdent::Freeform { name, .. } => (mp4::freeform_field(name), name.to_string()),
        };
        for data in atom.data() {
            let Some(value) = atom_text(data) else {
                continue;
            };
            let value = fixer.fix(&value).into_owned();
            match field {
                Some(field) => b.apply(field, &value),
                None => b.apply_unknown(&unknown_key, &value),
            }
        }
    }
}

/// The text of one MP4 data atom; integers and flags as decimal.
fn atom_text(data: &AtomData) -> Option<String> {
    match data {
        AtomData::UTF8(s) | AtomData::UTF16(s) => Some(s.clone()),
        AtomData::SignedInteger(i) => Some(i.to_string()),
        AtomData::UnsignedInteger(u) => Some(u.to_string()),
        AtomData::Bool(flag) => Some(if *flag { "1" } else { "0" }.to_owned()),
        AtomData::Unknown { data, .. } => match data.len() {
            // Integers written with the "implicit" type, such as `tmpo` and `cpil`.
            1 => Some(data[0].to_string()),
            2 => Some(u16::from_be_bytes([data[0], data[1]]).to_string()),
            _ => std::str::from_utf8(data).ok().map(str::to_owned),
        },
        AtomData::Picture(_) => None,
    }
}

/// A `gnre` atom: a 1-based index into the ID3v1 genre list.
fn id3_genre_index(data: &AtomData) -> Option<&'static str> {
    let index = match data {
        AtomData::Unknown { data, .. } if data.len() == 2 => u16::from_be_bytes([data[0], data[1]]),
        AtomData::SignedInteger(i) => u16::try_from(*i).ok()?,
        AtomData::UnsignedInteger(u) => u16::try_from(*u).ok()?,
        _ => return None,
    };
    GENRES.get(usize::from(index.checked_sub(1)?)).copied()
}

fn riff_tag(b: &mut TagBuilder, fixer: &mut TextFixer, tag: Option<&RiffInfoList>) {
    let Some(tag) = tag else {
        return;
    };
    b.begin_tag();
    fixer.begin_tag(tag.into_iter().map(|(_, v)| v.as_str()));
    for (key, value) in tag {
        let value = fixer.fix(value).into_owned();
        match riff::field(key) {
            Some(field) => b.apply(field, &value),
            None => b.apply_unknown(key, &value),
        }
    }
}

fn aiff_tag(b: &mut TagBuilder, fixer: &mut TextFixer, tag: Option<&AiffTextChunks>) {
    let Some(tag) = tag else {
        return;
    };
    b.begin_tag();
    let mut fields: Vec<(Field, &str)> = Vec::new();
    if let Some(name) = &tag.name {
        fields.push((Field::Title, name));
    }
    if let Some(author) = &tag.author {
        fields.push((Field::Artist, author));
    }
    if let Some(copyright) = &tag.copyright {
        fields.push((Field::Copyright, copyright));
    }
    for annotation in tag.annotations.iter().flatten() {
        fields.push((Field::Comment, annotation));
    }
    for comment in tag.comments.iter().flatten() {
        fields.push((Field::Comment, &comment.text));
    }
    fixer.begin_tag(fields.iter().map(|(_, v)| *v));
    for (field, value) in fields {
        let value = fixer.fix(value).into_owned();
        b.apply(field, &value);
    }
}
