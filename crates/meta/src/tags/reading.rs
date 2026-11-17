// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading tags from a file: the tag library first, FFmpeg as the fallback.

mod ffmpeg_fallback;
mod lofty_files;
mod text;

use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::Path;

use encoding_rs::Encoding;

use super::charset::encoding_for_label;
use super::record::{FileTags, TagSource};

/// How tags are read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOptions {
    /// Whether text in legacy charsets and mojibake is repaired. On by default.
    pub repair_charsets: bool,
    /// The legacy charset the user chose for old tags, as a WHATWG label such as
    /// `windows-1251`, `shift_jis` or `gbk`. When unset, the charset is guessed.
    pub legacy_charset: Option<String>,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            repair_charsets: true,
            legacy_charset: None,
        }
    }
}

impl ReadOptions {
    fn preferred_charset(&self) -> Result<Option<&'static Encoding>, TagError> {
        match &self.legacy_charset {
            None => Ok(None),
            Some(label) => encoding_for_label(label)
                .map(Some)
                .ok_or_else(|| TagError::UnknownCharset(label.clone())),
        }
    }
}

/// Why tags could not be read.
#[derive(Debug, thiserror::Error)]
pub enum TagError {
    /// The file could not be opened or read.
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),
    /// The tag library does not know the format.
    #[error("the tag library does not support this format")]
    UnsupportedFormat,
    /// The tag library knows the format but could not parse the file.
    #[error("the tag library cannot parse this file: {0}")]
    Parse(String),
    /// FFmpeg could not read the file.
    #[error("FFmpeg cannot read this file: {0}")]
    Ffmpeg(#[from] helikodu_ffmpeg_bridge::Error),
    /// [`ReadOptions::legacy_charset`] names no known charset.
    #[error("unknown charset {0:?}")]
    UnknownCharset(String),
}

/// A source of tags for one file.
pub trait TagReader {
    /// Reads the tags and audio properties of an open file, starting from any offset.
    fn read_file(&self, file: File, options: &ReadOptions) -> Result<FileTags, TagError>;
}

/// Reads through the pure-Rust tag library (lofty). Its tags can be written back.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoftyReader;

impl TagReader for LoftyReader {
    fn read_file(&self, file: File, options: &ReadOptions) -> Result<FileTags, TagError> {
        let preferred = options.preferred_charset()?;
        let mut fixer = text::TextFixer::new(options.repair_charsets, preferred);
        let (tags, audio) = lofty_files::read(file, &mut fixer)?;
        Ok(FileTags {
            tags,
            audio,
            source: TagSource::Lofty,
            fallback_reason: None,
            charset_repaired: fixer.repaired(),
        })
    }
}

/// Reads through FFmpeg: read-only, for formats the tag library lacks.
#[derive(Debug, Clone, Copy, Default)]
pub struct FfmpegReader;

impl TagReader for FfmpegReader {
    fn read_file(&self, file: File, options: &ReadOptions) -> Result<FileTags, TagError> {
        let preferred = options.preferred_charset()?;
        let mut fixer = text::TextFixer::new(options.repair_charsets, preferred);
        let (tags, audio) = ffmpeg_fallback::read(file, &mut fixer)?;
        Ok(FileTags {
            tags,
            audio,
            source: TagSource::Ffmpeg,
            fallback_reason: None,
            charset_repaired: fixer.repaired(),
        })
    }
}

/// Reads the tags and audio properties of the file at `path`.
///
/// The tag library reads the file first. When it does not know the format
/// (Matroska, ASF/WMA, DSF, DFF, TAK, TTA, …) or cannot parse the file, FFmpeg
/// reads it instead, and [`FileTags::fallback_reason`] says why.
pub fn read_tags(path: impl AsRef<Path>, options: &ReadOptions) -> Result<FileTags, TagError> {
    read_tags_from_file(File::open(path)?, options)
}

/// Reads the tags of an open file, like [`read_tags`].
pub fn read_tags_from_file(file: File, options: &ReadOptions) -> Result<FileTags, TagError> {
    let mut second = file.try_clone()?;
    let reason = match LoftyReader.read_file(file, options) {
        Ok(tags) => return Ok(tags),
        Err(e @ TagError::UnknownCharset(_)) => return Err(e),
        // A read error inside the tag library (a truncated header, say) is worth
        // a second opinion too.
        Err(e) => e.to_string(),
    };
    second.seek(SeekFrom::Start(0))?;
    let mut tags = FfmpegReader.read_file(second, options)?;
    tags.fallback_reason = Some(reason);
    Ok(tags)
}
