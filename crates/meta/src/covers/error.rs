// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a cover could not be read or decoded.

use super::format::PictureFormat;

/// Why a cover could not be read, decoded or encoded.
#[derive(Debug, thiserror::Error)]
pub enum CoverError {
    /// The file could not be opened or read.
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),
    /// Neither the tag library nor FFmpeg could read the audio file.
    #[error("cannot read the pictures of this file: {0}")]
    Unreadable(String),
    /// The picture is larger than [`CoverLimits::max_bytes`](super::CoverLimits::max_bytes).
    #[error("the picture has {bytes} bytes; the limit is {limit}")]
    TooLarge {
        /// The picture's size.
        bytes: u64,
        /// The limit.
        limit: u64,
    },
    /// The picture declares more pixels than
    /// [`CoverLimits::max_pixels`](super::CoverLimits::max_pixels), or a zero
    /// width or height.
    #[error("the picture declares {width}×{height} pixels; the limit is {limit} pixels")]
    BadDimensions {
        /// The declared width.
        width: u32,
        /// The declared height.
        height: u32,
        /// The pixel limit.
        limit: u64,
    },
    /// The decoder stopped at one of its own allocation limits.
    #[error("the picture needs more memory than allowed: {0}")]
    LimitExceeded(String),
    /// The bytes are not in a picture format known here.
    #[error("the data is not a known picture format")]
    UnknownFormat,
    /// The picture's format is known but not decoded (only JPEG, PNG and WebP are).
    #[error("{} pictures are not supported", .0.mime_type())]
    UnsupportedFormat(PictureFormat),
    /// The picture is malformed or truncated.
    #[error("the picture cannot be decoded: {0}")]
    Decode(String),
    /// A thumbnail could not be encoded.
    #[error("the thumbnail cannot be encoded: {0}")]
    Encode(String),
}
