// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which formats can be written, and the edit of one open file: parse it with
//! lofty (keeping the pictures and every frame the editor does not touch), apply
//! the plan to its tags in memory, and save them back into the same file.

use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};

use lofty::aac::AacFile;
use lofty::ape::{ApeFile, ApeTag};
use lofty::config::{ParseOptions, ParsingMode, WriteOptions as LoftyWriteOptions};
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::{Id3v2Tag, Id3v2Version};
use lofty::iff::aiff::AiffFile;
use lofty::iff::wav::WavFile;
use lofty::mp4::Mp4File;
use lofty::mpeg::MpegFile;
use lofty::musepack::MpcFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OggPictureStorage, OpusFile, SpeexFile, VorbisFile};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::wavpack::WavPackFile;

use super::WriteError;
use super::ape::ApeEditor;
use super::changes::CoverChange;
use super::editor::{Plan, SkippedChange, TagEditor, apply};
use super::id3v2::Id3Editor;
use super::mp4::Mp4Editor;
use super::secondary::{AiffEditor, Id3v1Editor, RiffEditor};
use super::vorbis::{VorbisEditor, edit_pictures};

/// The ID3v2 version of a written tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Id3Version {
    /// ID3v2.3.
    V2_3,
    /// ID3v2.4.
    V2_4,
}

/// What an edit did to one file's tags.
#[derive(Debug, Default)]
pub(crate) struct Edited {
    /// Changes the main tag could not take.
    pub(crate) skipped: Vec<SkippedChange>,
    /// The version of the ID3v2 tag written, when the main tag is ID3v2.
    pub(crate) id3_version: Option<Id3Version>,
}

/// The lofty file type of `file`, if lofty can write its tags. Leaves the file
/// at an unspecified position.
pub(crate) fn writable_type(file: &mut File) -> Result<Option<FileType>, WriteError> {
    file.seek(SeekFrom::Start(0))?;
    let file_type = Probe::new(BufReader::new(&mut *file))
        .guess_file_type()?
        .file_type();
    Ok(file_type.filter(|t| {
        matches!(
            t,
            FileType::Mpeg
                | FileType::Aac
                | FileType::Flac
                | FileType::Opus
                | FileType::Vorbis
                | FileType::Speex
                | FileType::Mp4
                | FileType::Ape
                | FileType::Mpc
                | FileType::WavPack
                | FileType::Wav
                | FileType::Aiff
        )
    }))
}

fn parse<F: AudioFile>(file: &mut File) -> Result<F, WriteError> {
    file.seek(SeekFrom::Start(0))?;
    let options = ParseOptions::new()
        .read_cover_art(true)
        .parsing_mode(ParsingMode::BestAttempt);
    F::read_from(&mut BufReader::new(&mut *file), options)
        .map_err(|e| WriteError::Parse(e.to_string()))
}

fn save<F: AudioFile>(parsed: &F, file: &mut File, v23: bool) -> Result<(), WriteError> {
    file.seek(SeekFrom::Start(0))?;
    parsed
        .save_to(file, LoftyWriteOptions::new().use_id3v23(v23))
        .map_err(|e| WriteError::Save(e.to_string()))
}

/// Whether an ID3v2 tag is written as ID3v2.3: an existing tag keeps its version
/// (ID3v2.2, which cannot be written, gets the preferred one), a new tag gets the
/// preferred one.
fn id3_v23(tag: Option<&Id3v2Tag>, preferred: Id3Version) -> bool {
    match tag.map(Id3v2Tag::original_version) {
        Some(Id3v2Version::V3) => true,
        Some(Id3v2Version::V4) => false,
        _ => preferred == Id3Version::V2_3,
    }
}

fn version(v23: bool) -> Option<Id3Version> {
    Some(if v23 {
        Id3Version::V2_3
    } else {
        Id3Version::V2_4
    })
}

/// Applies `plan` to the main ID3v2 tag (created if missing) and to `others`.
fn edit_id3(
    tag: &mut Option<Id3v2Tag>,
    plan: &Plan<'_>,
    preferred: Id3Version,
) -> (Vec<SkippedChange>, bool) {
    let v23 = id3_v23(tag.as_ref(), preferred);
    let tag = tag.get_or_insert_with(Id3v2Tag::new);
    (apply(&mut Id3Editor::new(tag, v23), plan), v23)
}

fn secondary(editor: &mut dyn TagEditor, plan: &Plan<'_>) {
    // The main tag reports what could not be written; these tags take what they can.
    drop(apply(editor, plan));
}

/// Edits the tags of the open, writable `file` (of `file_type`) by `plan`, and
/// saves them into it.
pub(crate) fn edit(
    file: &mut File,
    file_type: FileType,
    plan: &Plan<'_>,
    preferred: Id3Version,
) -> Result<Edited, WriteError> {
    let mut edited = Edited::default();
    match file_type {
        FileType::Mpeg => {
            let mut f: MpegFile = parse(file)?;
            let mut tag = f.remove_id3v2();
            let (skipped, v23) = edit_id3(&mut tag, plan, preferred);
            if let Some(tag) = tag {
                f.set_id3v2(tag);
            }
            if let Some(ape) = f.ape_mut() {
                secondary(&mut ApeEditor::new(ape), plan);
            }
            if let Some(v1) = f.id3v1_mut() {
                secondary(&mut Id3v1Editor::new(v1), plan);
            }
            save(&f, file, v23)?;
            edited.skipped = skipped;
            edited.id3_version = version(v23);
        }
        FileType::Aac => {
            let mut f: AacFile = parse(file)?;
            let mut tag = f.remove_id3v2();
            let (skipped, v23) = edit_id3(&mut tag, plan, preferred);
            if let Some(tag) = tag {
                f.set_id3v2(tag);
            }
            if let Some(v1) = f.id3v1_mut() {
                secondary(&mut Id3v1Editor::new(v1), plan);
            }
            save(&f, file, v23)?;
            edited.skipped = skipped;
            edited.id3_version = version(v23);
        }
        FileType::Wav => {
            let mut f: WavFile = parse(file)?;
            let mut tag = f.remove_id3v2();
            let (skipped, v23) = edit_id3(&mut tag, plan, preferred);
            if let Some(tag) = tag {
                f.set_id3v2(tag);
            }
            if let Some(info) = f.riff_info_mut() {
                secondary(&mut RiffEditor::new(info), plan);
            }
            save(&f, file, v23)?;
            edited.skipped = skipped;
            edited.id3_version = version(v23);
        }
        FileType::Aiff => {
            let mut f: AiffFile = parse(file)?;
            let mut tag = f.remove_id3v2();
            let (skipped, v23) = edit_id3(&mut tag, plan, preferred);
            if let Some(tag) = tag {
                f.set_id3v2(tag);
            }
            if let Some(chunks) = f.text_chunks_mut() {
                secondary(&mut AiffEditor::new(chunks), plan);
            }
            save(&f, file, v23)?;
            edited.skipped = skipped;
            edited.id3_version = version(v23);
        }
        FileType::Flac => {
            let mut f: FlacFile = parse(file)?;
            let mut comments = f.remove_vorbis_comments().unwrap_or_default();
            edited.skipped = apply(&mut VorbisEditor::new(&mut comments, false), plan);
            if let Some(change) = &plan.changes.cover {
                // Pictures are FLAC blocks; a picture comment, if any, goes too.
                edit_pictures(&mut f, change).map_err(WriteError::Picture)?;
                match change {
                    CoverChange::RemoveAll => drop(comments.remove_pictures()),
                    _ => comments.remove_picture_type(PictureType::CoverFront),
                }
            }
            f.set_vorbis_comments(comments);
            save(&f, file, false)?;
        }
        FileType::Opus => {
            let mut f: OpusFile = parse(file)?;
            edited.skipped = edit_comments(f.vorbis_comments_mut(), plan);
            save(&f, file, false)?;
        }
        FileType::Vorbis => {
            let mut f: VorbisFile = parse(file)?;
            edited.skipped = edit_comments(f.vorbis_comments_mut(), plan);
            save(&f, file, false)?;
        }
        FileType::Speex => {
            let mut f: SpeexFile = parse(file)?;
            edited.skipped = edit_comments(f.vorbis_comments_mut(), plan);
            save(&f, file, false)?;
        }
        FileType::Mp4 => {
            let mut f: Mp4File = parse(file)?;
            let mut ilst = f.remove_ilst().unwrap_or_default();
            edited.skipped = apply(&mut Mp4Editor::new(&mut ilst), plan);
            f.set_ilst(ilst);
            save(&f, file, false)?;
        }
        FileType::Ape => {
            let mut f: ApeFile = parse(file)?;
            edited.skipped = edit_ape(&mut f.remove_ape(), plan, |ape| drop(f.set_ape(ape)));
            if let Some(v1) = f.id3v1_mut() {
                secondary(&mut Id3v1Editor::new(v1), plan);
            }
            save(&f, file, false)?;
        }
        FileType::Mpc => {
            let mut f: MpcFile = parse(file)?;
            edited.skipped = edit_ape(&mut f.remove_ape(), plan, |ape| drop(f.set_ape(ape)));
            if let Some(v1) = f.id3v1_mut() {
                secondary(&mut Id3v1Editor::new(v1), plan);
            }
            save(&f, file, false)?;
        }
        FileType::WavPack => {
            let mut f: WavPackFile = parse(file)?;
            edited.skipped = edit_ape(&mut f.remove_ape(), plan, |ape| drop(f.set_ape(ape)));
            if let Some(v1) = f.id3v1_mut() {
                secondary(&mut Id3v1Editor::new(v1), plan);
            }
            save(&f, file, false)?;
        }
        _ => return Err(WriteError::ReadOnlyFormat),
    }
    Ok(edited)
}

fn edit_comments(comments: &mut VorbisComments, plan: &Plan<'_>) -> Vec<SkippedChange> {
    apply(&mut VorbisEditor::new(comments, true), plan)
}

fn edit_ape(
    tag: &mut Option<ApeTag>,
    plan: &Plan<'_>,
    put_back: impl FnOnce(ApeTag),
) -> Vec<SkippedChange> {
    let mut tag = tag.take().unwrap_or_default();
    let skipped = apply(&mut ApeEditor::new(&mut tag), plan);
    put_back(tag);
    skipped
}

/// The names of the tags a parsed file carries, such as `ID3v2.4` and `APEv2`,
/// for the editor's file list.
pub(crate) fn tag_formats(file: &mut File) -> Result<Vec<String>, WriteError> {
    let Some(file_type) = writable_type(file)? else {
        return Ok(Vec::new());
    };
    let id3 = |tag: Option<&Id3v2Tag>| {
        tag.map(|t| match t.original_version() {
            Id3v2Version::V2 => "ID3v2.2",
            Id3v2Version::V3 => "ID3v2.3",
            Id3v2Version::V4 => "ID3v2.4",
        })
    };
    let mut names: Vec<Option<&str>> = Vec::new();
    match file_type {
        FileType::Mpeg => {
            let f: MpegFile = parse_light(file)?;
            names.extend([
                id3(f.id3v2()),
                f.ape().map(|_| "APEv2"),
                f.id3v1().map(|_| "ID3v1"),
            ]);
        }
        FileType::Aac => {
            let f: AacFile = parse_light(file)?;
            names.extend([id3(f.id3v2()), f.id3v1().map(|_| "ID3v1")]);
        }
        FileType::Wav => {
            let f: WavFile = parse_light(file)?;
            names.extend([id3(f.id3v2()), f.riff_info().map(|_| "RIFF INFO")]);
        }
        FileType::Aiff => {
            let f: AiffFile = parse_light(file)?;
            names.extend([id3(f.id3v2()), f.text_chunks().map(|_| "AIFF text")]);
        }
        FileType::Flac => {
            let f: FlacFile = parse_light(file)?;
            names.extend([
                f.vorbis_comments().map(|_| "Vorbis comments"),
                id3(f.id3v2()),
            ]);
        }
        FileType::Opus | FileType::Vorbis | FileType::Speex => names.push(Some("Vorbis comments")),
        FileType::Mp4 => {
            let f: Mp4File = parse_light(file)?;
            names.push(f.ilst().map(|_| "iTunes ilst"));
        }
        FileType::Ape => {
            let f: ApeFile = parse_light(file)?;
            names.extend([
                f.ape().map(|_| "APEv2"),
                id3(f.id3v2()),
                f.id3v1().map(|_| "ID3v1"),
            ]);
        }
        FileType::Mpc => {
            let f: MpcFile = parse_light(file)?;
            names.extend([
                f.ape().map(|_| "APEv2"),
                id3(f.id3v2()),
                f.id3v1().map(|_| "ID3v1"),
            ]);
        }
        FileType::WavPack => {
            let f: WavPackFile = parse_light(file)?;
            names.extend([f.ape().map(|_| "APEv2"), f.id3v1().map(|_| "ID3v1")]);
        }
        _ => {}
    }
    Ok(names.into_iter().flatten().map(str::to_owned).collect())
}

fn parse_light<F: AudioFile>(file: &mut File) -> Result<F, WriteError> {
    file.seek(SeekFrom::Start(0))?;
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(false)
        .parsing_mode(ParsingMode::BestAttempt);
    F::read_from(&mut BufReader::new(&mut *file), options)
        .map_err(|e| WriteError::Parse(e.to_string()))
}

/// Reads the unsynchronized lyrics of an open file: ID3v2 `USLT`, the Vorbis
/// `LYRICS` comment, the APEv2 `Lyrics` item or the MP4 `©lyr` atom.
pub(crate) fn lyrics(file: &mut File) -> Result<Option<String>, WriteError> {
    use crate::tags::field::Field;
    use crate::tags::mapping::{ape, vorbis};
    use lofty::id3::v2::Frame;
    use lofty::tag::ItemValue;

    let Some(file_type) = writable_type(file)? else {
        return Ok(None);
    };
    let from_id3 = |tag: Option<&Id3v2Tag>| {
        tag.and_then(|t| {
            t.into_iter().find_map(|frame| match frame {
                Frame::UnsynchronizedText(f) => Some(f.content.to_string()),
                _ => None,
            })
        })
    };
    let from_vorbis = |tag: Option<&VorbisComments>| {
        tag.and_then(|t| {
            t.items()
                .find(|(k, _)| vorbis::field(k) == Some(Field::Lyrics))
                .map(|(_, v)| v.to_owned())
        })
    };
    let from_ape = |tag: Option<&ApeTag>| {
        tag.and_then(|t| {
            t.into_iter().find_map(|item| match item.value() {
                ItemValue::Text(text) if ape::field(item.key()) == Some(Field::Lyrics) => {
                    Some(text.clone())
                }
                _ => None,
            })
        })
    };
    let text = match file_type {
        FileType::Mpeg => {
            let f: MpegFile = parse_light(file)?;
            from_id3(f.id3v2()).or_else(|| from_ape(f.ape()))
        }
        FileType::Aac => from_id3(parse_light::<AacFile>(file)?.id3v2()),
        FileType::Wav => from_id3(parse_light::<WavFile>(file)?.id3v2()),
        FileType::Aiff => from_id3(parse_light::<AiffFile>(file)?.id3v2()),
        FileType::Flac => {
            let f: FlacFile = parse_light(file)?;
            from_vorbis(f.vorbis_comments()).or_else(|| from_id3(f.id3v2()))
        }
        FileType::Opus => from_vorbis(Some(parse_light::<OpusFile>(file)?.vorbis_comments())),
        FileType::Vorbis => from_vorbis(Some(parse_light::<VorbisFile>(file)?.vorbis_comments())),
        FileType::Speex => from_vorbis(Some(parse_light::<SpeexFile>(file)?.vorbis_comments())),
        FileType::Mp4 => parse_light::<Mp4File>(file)?.ilst().and_then(|ilst| {
            ilst.get(&lofty::mp4::AtomIdent::Fourcc(*b"\xa9lyr"))
                .and_then(|atom| {
                    atom.data().find_map(|d| match d {
                        lofty::mp4::AtomData::UTF8(s) | lofty::mp4::AtomData::UTF16(s) => {
                            Some(s.clone())
                        }
                        _ => None,
                    })
                })
        }),
        FileType::Ape => from_ape(parse_light::<ApeFile>(file)?.ape()),
        FileType::Mpc => from_ape(parse_light::<MpcFile>(file)?.ape()),
        FileType::WavPack => from_ape(parse_light::<WavPackFile>(file)?.ape()),
        _ => None,
    };
    Ok(text.filter(|t| !t.trim().is_empty()))
}
