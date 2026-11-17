// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The messages between the scanner and a worker, and their encoding.
//!
//! Every frame starts with a kind byte: 1 for [`Hello`], 2 for [`Request`], 3 for
//! [`Response`]. The rest of the layout is the order of the fields in the code below;
//! [`PROTOCOL_VERSION`] changes whenever it changes.
//!
//! Version 2 added the covers: [`RequestOptions::covers`], and in a [`FileReport`]
//! the file's embedded cover and its folder image as [`CoverReport`]s.

use std::ffi::OsStr;
use std::num::NonZeroU32;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use helikodu_model::audio::MediaTime;
use helikodu_model::library::{MediaSource, SourceSection};

use super::wire::{DecodeError, Decoder, Encoder};
use crate::covers::{CoverHash, EmbeddedTag, PictureFormat, PictureType};
use crate::cue::{CueOrigin, VirtualTrack};
use crate::tags::{
    AudioProperties, Credit, FileTags, MusicBrainzIds, Rating, ReplayGainTags, Role, TagSource,
    TrackTags,
};

/// The version of the message layout. A worker announces it in its [`Hello`]; the
/// scanner refuses a worker with another version.
pub const PROTOCOL_VERSION: u16 = 2;

/// The bytes that start a [`Hello`], so that a program that is not a worker is
/// recognised at once.
const MAGIC: [u8; 4] = *b"HKSW";

const KIND_HELLO: u8 = 1;
const KIND_REQUEST: u8 = 2;
const KIND_RESPONSE: u8 = 3;

/// One message, as sent in one frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// The first message of a worker.
    Hello(Hello),
    /// From the scanner: read a file.
    Request(Request),
    /// From the worker: what the file holds, or why it could not be read. Boxed: it
    /// is much larger than the other messages.
    Response(Box<Response>),
}

/// What a worker says when it starts, before any request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hello {
    /// The worker's [`PROTOCOL_VERSION`].
    pub version: u16,
    /// The worker's process id, for diagnostics.
    pub pid: u32,
}

impl Hello {
    /// The hello of this build.
    pub fn current() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            pid: std::process::id(),
        }
    }
}

/// A file to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Chosen by the scanner and returned in the [`Response`].
    pub id: u64,
    /// The file, as the bytes the kernel returned.
    pub path: PathBuf,
    /// What to read and how.
    pub options: RequestOptions,
}

/// What a worker reads besides the tags, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOptions {
    /// Whether text in legacy charsets is repaired.
    pub repair_charsets: bool,
    /// The legacy charset the user chose, as a WHATWG label, for tags and sheets.
    pub legacy_charset: Option<String>,
    /// CUE sheets that may describe the file: the `.cue` files in its folder. The
    /// first one with tracks in this file gives its virtual tracks.
    pub cue_sheets: Vec<PathBuf>,
    /// Whether a sheet embedded in the file is read when no external sheet has
    /// tracks in it.
    pub embedded_cue: bool,
    /// Whether container chapters are read (in the formats that have them).
    pub chapters: bool,
    /// Whether the embedded cover and the folder image are looked for.
    pub covers: bool,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            repair_charsets: true,
            legacy_charset: None,
            cue_sheets: Vec::new(),
            embedded_cue: true,
            chapters: true,
            covers: true,
        }
    }
}

/// The answer to a [`Request`].
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// The request's id.
    pub id: u64,
    /// What the file holds, or why it could not be read.
    pub result: Result<FileReport, Failure>,
}

/// What a worker read from one file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileReport {
    /// The size in bytes when the worker read it.
    pub size: u64,
    /// The quick hash of the content (see [`quick_hash`](super::quick_hash)).
    pub quick_hash: Vec<u8>,
    /// The tags and audio properties.
    pub tags: FileTags,
    /// The virtual tracks of the CUE sheet that describes the file, if one does.
    pub cue: Option<CueReport>,
    /// The container's chapters as virtual tracks; empty without chapters.
    pub chapters: Vec<VirtualTrack>,
    /// The picture embedded in the file that best serves as its cover, if any.
    pub cover: Option<CoverReport>,
    /// The first usable image file in the file's folder (or in the folder above a
    /// disc folder), if any.
    pub folder_cover: Option<CoverReport>,
    /// Things that went wrong without making the file unreadable, such as a sheet
    /// that could not be parsed.
    pub notes: Vec<String>,
}

/// The CUE sheet that splits a file into tracks.
#[derive(Debug, Clone, PartialEq)]
pub struct CueReport {
    /// Where the sheet is.
    pub origin: CueOrigin,
    /// The `.cue` file, for an external sheet.
    pub sheet: Option<PathBuf>,
    /// The sheet's tracks in this file, in sheet order.
    pub tracks: Vec<VirtualTrack>,
    /// What the parser skipped or repaired, as text.
    pub warnings: Vec<String>,
}

/// A cover a worker found, without its bytes: what identifies it, where it is,
/// and what its header says. The worker checks the header within the picture
/// limits but decodes no pixels; whoever needs the pixels reads the bytes again
/// from [`origin`](Self::origin) and checks them against [`hash`](Self::hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverReport {
    /// The content hash of the encoded picture.
    pub hash: CoverHash,
    /// Where the picture is.
    pub origin: CoverOrigin,
    /// Its format: JPEG, PNG or WebP.
    pub format: PictureFormat,
    /// Its width in pixels, after the stored orientation.
    pub width: u32,
    /// Its height in pixels, after the stored orientation.
    pub height: u32,
    /// The size of the encoded picture in bytes.
    pub byte_len: u64,
}

/// Where a [`CoverReport`]'s picture is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverOrigin {
    /// Embedded in the file that was read.
    Embedded {
        /// The tag or structure that holds it.
        tag: EmbeddedTag,
        /// Its picture type, when the tag has one.
        picture_type: Option<PictureType>,
        /// Its position among the file's pictures, as
        /// [`CoverSource::Embedded`](crate::covers::CoverSource::Embedded) counts them.
        index: u32,
    },
    /// An image file.
    Folder {
        /// The image file.
        path: PathBuf,
    },
}

/// Why a file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The kind of failure.
    pub kind: FailureKind,
    /// A description for the user and the log.
    pub message: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The kinds of [`Failure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureKind {
    /// The file could not be opened or read.
    Io,
    /// No reader understood the file.
    Unreadable,
    /// A reader failed in an unexpected way (a panic) and the worker survived it.
    Internal,
}

impl Message {
    /// The frame payload of this message.
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Encoder::default();
        match self {
            Self::Hello(hello) => {
                e.u8(KIND_HELLO);
                for byte in MAGIC {
                    e.u8(byte);
                }
                e.u16(hello.version);
                e.u32(hello.pid);
            }
            Self::Request(request) => {
                e.u8(KIND_REQUEST);
                e.u64(request.id);
                path(&mut e, &request.path);
                let o = &request.options;
                e.bool(o.repair_charsets);
                e.opt_str(o.legacy_charset.as_deref());
                e.list(&o.cue_sheets, |e, p| path(e, p));
                e.bool(o.embedded_cue);
                e.bool(o.chapters);
                e.bool(o.covers);
            }
            Self::Response(response) => {
                e.u8(KIND_RESPONSE);
                e.u64(response.id);
                match &response.result {
                    Ok(report) => {
                        e.u8(0);
                        file_report(&mut e, report);
                    }
                    Err(failure) => {
                        e.u8(1);
                        e.u8(match failure.kind {
                            FailureKind::Io => 0,
                            FailureKind::Unreadable => 1,
                            FailureKind::Internal => 2,
                        });
                        e.str(&failure.message);
                    }
                }
            }
        }
        e.finish()
    }

    /// Decodes a frame payload. A [`Hello`] of another version decodes as far as its
    /// version, so that the mismatch can be reported; everything else must match this
    /// build's layout exactly.
    pub fn decode(payload: &[u8]) -> Result<Self, DecodeError> {
        let mut d = Decoder::new(payload);
        let message = match d.u8()? {
            KIND_HELLO => {
                let magic = [d.u8()?, d.u8()?, d.u8()?, d.u8()?];
                if magic != MAGIC {
                    return Err(DecodeError::Invalid("greeting"));
                }
                let version = d.u16()?;
                let pid = d.u32()?;
                if version != PROTOCOL_VERSION {
                    // A future layout may add fields after these.
                    return Ok(Self::Hello(Hello { version, pid }));
                }
                Self::Hello(Hello { version, pid })
            }
            KIND_REQUEST => {
                let id = d.u64()?;
                let path = read_path(&mut d)?;
                let options = RequestOptions {
                    repair_charsets: d.bool()?,
                    legacy_charset: d.opt_string()?,
                    cue_sheets: d.list(read_path)?,
                    embedded_cue: d.bool()?,
                    chapters: d.bool()?,
                    covers: d.bool()?,
                };
                Self::Request(Request { id, path, options })
            }
            KIND_RESPONSE => {
                let id = d.u64()?;
                let result = match d.u8()? {
                    0 => Ok(read_file_report(&mut d)?),
                    1 => {
                        let kind = match d.u8()? {
                            0 => FailureKind::Io,
                            1 => FailureKind::Unreadable,
                            2 => FailureKind::Internal,
                            value => {
                                return Err(DecodeError::UnknownTag {
                                    what: "failure kind",
                                    value,
                                });
                            }
                        };
                        Err(Failure {
                            kind,
                            message: d.string()?,
                        })
                    }
                    value => {
                        return Err(DecodeError::UnknownTag {
                            what: "result",
                            value,
                        });
                    }
                };
                Self::Response(Box::new(Response { id, result }))
            }
            value => {
                return Err(DecodeError::UnknownTag {
                    what: "message kind",
                    value,
                });
            }
        };
        d.finish()?;
        Ok(message)
    }
}

fn path(e: &mut Encoder, path: &Path) {
    e.bytes(path.as_os_str().as_bytes());
}

fn read_path(d: &mut Decoder<'_>) -> Result<PathBuf, DecodeError> {
    Ok(PathBuf::from(OsStr::from_bytes(d.bytes()?)))
}

fn file_report(e: &mut Encoder, r: &FileReport) {
    e.u64(r.size);
    e.bytes(&r.quick_hash);
    file_tags(e, &r.tags);
    e.opt(r.cue.as_ref(), |e, cue| {
        e.u8(match cue.origin {
            CueOrigin::External => 0,
            CueOrigin::Tag => 1,
            CueOrigin::FlacBlock => 2,
        });
        e.opt(cue.sheet.as_deref(), path);
        e.list(&cue.tracks, virtual_track);
        e.strs(&cue.warnings);
    });
    e.list(&r.chapters, virtual_track);
    e.opt(r.cover.as_ref(), cover_report);
    e.opt(r.folder_cover.as_ref(), cover_report);
    e.strs(&r.notes);
}

const EMBEDDED_TAGS: [EmbeddedTag; 7] = [
    EmbeddedTag::Id3v2,
    EmbeddedTag::Ape,
    EmbeddedTag::FlacPicture,
    EmbeddedTag::VorbisComment,
    EmbeddedTag::Mp4Cover,
    EmbeddedTag::FfmpegAttachedPicture,
    EmbeddedTag::MatroskaAttachment,
];

const PICTURE_FORMATS: [PictureFormat; 9] = [
    PictureFormat::Jpeg,
    PictureFormat::Png,
    PictureFormat::WebP,
    PictureFormat::Gif,
    PictureFormat::Bmp,
    PictureFormat::Tiff,
    PictureFormat::Avif,
    PictureFormat::Heif,
    PictureFormat::JpegXl,
];

/// The position of `value` in `table`, as a byte.
fn code_of<T: PartialEq>(table: &[T], value: &T) -> u8 {
    table
        .iter()
        .position(|t| t == value)
        .and_then(|i| u8::try_from(i).ok())
        .unwrap_or(u8::MAX)
}

/// The entry of `table` at `code`.
fn from_code<T: Copy>(table: &[T], code: u8, what: &'static str) -> Result<T, DecodeError> {
    table
        .get(usize::from(code))
        .copied()
        .ok_or(DecodeError::UnknownTag { what, value: code })
}

fn cover_report(e: &mut Encoder, c: &CoverReport) {
    e.bytes(c.hash.as_bytes());
    match &c.origin {
        CoverOrigin::Embedded {
            tag,
            picture_type,
            index,
        } => {
            e.u8(0);
            e.u8(code_of(&EMBEDDED_TAGS, tag));
            e.opt(picture_type.map(|t| t.0), Encoder::u8);
            e.u32(*index);
        }
        CoverOrigin::Folder { path: p } => {
            e.u8(1);
            path(e, p);
        }
    }
    e.u8(code_of(&PICTURE_FORMATS, &c.format));
    e.u32(c.width);
    e.u32(c.height);
    e.u64(c.byte_len);
}

fn read_cover_report(d: &mut Decoder<'_>) -> Result<CoverReport, DecodeError> {
    let hash = CoverHash::from_bytes(d.bytes()?).ok_or(DecodeError::Invalid("cover hash"))?;
    let origin = match d.u8()? {
        0 => CoverOrigin::Embedded {
            tag: from_code(&EMBEDDED_TAGS, d.u8()?, "embedded tag")?,
            picture_type: d.opt(Decoder::u8)?.map(PictureType),
            index: d.u32()?,
        },
        1 => CoverOrigin::Folder {
            path: read_path(d)?,
        },
        value => {
            return Err(DecodeError::UnknownTag {
                what: "cover origin",
                value,
            });
        }
    };
    Ok(CoverReport {
        hash,
        origin,
        format: from_code(&PICTURE_FORMATS, d.u8()?, "picture format")?,
        width: d.u32()?,
        height: d.u32()?,
        byte_len: d.u64()?,
    })
}

fn read_file_report(d: &mut Decoder<'_>) -> Result<FileReport, DecodeError> {
    Ok(FileReport {
        size: d.u64()?,
        quick_hash: d.bytes()?.to_vec(),
        tags: read_file_tags(d)?,
        cue: d.opt(|d| {
            let origin = match d.u8()? {
                0 => CueOrigin::External,
                1 => CueOrigin::Tag,
                2 => CueOrigin::FlacBlock,
                value => {
                    return Err(DecodeError::UnknownTag {
                        what: "sheet origin",
                        value,
                    });
                }
            };
            Ok(CueReport {
                origin,
                sheet: d.opt(read_path)?,
                tracks: d.list(read_virtual_track)?,
                warnings: d.strings()?,
            })
        })?,
        chapters: d.list(read_virtual_track)?,
        cover: d.opt(read_cover_report)?,
        folder_cover: d.opt(read_cover_report)?,
        notes: d.strings()?,
    })
}

fn file_tags(e: &mut Encoder, f: &FileTags) {
    track_tags(e, &f.tags);
    audio(e, &f.audio);
    e.u8(match f.source {
        TagSource::Lofty => 0,
        TagSource::Ffmpeg => 1,
    });
    e.opt_str(f.fallback_reason.as_deref());
    e.bool(f.charset_repaired);
}

fn read_file_tags(d: &mut Decoder<'_>) -> Result<FileTags, DecodeError> {
    Ok(FileTags {
        tags: read_track_tags(d)?,
        audio: read_audio(d)?,
        source: match d.u8()? {
            0 => TagSource::Lofty,
            1 => TagSource::Ffmpeg,
            value => {
                return Err(DecodeError::UnknownTag {
                    what: "tag source",
                    value,
                });
            }
        },
        fallback_reason: d.opt_string()?,
        charset_repaired: d.bool()?,
    })
}

fn audio(e: &mut Encoder, a: &AudioProperties) {
    e.opt(a.duration, |e, duration| {
        e.u64(duration.as_secs());
        e.u32(duration.subsec_nanos());
    });
    e.opt_u32(a.sample_rate);
    e.opt(a.channels, Encoder::u16);
    e.opt_u32(a.bit_depth);
    e.opt_u32(a.bitrate_kbps);
    e.opt_str(a.codec.as_deref());
    e.opt_str(a.container.as_deref());
}

fn read_audio(d: &mut Decoder<'_>) -> Result<AudioProperties, DecodeError> {
    Ok(AudioProperties {
        duration: d.opt(|d| {
            let secs = d.u64()?;
            let nanos = d.u32()?;
            if nanos >= 1_000_000_000 {
                return Err(DecodeError::Invalid("duration"));
            }
            Ok(Duration::new(secs, nanos))
        })?,
        sample_rate: d.opt_u32()?,
        channels: d.opt(Decoder::u16)?,
        bit_depth: d.opt_u32()?,
        bitrate_kbps: d.opt_u32()?,
        codec: d.opt_string()?,
        container: d.opt_string()?,
    })
}

const ROLES: [Role; 13] = [
    Role::Artist,
    Role::AlbumArtist,
    Role::Composer,
    Role::Conductor,
    Role::Lyricist,
    Role::Writer,
    Role::Arranger,
    Role::Remixer,
    Role::Producer,
    Role::Engineer,
    Role::Mixer,
    Role::DjMixer,
    Role::Performer,
];

fn role_code(role: Role) -> u8 {
    ROLES
        .iter()
        .position(|r| *r == role)
        .and_then(|i| u8::try_from(i).ok())
        .unwrap_or(u8::MAX)
}

fn track_tags(e: &mut Encoder, t: &TrackTags) {
    e.opt_str(t.title.as_deref());
    e.opt_str(t.title_sort.as_deref());
    e.opt_str(t.artist.as_deref());
    e.strs(&t.artists);
    e.opt_str(t.artist_sort.as_deref());
    e.opt_str(t.album.as_deref());
    e.opt_str(t.album_sort.as_deref());
    e.opt_str(t.album_artist.as_deref());
    e.strs(&t.album_artists);
    e.opt_str(t.album_artist_sort.as_deref());
    e.opt_str(t.composer_sort.as_deref());
    e.opt_u32(t.track_number);
    e.opt_u32(t.track_total);
    e.opt_u32(t.disc_number);
    e.opt_u32(t.disc_total);
    e.opt_str(t.disc_subtitle.as_deref());
    e.opt_str(t.date.as_deref());
    e.opt_str(t.original_date.as_deref());
    e.strs(&t.genres);
    e.list(&t.credits, |e, c| {
        e.u8(role_code(c.role));
        e.str(&c.name);
        e.opt_str(c.instrument.as_deref());
    });
    e.strs(&t.labels);
    e.strs(&t.catalog_numbers);
    e.strs(&t.release_types);
    let mb = &t.musicbrainz;
    e.opt_str(mb.recording.as_deref());
    e.opt_str(mb.release_track.as_deref());
    e.opt_str(mb.release.as_deref());
    e.opt_str(mb.release_group.as_deref());
    e.strs(&mb.artists);
    e.strs(&mb.album_artists);
    e.strs(&mb.works);
    e.opt_str(t.comment.as_deref());
    e.bool(t.has_lyrics);
    let rg = &t.replay_gain;
    for value in [
        rg.track_gain_db,
        rg.track_peak,
        rg.album_gain_db,
        rg.album_peak,
    ] {
        e.opt(value, Encoder::f64);
    }
    e.opt(rg.r128_track_gain, Encoder::i16);
    e.opt(rg.r128_album_gain, Encoder::i16);
    e.opt(t.rating.map(Rating::half_stars), Encoder::u8);
    e.opt(t.compilation, Encoder::bool);
    e.opt_str(t.grouping.as_deref());
    e.opt_str(t.work.as_deref());
    e.opt_str(t.movement.as_deref());
    e.opt_u32(t.movement_number);
    e.opt_u32(t.movement_total);
    e.list(&t.extra, |e, (key, value)| {
        e.str(key);
        e.str(value);
    });
}

fn read_track_tags(d: &mut Decoder<'_>) -> Result<TrackTags, DecodeError> {
    Ok(TrackTags {
        title: d.opt_string()?,
        title_sort: d.opt_string()?,
        artist: d.opt_string()?,
        artists: d.strings()?,
        artist_sort: d.opt_string()?,
        album: d.opt_string()?,
        album_sort: d.opt_string()?,
        album_artist: d.opt_string()?,
        album_artists: d.strings()?,
        album_artist_sort: d.opt_string()?,
        composer_sort: d.opt_string()?,
        track_number: d.opt_u32()?,
        track_total: d.opt_u32()?,
        disc_number: d.opt_u32()?,
        disc_total: d.opt_u32()?,
        disc_subtitle: d.opt_string()?,
        date: d.opt_string()?,
        original_date: d.opt_string()?,
        genres: d.strings()?,
        credits: d.list(|d| {
            let code = d.u8()?;
            let role = ROLES
                .get(usize::from(code))
                .copied()
                .ok_or(DecodeError::UnknownTag {
                    what: "role",
                    value: code,
                })?;
            Ok(Credit {
                role,
                name: d.string()?,
                instrument: d.opt_string()?,
            })
        })?,
        labels: d.strings()?,
        catalog_numbers: d.strings()?,
        release_types: d.strings()?,
        musicbrainz: MusicBrainzIds {
            recording: d.opt_string()?,
            release_track: d.opt_string()?,
            release: d.opt_string()?,
            release_group: d.opt_string()?,
            artists: d.strings()?,
            album_artists: d.strings()?,
            works: d.strings()?,
        },
        comment: d.opt_string()?,
        has_lyrics: d.bool()?,
        replay_gain: ReplayGainTags {
            track_gain_db: d.opt(Decoder::f64)?,
            track_peak: d.opt(Decoder::f64)?,
            album_gain_db: d.opt(Decoder::f64)?,
            album_peak: d.opt(Decoder::f64)?,
            r128_track_gain: d.opt(Decoder::i16)?,
            r128_album_gain: d.opt(Decoder::i16)?,
        },
        rating: d
            .opt(Decoder::u8)?
            .map(|half| Rating::from_half_stars(half).ok_or(DecodeError::Invalid("rating")))
            .transpose()?,
        compilation: d.opt(Decoder::bool)?,
        grouping: d.opt_string()?,
        work: d.opt_string()?,
        movement: d.opt_string()?,
        movement_number: d.opt_u32()?,
        movement_total: d.opt_u32()?,
        extra: d.list(|d| Ok((d.string()?, d.string()?)))?,
    })
}

fn media_time(e: &mut Encoder, t: MediaTime) {
    e.u64(t.ticks());
    e.u32(t.ticks_per_second().get());
}

fn read_media_time(d: &mut Decoder<'_>) -> Result<MediaTime, DecodeError> {
    let ticks = d.u64()?;
    let rate = NonZeroU32::new(d.u32()?).ok_or(DecodeError::Invalid("time base"))?;
    Ok(MediaTime::new(ticks, rate))
}

fn virtual_track(e: &mut Encoder, t: &VirtualTrack) {
    e.u32(t.number);
    path(e, &t.source.path);
    e.opt(t.source.section.as_ref(), |e, section| match section {
        SourceSection::Subsong(n) => {
            e.u8(0);
            e.u32(*n);
        }
        SourceSection::Span { start, end } => {
            e.u8(1);
            media_time(e, *start);
            e.opt(*end, media_time);
        }
    });
    track_tags(e, &t.tags);
    e.opt(t.pregap_start, media_time);
    e.bool(t.pre_emphasis);
}

fn read_virtual_track(d: &mut Decoder<'_>) -> Result<VirtualTrack, DecodeError> {
    let number = d.u32()?;
    let path = read_path(d)?;
    let section = d.opt(|d| match d.u8()? {
        0 => Ok(SourceSection::Subsong(d.u32()?)),
        1 => Ok(SourceSection::Span {
            start: read_media_time(d)?,
            end: d.opt(read_media_time)?,
        }),
        value => Err(DecodeError::UnknownTag {
            what: "section",
            value,
        }),
    })?;
    let source = match section {
        Some(section) => MediaSource::file(path).with_section(section),
        None => MediaSource::file(path),
    };
    Ok(VirtualTrack {
        number,
        source,
        tags: read_track_tags(d)?,
        pregap_start: d.opt(read_media_time)?,
        pre_emphasis: d.bool()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rich_tags() -> TrackTags {
        TrackTags {
            title: Some("Jóga".into()),
            artists: vec!["Björk".into(), "Other".into()],
            album_artists: vec!["Björk".into()],
            track_number: Some(3),
            genres: vec!["Pop".into()],
            credits: vec![Credit {
                role: Role::Performer,
                name: "Someone".into(),
                instrument: Some("guitar".into()),
            }],
            musicbrainz: MusicBrainzIds {
                release: Some("id".into()),
                works: vec!["w".into()],
                ..MusicBrainzIds::default()
            },
            replay_gain: ReplayGainTags {
                track_gain_db: Some(-6.5),
                r128_album_gain: Some(-256),
                ..ReplayGainTags::default()
            },
            rating: Rating::from_half_stars(7),
            compilation: Some(false),
            extra: vec![("isrc".into(), "X".into()), ("isrc".into(), "Y".into())],
            ..TrackTags::default()
        }
    }

    fn report() -> FileReport {
        let path = PathBuf::from(OsStr::from_bytes(b"/m/\xff\xfe.flac"));
        let rate = NonZeroU32::new(75).unwrap();
        FileReport {
            size: 1234,
            quick_hash: vec![1, 2, 3],
            tags: FileTags {
                tags: rich_tags(),
                audio: AudioProperties {
                    duration: Some(Duration::new(61, 500)),
                    sample_rate: Some(44_100),
                    channels: Some(2),
                    bit_depth: Some(16),
                    bitrate_kbps: Some(900),
                    codec: Some("flac".into()),
                    container: Some("flac".into()),
                },
                source: TagSource::Lofty,
                fallback_reason: None,
                charset_repaired: true,
            },
            cue: Some(CueReport {
                origin: CueOrigin::External,
                sheet: Some("/m/a.cue".into()),
                tracks: vec![VirtualTrack {
                    number: 2,
                    source: MediaSource::file(&path).with_section(SourceSection::Span {
                        start: MediaTime::new(150, rate),
                        end: None,
                    }),
                    tags: rich_tags(),
                    pregap_start: Some(MediaTime::new(100, rate)),
                    pre_emphasis: true,
                }],
                warnings: vec!["line 3: odd".into()],
            }),
            chapters: Vec::new(),
            cover: Some(CoverReport {
                hash: CoverHash::of(b"front"),
                origin: CoverOrigin::Embedded {
                    tag: EmbeddedTag::MatroskaAttachment,
                    picture_type: Some(PictureType::FRONT_COVER),
                    index: 2,
                },
                format: PictureFormat::WebP,
                width: 1200,
                height: 1199,
                byte_len: 345_678,
            }),
            folder_cover: Some(CoverReport {
                hash: CoverHash::of(b"folder"),
                origin: CoverOrigin::Folder {
                    path: PathBuf::from(OsStr::from_bytes(b"/m/\xe9/cover.jpg")),
                },
                format: PictureFormat::Jpeg,
                width: 500,
                height: 500,
                byte_len: 40_000,
            }),
            notes: vec!["note".into()],
        }
    }

    #[test]
    fn every_message_round_trips() {
        let messages = [
            Message::Hello(Hello::current()),
            Message::Request(Request {
                id: 42,
                path: PathBuf::from(OsStr::from_bytes(b"/music/caf\xe9.mp3")),
                options: RequestOptions {
                    legacy_charset: Some("windows-1251".into()),
                    cue_sheets: vec!["/music/a.cue".into()],
                    ..RequestOptions::default()
                },
            }),
            Message::Response(Box::new(Response {
                id: 42,
                result: Ok(report()),
            })),
            Message::Response(Box::new(Response {
                id: 43,
                result: Err(Failure {
                    kind: FailureKind::Unreadable,
                    message: "no".into(),
                }),
            })),
        ];
        for message in messages {
            let bytes = message.encode();
            assert_eq!(Message::decode(&bytes).unwrap(), message);
        }
    }

    #[test]
    fn truncated_or_damaged_messages_are_errors_not_panics() {
        let bytes = Message::Response(Box::new(Response {
            id: 1,
            result: Ok(report()),
        }))
        .encode();
        for cut in 0..bytes.len() {
            assert!(Message::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(Message::decode(&longer), Err(DecodeError::TrailingBytes(1)));
        // Flipping any single byte must never panic.
        for i in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[i] ^= 0xff;
            let _ = Message::decode(&damaged);
        }
    }

    #[test]
    fn every_cover_code_round_trips() {
        for tag in EMBEDDED_TAGS {
            for format in PICTURE_FORMATS {
                let mut r = report();
                r.cover = r.cover.map(|c| CoverReport {
                    origin: CoverOrigin::Embedded {
                        tag,
                        picture_type: None,
                        index: 0,
                    },
                    format,
                    ..c
                });
                r.folder_cover = None;
                let message = Message::Response(Box::new(Response {
                    id: 9,
                    result: Ok(r),
                }));
                assert_eq!(Message::decode(&message.encode()).unwrap(), message);
            }
        }
    }

    #[test]
    fn a_short_cover_hash_is_refused() {
        let mut e = Encoder::default();
        e.bytes(&[1; 31]);
        e.u8(1);
        e.bytes(b"/x");
        e.u8(0);
        e.u32(1);
        e.u32(1);
        e.u64(1);
        let bytes = e.finish();
        assert_eq!(
            read_cover_report(&mut Decoder::new(&bytes)),
            Err(DecodeError::Invalid("cover hash"))
        );
    }

    #[test]
    fn a_version_1_hello_is_reported_as_such() {
        let mut bytes = Message::Hello(Hello::current()).encode();
        bytes[5] = 1;
        bytes[6] = 0;
        match Message::decode(&bytes).unwrap() {
            Message::Hello(hello) => {
                assert_eq!(hello.version, 1);
                assert_ne!(hello.version, PROTOCOL_VERSION);
            }
            other => panic!("not a hello: {other:?}"),
        }
    }

    #[test]
    fn a_hello_of_another_version_is_recognised() {
        let mut bytes = Message::Hello(Hello::current()).encode();
        bytes[5] = 99;
        bytes[6] = 0;
        match Message::decode(&bytes).unwrap() {
            Message::Hello(hello) => assert_eq!(hello.version, 99),
            other => panic!("not a hello: {other:?}"),
        }
        assert!(Message::decode(b"\x01NOPE\x01\x00\x00\x00\x00\x00").is_err());
    }
}
