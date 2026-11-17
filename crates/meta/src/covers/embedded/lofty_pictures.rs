// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Embedded pictures through lofty, one concrete tag type at a time, so that the
//! tag each picture came from is known.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

use lofty::aac::AacFile;
use lofty::ape::{ApeFile, ApeTag};
use lofty::config::{ParseOptions, ParsingMode};
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::{Frame, Id3v2Tag};
use lofty::iff::aiff::AiffFile;
use lofty::iff::wav::WavFile;
use lofty::mp4::{Ilst, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::musepack::MpcFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OggPictureStorage, OpusFile, SpeexFile, VorbisFile};
use lofty::picture::{Picture, PictureInformation};
use lofty::probe::Probe;
use lofty::tag::ItemValue;
use lofty::wavpack::WavPackFile;

use super::{Collector, EmbeddedCovers, RawPicture};
use crate::covers::limits::CoverLimits;
use crate::covers::picture_type::PictureType;
use crate::covers::source::EmbeddedTag;

/// Reads the pictures of `file`, or says why lofty cannot.
pub(super) fn read(file: File, limits: &CoverLimits) -> Result<EmbeddedCovers, String> {
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let file_type = Probe::new(&mut reader)
        .guess_file_type()
        .map_err(|e| e.to_string())?
        .file_type()
        .ok_or_else(|| "the tag library does not support this format".to_owned())?;
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;

    let mut c = Collector::new(limits);
    match file_type {
        FileType::Mpeg => {
            let f: MpegFile = parse(&mut reader)?;
            id3v2(&mut c, f.id3v2());
            ape(&mut c, f.ape());
        }
        FileType::Aac => {
            let f: AacFile = parse(&mut reader)?;
            id3v2(&mut c, f.id3v2());
        }
        FileType::Flac => {
            let f: FlacFile = parse(&mut reader)?;
            ogg_pictures(&mut c, EmbeddedTag::FlacPicture, f.pictures());
            if let Some(comments) = f.vorbis_comments() {
                vorbis(&mut c, comments);
            }
            id3v2(&mut c, f.id3v2());
        }
        FileType::Opus => {
            let f: OpusFile = parse(&mut reader)?;
            vorbis(&mut c, f.vorbis_comments());
        }
        FileType::Vorbis => {
            let f: VorbisFile = parse(&mut reader)?;
            vorbis(&mut c, f.vorbis_comments());
        }
        FileType::Speex => {
            let f: SpeexFile = parse(&mut reader)?;
            vorbis(&mut c, f.vorbis_comments());
        }
        FileType::Mp4 => {
            let f: Mp4File = parse(&mut reader)?;
            mp4(&mut c, f.ilst());
        }
        FileType::Ape => {
            let f: ApeFile = parse(&mut reader)?;
            ape(&mut c, f.ape());
            id3v2(&mut c, f.id3v2());
        }
        FileType::Mpc => {
            let f: MpcFile = parse(&mut reader)?;
            ape(&mut c, f.ape());
            id3v2(&mut c, f.id3v2());
        }
        FileType::WavPack => {
            let f: WavPackFile = parse(&mut reader)?;
            ape(&mut c, f.ape());
        }
        FileType::Wav => {
            let f: WavFile = parse(&mut reader)?;
            id3v2(&mut c, f.id3v2());
        }
        FileType::Aiff => {
            let f: AiffFile = parse(&mut reader)?;
            id3v2(&mut c, f.id3v2());
        }
        _ => return Err("the tag library does not support this format".to_owned()),
    }
    Ok(c.finish())
}

fn parse<F: AudioFile, R: Read + Seek>(reader: &mut R) -> Result<F, String> {
    let options = ParseOptions::new()
        .read_cover_art(true)
        .parsing_mode(ParsingMode::BestAttempt);
    F::read_from(reader, options)
        .map_err(|e| format!("the tag library cannot parse this file: {e}"))
}

fn raw(tag: EmbeddedTag, picture_type: Option<PictureType>, picture: &Picture) -> RawPicture<'_> {
    RawPicture {
        tag,
        picture_type,
        description: picture.description().map(str::to_owned),
        data: picture.data(),
    }
}

fn typed(picture: &Picture) -> Option<PictureType> {
    Some(PictureType(picture.pic_type().as_u8()))
}

fn id3v2(c: &mut Collector<'_>, tag: Option<&Id3v2Tag>) {
    for frame in tag.into_iter().flatten() {
        if let Frame::Picture(f) = frame {
            let picture = &f.picture;
            c.push(raw(EmbeddedTag::Id3v2, typed(picture), picture));
        }
    }
}

fn ogg_pictures(
    c: &mut Collector<'_>,
    tag: EmbeddedTag,
    pictures: &[(Picture, PictureInformation)],
) {
    for (picture, _) in pictures {
        c.push(raw(tag, typed(picture), picture));
    }
}

fn vorbis(c: &mut Collector<'_>, comments: &VorbisComments) {
    ogg_pictures(c, EmbeddedTag::VorbisComment, comments.pictures());
}

fn mp4(c: &mut Collector<'_>, ilst: Option<&Ilst>) {
    let Some(pictures) = ilst.and_then(Ilst::pictures) else {
        return;
    };
    for picture in pictures {
        // MP4 has no picture types; lofty reports every `covr` as "other".
        c.push(raw(EmbeddedTag::Mp4Cover, None, picture));
    }
}

fn ape(c: &mut Collector<'_>, tag: Option<&ApeTag>) {
    for item in tag.into_iter().flatten() {
        let ItemValue::Binary(value) = item.value() else {
            continue;
        };
        let Some(picture_type) = PictureType::from_ape_key(item.key()) else {
            continue;
        };
        // The value is a file name (or description), a NUL, then the picture.
        let (description, data) = match value.iter().position(|&b| b == 0) {
            Some(nul) => (
                Some(String::from_utf8_lossy(&value[..nul]).into_owned()),
                &value[nul + 1..],
            ),
            None => (None, &value[..]),
        };
        c.push(RawPicture {
            tag: EmbeddedTag::Ape,
            picture_type: Some(picture_type),
            description,
            data,
        });
    }
}
