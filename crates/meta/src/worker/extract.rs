// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a worker does with one request: read the file's tags, quick hash, CUE sheet,
//! chapters and covers.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::ffi::OsStrExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use super::covers::{embedded_cover, folder_cover};
use super::message::{CueReport, Failure, FailureKind, FileReport, Request, Response};
use super::quick_hash::quick_hash;
use crate::covers::CoverLimits;
use crate::cue::{CueOptions, CueOrigin, read_chapters, read_cue, read_embedded_cue};
use crate::tags::{ReadOptions, TagError, read_tags_from_file};

/// Values of [`TrackTags::extra`](crate::tags::TrackTags::extra) longer than this are
/// not sent to the scanner (an embedded sheet, a long description); they would only
/// bloat the response.
pub const MAX_EXTRA_VALUE_BYTES: usize = 16 << 10;

/// Containers whose chapters are read, by file extension. Ogg files are added when
/// their tags hold `CHAPTERxxx` entries.
const CHAPTER_EXTENSIONS: [&[u8]; 8] = [
    b"m4b", b"m4a", b"mp4", b"m4v", b"mka", b"mkv", b"webm", b"mov",
];

/// Answers `request`. A panic inside a reader is caught and answered as a
/// [`FailureKind::Internal`] failure, so the worker lives on.
pub fn handle(request: &Request) -> Response {
    let result =
        panic::catch_unwind(AssertUnwindSafe(|| read_file(request))).unwrap_or_else(|payload| {
            let what = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown".to_owned());
            Err(Failure {
                kind: FailureKind::Internal,
                message: format!("the reader failed: {what}"),
            })
        });
    Response {
        id: request.id,
        result,
    }
}

/// Reads the file of `request`.
pub fn read_file(request: &Request) -> Result<FileReport, Failure> {
    let path = request.path.as_path();
    let options = &request.options;
    let mut file = File::open(path).map_err(io_failure)?;
    let size = file.metadata().map_err(io_failure)?.len();
    let quick_hash = quick_hash(&mut file, size).map_err(io_failure)?;
    file.seek(SeekFrom::Start(0)).map_err(io_failure)?;

    let read_options = ReadOptions {
        repair_charsets: options.repair_charsets,
        legacy_charset: options.legacy_charset.clone(),
    };
    let mut tags = read_tags_from_file(file, &read_options).map_err(|e| match e {
        TagError::Io(e) => io_failure(e),
        other => Failure {
            kind: FailureKind::Unreadable,
            message: other.to_string(),
        },
    })?;
    let has_cuesheet_tag = tags.tags.extra_values("cuesheet").next().is_some();
    let has_chapter_tags = tags
        .tags
        .extra
        .iter()
        .any(|(key, _)| key.len() > 7 && key.as_bytes()[..7].eq_ignore_ascii_case(b"chapter"));
    tags.tags
        .extra
        .retain(|(_, value)| value.len() <= MAX_EXTRA_VALUE_BYTES);

    let mut notes = Vec::new();
    let cue_options = CueOptions {
        legacy_charset: options.legacy_charset.clone(),
    };
    let mut cue = external_cue(path, &options.cue_sheets, &cue_options, &mut notes);
    if cue.is_none()
        && options.embedded_cue
        && (has_cuesheet_tag || has_flac_cuesheet_block(path).unwrap_or(false))
    {
        match read_embedded_cue(path, &cue_options) {
            Ok(Some(found)) if !found.tracks.is_empty() => {
                cue = Some(CueReport {
                    origin: found.origin,
                    sheet: None,
                    tracks: found.tracks,
                    warnings: found.warnings.iter().map(ToString::to_string).collect(),
                });
            }
            Ok(_) => {}
            Err(e) => notes.push(format!("embedded CUE sheet: {e}")),
        }
    }

    let mut chapters = Vec::new();
    if options.chapters && (has_chapter_tags || has_extension(path, &CHAPTER_EXTENSIONS)) {
        match read_chapters(path) {
            // One chapter is the whole file, not a split.
            Ok(found) if found.len() > 1 => chapters = found,
            Ok(_) => {}
            Err(e) => notes.push(format!("chapters: {e}")),
        }
    }

    let (cover, folder_cover) = if options.covers {
        let limits = CoverLimits::default();
        (
            embedded_cover(path, &limits, &mut notes),
            folder_cover(path, tags.tags.album.as_deref(), &limits, &mut notes),
        )
    } else {
        (None, None)
    };

    Ok(FileReport {
        size,
        quick_hash,
        tags,
        cue,
        chapters,
        cover,
        folder_cover,
        notes,
        subsongs: Vec::new(),
    })
}

/// The tracks in `path` of the first sheet in `sheets` that has any.
fn external_cue(
    path: &Path,
    sheets: &[std::path::PathBuf],
    options: &CueOptions,
    notes: &mut Vec<String>,
) -> Option<CueReport> {
    for sheet in sheets {
        match read_cue(sheet, options) {
            Ok(found) => {
                let tracks: Vec<_> = found
                    .tracks
                    .into_iter()
                    .filter(|track| track.source.path == path)
                    .collect();
                if !tracks.is_empty() {
                    return Some(CueReport {
                        origin: CueOrigin::External,
                        sheet: Some(sheet.clone()),
                        tracks,
                        warnings: found.warnings.iter().map(ToString::to_string).collect(),
                    });
                }
            }
            Err(e) => notes.push(format!("{}: {e}", sheet.display())),
        }
    }
    None
}

fn io_failure(error: io::Error) -> Failure {
    Failure {
        kind: FailureKind::Io,
        message: format!("cannot read the file: {error}"),
    }
}

fn has_extension(path: &Path, extensions: &[&[u8]]) -> bool {
    path.extension().is_some_and(|ext| {
        let ext = ext.as_bytes();
        extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
    })
}

/// The most metadata blocks looked at before giving up.
const MAX_FLAC_BLOCKS: usize = 1024;

/// Whether the FLAC file at `path` has a `CUESHEET` metadata block. Reads only the
/// block headers, so it is cheap enough to run on every FLAC file; a file that is
/// not FLAC has none.
fn has_flac_cuesheet_block(path: &Path) -> io::Result<bool> {
    const CUESHEET: u8 = 5;
    let mut file = File::open(path)?;
    let mut head = [0_u8; 10];
    file.read_exact(&mut head[..4])?;
    if &head[..3] == b"ID3" {
        // A stray ID3v2 tag before the stream: skip it.
        file.read_exact(&mut head[4..10])?;
        let size = head[6..10]
            .iter()
            .fold(0_u64, |acc, b| (acc << 7) | u64::from(b & 0x7f));
        let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
        file.seek(SeekFrom::Start(10 + size + footer))?;
        file.read_exact(&mut head[..4])?;
    }
    if &head[..4] != b"fLaC" {
        return Ok(false);
    }
    for _ in 0..MAX_FLAC_BLOCKS {
        let mut header = [0_u8; 4];
        file.read_exact(&mut header)?;
        if header[0] & 0x7f == CUESHEET {
            return Ok(true);
        }
        if header[0] & 0x80 != 0 {
            return Ok(false);
        }
        let length = u32::from_be_bytes([0, header[1], header[2], header[3]]);
        file.seek(SeekFrom::Current(i64::from(length)))?;
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hk-extract-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        File::create(&path).unwrap().write_all(bytes).unwrap();
        path
    }

    fn block(kind: u8, last: bool, body: &[u8]) -> Vec<u8> {
        let len = u32::try_from(body.len()).unwrap().to_be_bytes();
        let mut out = vec![kind | if last { 0x80 } else { 0 }, len[1], len[2], len[3]];
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn finds_the_cuesheet_block_by_headers() {
        let mut with = b"fLaC".to_vec();
        with.extend(block(0, false, &[0; 34]));
        with.extend(block(4, false, &[1; 20]));
        with.extend(block(5, true, &[2; 10]));
        assert!(has_flac_cuesheet_block(&write("with.flac", &with)).unwrap());

        let mut without = b"fLaC".to_vec();
        without.extend(block(0, false, &[0; 34]));
        without.extend(block(4, true, &[1; 20]));
        without.extend_from_slice(&[0xff; 100]);
        assert!(!has_flac_cuesheet_block(&write("without.flac", &without)).unwrap());

        let mut id3 = b"ID3\x04\x00\x00\x00\x00\x00\x05hello".to_vec();
        id3.extend_from_slice(&with);
        assert!(has_flac_cuesheet_block(&write("id3.flac", &id3)).unwrap());

        assert!(!has_flac_cuesheet_block(&write("x.mp3", b"\xff\xfb\x90\x00")).unwrap());
        assert!(has_flac_cuesheet_block(&write("short.flac", b"fLaC\x00")).is_err());
    }

    #[test]
    fn a_missing_file_is_an_io_failure() {
        let response = handle(&Request {
            id: 5,
            path: "/nonexistent/helikodu/a.flac".into(),
            options: Default::default(),
        });
        assert_eq!(response.id, 5);
        assert_eq!(response.result.unwrap_err().kind, FailureKind::Io);
    }

    #[test]
    fn a_file_no_reader_knows_is_unreadable() {
        let path = write("noise.flac", &[0x5a; 4096]);
        let failure = read_file(&Request {
            id: 1,
            path,
            options: Default::default(),
        })
        .unwrap_err();
        assert_eq!(failure.kind, FailureKind::Unreadable, "{failure}");
    }
}
