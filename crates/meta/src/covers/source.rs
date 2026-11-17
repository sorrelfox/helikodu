// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a cover came from, and its bytes, loaded when first needed.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::decode::{PictureInfo, probe_picture};
use super::error::CoverError;
use super::format::{PictureFormat, SNIFF_LEN};
use super::hash::CoverHash;
use super::limits::CoverLimits;
use super::picture_type::PictureType;

/// The tag or container structure an embedded picture was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddedTag {
    /// An ID3v2 `APIC` (or ID3v2.2 `PIC`) frame.
    Id3v2,
    /// An APEv2 `Cover Art (…)` binary item.
    Ape,
    /// A FLAC `PICTURE` metadata block.
    FlacPicture,
    /// A Vorbis comment `METADATA_BLOCK_PICTURE` (Ogg Vorbis, Opus, Speex, FLAC).
    VorbisComment,
    /// An MP4 `covr` atom.
    Mp4Cover,
    /// An attached-picture stream as FFmpeg reports it, for formats the tag
    /// library does not read (Matroska, ASF/WMA, …) or files it cannot parse.
    FfmpegAttachedPicture,
    /// A Matroska image attachment, through FFmpeg.
    MatroskaAttachment,
}

/// Where a cover was found.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CoverSource {
    /// A picture embedded in an audio file.
    Embedded {
        /// The tag or structure that holds it.
        tag: EmbeddedTag,
        /// Its picture type, when the tag has one (MP4 `covr` has none).
        picture_type: Option<PictureType>,
        /// Its position among all the pictures of the file, counting skipped
        /// ones, in the order they were read.
        index: usize,
        /// Its description, or a Matroska attachment's file name.
        description: Option<String>,
    },
    /// An image file in the album folder.
    Folder {
        /// The file's path, with its bytes as the file system gave them.
        path: PathBuf,
    },
}

impl CoverSource {
    /// Whether the picture is embedded in an audio file.
    pub fn is_embedded(&self) -> bool {
        matches!(self, Self::Embedded { .. })
    }

    /// The picture type of an embedded picture.
    pub fn picture_type(&self) -> Option<PictureType> {
        match self {
            Self::Embedded { picture_type, .. } => *picture_type,
            Self::Folder { .. } => None,
        }
    }

    /// The path of a folder image.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Embedded { .. } => None,
            Self::Folder { path } => Some(path),
        }
    }
}

/// One cover: where it came from, and its encoded bytes.
///
/// An embedded picture's bytes are in memory from the start, because the tag
/// had to be parsed to find it. A folder image is read only when its bytes, its
/// format or its hash are first asked for, at most
/// [`CoverLimits::max_bytes`] of them; its format alone needs only the first
/// [`SNIFF_LEN`] bytes.
#[derive(Clone)]
pub struct Cover {
    source: CoverSource,
    byte_len: u64,
    max_bytes: usize,
    data: OnceLock<Vec<u8>>,
}

impl Cover {
    pub(crate) fn embedded(source: CoverSource, data: Vec<u8>) -> Self {
        let byte_len = data.len() as u64;
        Self {
            source,
            byte_len,
            max_bytes: data.len(),
            data: OnceLock::from(data),
        }
    }

    pub(crate) fn folder(path: PathBuf, byte_len: u64, limits: &CoverLimits) -> Self {
        Self {
            source: CoverSource::Folder { path },
            byte_len,
            max_bytes: limits.max_bytes,
            data: OnceLock::new(),
        }
    }

    /// Where the cover was found.
    pub fn source(&self) -> &CoverSource {
        &self.source
    }

    /// The size of the encoded picture in bytes; for a folder image not yet
    /// loaded, the file's size when it was found.
    pub fn byte_len(&self) -> u64 {
        self.data.get().map_or(self.byte_len, |d| d.len() as u64)
    }

    /// Whether the bytes are in memory.
    pub fn is_loaded(&self) -> bool {
        self.data.get().is_some()
    }

    /// The encoded picture, read from its file on first use.
    pub fn bytes(&self) -> Result<&[u8], CoverError> {
        if let Some(data) = self.data.get() {
            return Ok(data);
        }
        let loaded = self.load()?;
        Ok(self.data.get_or_init(|| loaded))
    }

    /// The encoded picture, consuming the cover.
    pub fn into_bytes(self) -> Result<Vec<u8>, CoverError> {
        match self.data.into_inner() {
            Some(data) => Ok(data),
            None => Self::read_file(&self.source, self.max_bytes),
        }
    }

    /// The picture's format, sniffed from its leading bytes. A folder image not
    /// yet loaded has only those bytes read.
    ///
    /// [`CoverError::UnknownFormat`] when the bytes are no picture known here.
    pub fn format(&self) -> Result<PictureFormat, CoverError> {
        let sniffed = match (self.data.get(), &self.source) {
            (Some(data), _) => PictureFormat::sniff(data),
            (None, CoverSource::Folder { path }) => {
                let mut head = Vec::with_capacity(SNIFF_LEN);
                File::open(path)?
                    .take(SNIFF_LEN as u64)
                    .read_to_end(&mut head)?;
                PictureFormat::sniff(&head)
            }
            (None, CoverSource::Embedded { .. }) => None,
        };
        sniffed.ok_or(CoverError::UnknownFormat)
    }

    /// The content hash of the encoded picture; loads the bytes.
    pub fn hash(&self) -> Result<CoverHash, CoverError> {
        Ok(CoverHash::of(self.bytes()?))
    }

    /// The picture's format and dimensions from its header, within `limits`;
    /// loads the bytes but decodes no pixels.
    pub fn probe(&self, limits: &CoverLimits) -> Result<PictureInfo, CoverError> {
        probe_picture(self.bytes()?, limits)
    }

    fn load(&self) -> Result<Vec<u8>, CoverError> {
        Self::read_file(&self.source, self.max_bytes)
    }

    fn read_file(source: &CoverSource, max_bytes: usize) -> Result<Vec<u8>, CoverError> {
        let CoverSource::Folder { path } = source else {
            // An embedded cover always holds its bytes.
            return Err(CoverError::UnknownFormat);
        };
        let file = File::open(path)?;
        let limit = max_bytes as u64;
        let len = file.metadata()?.len();
        if len > limit {
            return Err(CoverError::TooLarge { bytes: len, limit });
        }
        let mut data = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
        // The file may have grown since it was measured.
        file.take(limit.saturating_add(1)).read_to_end(&mut data)?;
        if data.len() as u64 > limit {
            return Err(CoverError::TooLarge {
                bytes: data.len() as u64,
                limit,
            });
        }
        Ok(data)
    }
}

impl fmt::Debug for Cover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cover")
            .field("source", &self.source)
            .field("byte_len", &self.byte_len())
            .field("loaded", &self.is_loaded())
            .finish()
    }
}
