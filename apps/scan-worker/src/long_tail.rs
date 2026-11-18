// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Files of the long tail (tracker modules, game music, SID tunes, MIDI files and,
//! with vgmstream, game audio streams), which neither the tag library nor FFmpeg
//! reads: their text and subsongs come from their own decoders.
//!
//! A file is taken here when one of the decoders recognises its first bytes (or,
//! for vgmstream, its name). Each subsong of a file with several becomes a track of
//! its own ([`FileReport::subsongs`]): its title is the subsong's own, or the
//! file's title with its number; the album is the game, or the file's title. The
//! lengths are those of the default settings (two loops, an 8 s fade, 150 s for a
//! tune whose length nothing states); playback uses the user's settings.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use helikodu_extra_decoders::song::{SongError, SongInfo, SongOptions};
use helikodu_meta::tags::{AudioProperties, FileTags, TagSource, TrackTags};
use helikodu_meta::worker::{Failure, FailureKind, FileReport, Request, SubsongReport, quick_hash};

/// The largest file read here; such files are kilobytes, a few megabytes at most.
const MAX_FILE: u64 = 64 << 20;

/// The decoder that recognises a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    #[cfg(feature = "openmpt")]
    OpenMpt,
    #[cfg(feature = "gme")]
    Gme,
    #[cfg(feature = "sid")]
    Sid,
    #[cfg(feature = "midi")]
    Midi,
    #[cfg(feature = "vgmstream")]
    Vgmstream,
}

impl Kind {
    /// The decoder whose signature `header` carries, or that takes `path`.
    #[allow(unused_variables)]
    fn of(header: &[u8], path: &Path) -> Option<Self> {
        #[cfg(feature = "sid")]
        if helikodu_extra_decoders::sid::probe(header) {
            return Some(Self::Sid);
        }
        #[cfg(feature = "midi")]
        if helikodu_extra_decoders::midi::probe(header) {
            return Some(Self::Midi);
        }
        #[cfg(feature = "gme")]
        if helikodu_extra_decoders::gme::probe(header) {
            return Some(Self::Gme);
        }
        #[cfg(feature = "openmpt")]
        if helikodu_extra_decoders::openmpt::probe(header) {
            return Some(Self::OpenMpt);
        }
        #[cfg(feature = "vgmstream")]
        if helikodu_extra_decoders::vgmstream::probe(path) {
            return Some(Self::Vgmstream);
        }
        None
    }

    /// The codec name stored with the tracks: the backend's name in the engine.
    fn codec(self) -> &'static str {
        match self {
            #[cfg(feature = "openmpt")]
            Self::OpenMpt => "openmpt",
            #[cfg(feature = "gme")]
            Self::Gme => "gme",
            #[cfg(feature = "sid")]
            Self::Sid => "sidplayfp",
            #[cfg(feature = "midi")]
            Self::Midi => "rustysynth",
            #[cfg(feature = "vgmstream")]
            Self::Vgmstream => "vgmstream",
        }
    }

    /// What the file says about itself and its subsongs.
    #[allow(unused_variables)]
    fn info(self, data: &[u8], path: &Path) -> Result<SongInfo, SongError> {
        let options = SongOptions::default();
        use helikodu_extra_decoders::song::SongDecoder as _;
        Ok(match self {
            #[cfg(feature = "openmpt")]
            Self::OpenMpt => {
                helikodu_extra_decoders::openmpt::OpenMptDecoder::open(data, None, &options)?
                    .info()
                    .clone()
            }
            #[cfg(feature = "gme")]
            Self::Gme => helikodu_extra_decoders::gme::GmeDecoder::open(data, None, &options)?
                .info()
                .clone(),
            #[cfg(feature = "sid")]
            Self::Sid => helikodu_extra_decoders::sid::SidDecoder::open(data, None, &options)?
                .info()
                .clone(),
            #[cfg(feature = "midi")]
            Self::Midi => helikodu_extra_decoders::midi::read_info(data)?,
            #[cfg(feature = "vgmstream")]
            Self::Vgmstream => {
                helikodu_extra_decoders::vgmstream::VgmstreamDecoder::open(path, None, &options)?
                    .info()
                    .clone()
            }
        })
    }
}

fn io_failure(error: &io::Error) -> Failure {
    Failure {
        kind: FailureKind::Io,
        message: error.to_string(),
    }
}

/// Reads the file of `request` when it is a long-tail format; `None` when it is not
/// one, for the usual readers.
pub(crate) fn read(request: &Request) -> Option<Result<FileReport, Failure>> {
    let path = request.path.as_path();
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) => return Some(Err(io_failure(&e))),
    };
    let mut header = Vec::with_capacity(4096);
    if let Err(e) = (&mut file).take(4096).read_to_end(&mut header) {
        return Some(Err(io_failure(&e)));
    }
    let kind = Kind::of(&header, path)?;
    match report(kind, &mut file, path) {
        // The signature was a coincidence (a gzip file that is not VGZ, say): the
        // usual readers may know the file.
        Err(failure) if failure.kind == FailureKind::Unreadable => None,
        other => Some(other),
    }
}

fn report(kind: Kind, file: &mut File, path: &Path) -> Result<FileReport, Failure> {
    let size = file.metadata().map_err(|e| io_failure(&e))?.len();
    if size > MAX_FILE {
        return Err(Failure {
            kind: FailureKind::Unreadable,
            message: "the file is too large for its format".into(),
        });
    }
    let quick_hash = quick_hash(file, size).map_err(|e| io_failure(&e))?;
    file.seek(SeekFrom::Start(0)).map_err(|e| io_failure(&e))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).map_err(|e| io_failure(&e))?;
    let info = panic::catch_unwind(AssertUnwindSafe(|| kind.info(&data, path)))
        .map_err(|_| Failure {
            kind: FailureKind::Internal,
            message: "the decoder failed".into(),
        })?
        .map_err(|e| Failure {
            kind: match e {
                SongError::Io(_) => FailureKind::Io,
                _ => FailureKind::Unreadable,
            },
            message: e.to_string(),
        })?;
    let mut report = build(kind, &info);
    report.size = size;
    report.quick_hash = quick_hash;
    Ok(report)
}

/// The report of a file whose decoder said `info`.
fn build(kind: Kind, info: &SongInfo) -> FileReport {
    let options = SongOptions::default();
    let count = info.subsong_count();
    let album = info.album.clone().or_else(|| {
        // A file with several subsongs is an album of them.
        (count > 1).then(|| info.title.clone()).flatten()
    });
    let file_tags = TrackTags {
        title: info.title.clone(),
        artist: info.artist.clone(),
        album: album.clone(),
        album_artist: (count > 1).then(|| info.artist.clone()).flatten(),
        date: info.date.clone(),
        comment: info.comment.clone(),
        ..TrackTags::default()
    };
    let default = info
        .subsongs
        .get(info.default_subsong as usize)
        .or(info.subsongs.first());
    let subsongs = if count > 1 {
        info.subsongs
            .iter()
            .map(|subsong| {
                let number = subsong.index + 1;
                let title = subsong.title.clone().or_else(|| {
                    album
                        .as_ref()
                        .or(info.title.as_ref())
                        .map(|name| format!("{name} ({number})"))
                });
                SubsongReport {
                    index: subsong.index,
                    tags: TrackTags {
                        title,
                        track_number: Some(number),
                        track_total: Some(count),
                        ..file_tags.clone()
                    },
                    duration: subsong.length,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    FileReport {
        size: 0,
        quick_hash: Vec::new(),
        tags: FileTags {
            tags: file_tags,
            audio: AudioProperties {
                duration: default.and_then(|s| s.length),
                sample_rate: Some(options.rate),
                channels: Some(2),
                bit_depth: None,
                bitrate_kbps: None,
                codec: Some(kind.codec().to_owned()),
                container: Some(info.format.clone()),
            },
            source: TagSource::Decoder,
            fallback_reason: None,
            charset_repaired: false,
        },
        cue: None,
        chapters: Vec::new(),
        cover: None,
        folder_cover: None,
        notes: Vec::new(),
        subsongs,
    }
}
