// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading external sheets from disk.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use encoding_rs::Encoding;

use crate::tags::TagError;
use crate::tags::charset::encoding_for_label;

use super::parse::{MAX_SHEET_BYTES, parse_text};
use super::path::{PathRejection, relative_path};
use super::resolve::{Located, VirtualTrack, file_of, lexical_locator, resolve_with};
use super::sheet::CueSheet;
use super::text::{SheetEncoding, decode};
use super::warning::{CueWarning, CueWarningKind as Kind};

/// How sheets are read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CueOptions {
    /// The legacy charset the user chose for sheets that are neither UTF-8 nor
    /// marked by a byte order mark, as a WHATWG label such as `windows-1251` or
    /// `shift_jis`. When unset, the charset is guessed.
    pub legacy_charset: Option<String>,
}

impl CueOptions {
    fn chosen_charset(&self) -> Result<Option<&'static Encoding>, CueError> {
        match &self.legacy_charset {
            None => Ok(None),
            Some(label) => encoding_for_label(label)
                .map(Some)
                .ok_or_else(|| CueError::UnknownCharset(label.clone())),
        }
    }
}

/// Why a sheet, an embedded sheet or a file's chapters could not be read.
#[derive(Debug, thiserror::Error)]
pub enum CueError {
    /// The file could not be opened or read.
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),
    /// The sheet is larger than the parser accepts.
    #[error("the sheet is larger than {limit} bytes")]
    TooLarge {
        /// The largest size accepted, in bytes.
        limit: usize,
    },
    /// [`CueOptions::legacy_charset`] names no known charset.
    #[error("unknown charset {0:?}")]
    UnknownCharset(String),
    /// The tags of the audio file could not be read.
    #[error(transparent)]
    Tags(#[from] TagError),
    /// FFmpeg could not read the audio file.
    #[error("FFmpeg cannot read this file: {0}")]
    Ffmpeg(#[from] helikodu_ffmpeg_bridge::Error),
}

/// Where virtual tracks came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueOrigin {
    /// A `.cue` file beside the audio.
    External,
    /// A `CUESHEET` tag in the audio file (Vorbis comment, APEv2 item).
    Tag,
    /// The binary `CUESHEET` metadata block of a FLAC file.
    FlacBlock,
}

/// A sheet's virtual tracks, with everything that was skipped or repaired.
#[derive(Debug, Clone, PartialEq)]
pub struct CueTracks {
    /// Where the tracks came from.
    pub origin: CueOrigin,
    /// The parsed sheet; `None` for a FLAC cue sheet block, which is not text.
    pub sheet: Option<CueSheet>,
    /// The virtual tracks, in sheet order.
    pub tracks: Vec<VirtualTrack>,
    /// The parser's warnings followed by those of the resolution.
    pub warnings: Vec<CueWarning>,
}

/// The virtual tracks of a sheet and the resolution's warnings.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// The virtual tracks, in sheet order.
    pub tracks: Vec<VirtualTrack>,
    /// What the resolution left out, such as rejected paths.
    pub warnings: Vec<CueWarning>,
}

impl CueSheet {
    /// Parses a sheet's bytes. Fails only when the sheet is larger than 1 MiB or
    /// the chosen charset is unknown; anything else is reported in
    /// [`warnings`](Self::warnings).
    pub fn parse(bytes: &[u8], options: &CueOptions) -> Result<Self, CueError> {
        let chosen = options.chosen_charset()?;
        if bytes.len() > MAX_SHEET_BYTES {
            return Err(CueError::TooLarge {
                limit: MAX_SHEET_BYTES,
            });
        }
        let (text, encoding) = decode(bytes, chosen);
        Ok(parse_text(&text, encoding))
    }

    /// Parses a sheet that is already text, such as the value of a `CUESHEET` tag.
    pub fn parse_text(text: &str) -> Result<Self, CueError> {
        if text.len() > MAX_SHEET_BYTES {
            return Err(CueError::TooLarge {
                limit: MAX_SHEET_BYTES,
            });
        }
        Ok(parse_text(text, SheetEncoding::tag()))
    }

    /// The virtual tracks of the sheet, with the `FILE` paths taken relative to
    /// `folder` (the sheet's folder). Paths are checked by their names only: the
    /// disk is not touched, and files that do not exist are not noticed.
    pub fn resolve(&self, folder: &Path) -> Resolution {
        let (tracks, warnings) = resolve_with(self, lexical_locator(self, folder));
        Resolution { tracks, warnings }
    }
}

/// Reads the sheet at `path` and resolves its virtual tracks against the files
/// beside it.
///
/// A file that is not found under its own name is looked for under the name's
/// bytes in the sheet's own charset, then in another case, then with another audio
/// extension (a sheet written for `album.wav` beside `album.flac`). Tracks of a
/// file that is not found at all are kept, with a warning.
pub fn read_cue(path: impl AsRef<Path>, options: &CueOptions) -> Result<CueTracks, CueError> {
    let path = path.as_ref();
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_SHEET_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let sheet = CueSheet::parse(&bytes, options)?;
    let folder = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let (tracks, resolve_warnings) = resolve_with(&sheet, disk_locator(&sheet, folder));
    let mut warnings = sheet.warnings.clone();
    warnings.extend(resolve_warnings);
    Ok(CueTracks {
        origin: CueOrigin::External,
        sheet: Some(sheet),
        tracks,
        warnings,
    })
}

/// Audio extensions tried, in this order, when a sheet's file is missing.
const AUDIO_EXTENSIONS: [&str; 16] = [
    "flac", "wv", "ape", "tak", "tta", "wav", "aiff", "aif", "m4a", "ogg", "opus", "mp3", "mpc",
    "wma", "ofr", "w64",
];

/// The most folder entries looked at when searching for a substitute file.
const MAX_FOLDER_ENTRIES: usize = 10_000;

fn disk_locator<'a>(
    sheet: &'a CueSheet,
    folder: &'a Path,
) -> impl FnMut(Option<usize>) -> Located + 'a {
    move |file| {
        let file = file_of(sheet, file)?;
        let rejected = |reason: PathRejection| Kind::RejectedPath {
            name: file.name.clone(),
            reason,
        };
        let primary = folder.join(relative_path(file.name.as_bytes()).map_err(rejected)?);
        if is_file(&primary) {
            return Ok((primary, None));
        }
        if file.raw_name != file.name.as_bytes()
            && let Ok(raw) = relative_path(&file.raw_name)
        {
            let raw = folder.join(raw);
            if is_file(&raw) {
                return Ok((raw, None));
            }
        }
        if let Some(found) = substitute(&primary) {
            let found_name = found
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Ok((
                found,
                Some(Kind::FileSubstituted {
                    name: file.name.clone(),
                    found: found_name,
                }),
            ));
        }
        Ok((primary, Some(Kind::FileNotFound(file.name.clone()))))
    }
}

fn is_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// A file in the folder of `wanted` with its name in another case, or with its
/// stem and another audio extension.
fn substitute(wanted: &Path) -> Option<PathBuf> {
    let folder = wanted.parent()?;
    let name = wanted.file_name()?.to_string_lossy().to_lowercase();
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_owned(),
        _ => name.clone(),
    };
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in fs::read_dir(folder).ok()?.take(MAX_FOLDER_ENTRIES) {
        let Ok(entry) = entry else { continue };
        let candidate = entry.file_name().to_string_lossy().to_lowercase();
        let rank = if candidate == name {
            0
        } else {
            match candidate.rsplit_once('.') {
                Some((s, ext)) if s == stem => {
                    match AUDIO_EXTENSIONS.iter().position(|e| *e == ext) {
                        Some(p) => p + 1,
                        None => continue,
                    }
                }
                _ => continue,
            }
        };
        let path = entry.path();
        if best.as_ref().is_some_and(|(r, _)| *r <= rank) || !is_file(&path) {
            continue;
        }
        best = Some((rank, path));
    }
    best.map(|(_, path)| path)
}
