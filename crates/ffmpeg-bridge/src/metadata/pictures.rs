// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Embedded pictures, as encoded bytes. The bridge never decodes an image.

use std::ffi::c_int;

use crate::demux::{Input, StreamInfo, StreamKind};
use crate::ffi::{self, codec_id};

/// Limits on the pictures read from one file. Pictures are only copied, never
/// decoded, but a hostile file could still carry very large ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureLimits {
    /// The largest picture to copy, in bytes. Larger ones are left out.
    pub max_bytes: usize,
    /// The most pictures to copy from one file.
    pub max_count: usize,
}

impl Default for PictureLimits {
    /// 16 MiB per picture and 16 pictures.
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_count: 16,
        }
    }
}

/// Where a picture was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PictureSource {
    /// An attached-picture stream: ID3v2 `APIC`, a FLAC `PICTURE` block, an MP4
    /// `covr` atom and the like.
    AttachedPicture,
    /// A Matroska attachment with an image media type that FFmpeg did not already
    /// turn into an attached-picture stream (it does so for JPEG, PNG and a few
    /// more).
    Attachment,
}

/// One embedded picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedPicture {
    /// The index of the stream that carries it.
    pub stream_index: usize,
    /// Where it was found.
    pub source: PictureSource,
    /// The media type, such as `image/jpeg`, when it is known.
    pub mime_type: Option<String>,
    /// The picture's description: for ID3 and FLAC, FFmpeg puts the picture type
    /// here (`Cover (front)`); for a Matroska attachment, its file name.
    pub description: Option<String>,
    /// The encoded picture.
    pub data: Vec<u8>,
}

fn mime_for_codec(id: c_int) -> Option<&'static str> {
    Some(match id {
        codec_id::MJPEG => "image/jpeg",
        codec_id::PNG => "image/png",
        codec_id::BMP => "image/bmp",
        codec_id::GIF => "image/gif",
        codec_id::TIFF => "image/tiff",
        codec_id::WEBP => "image/webp",
        _ => return None,
    })
}

/// Copies `len` bytes from `data` if the limits allow.
///
/// # Safety
/// `data` must be NULL or readable for `len` bytes.
unsafe fn copy_bytes(data: *const u8, len: c_int, limits: &PictureLimits) -> Option<Vec<u8>> {
    let len = usize::try_from(len).ok()?;
    if data.is_null() || len == 0 || len > limits.max_bytes {
        return None;
    }
    // SAFETY: guaranteed by the caller.
    Some(unsafe { std::slice::from_raw_parts(data, len) }.to_vec())
}

/// Reads the pictures of an open input whose streams are described by `streams`.
pub(crate) fn read(
    input: &Input,
    streams: &[StreamInfo],
    limits: &PictureLimits,
) -> Vec<AttachedPicture> {
    let mut out = Vec::new();
    for info in streams {
        if out.len() >= limits.max_count {
            break;
        }
        let Some(st) = input.stream(info.index) else {
            continue;
        };
        if info.kind == StreamKind::Video && info.disposition.is_attached_picture() {
            // SAFETY: a valid stream of the open context. attached_pic is a packet
            // the demuxer filled while opening; its data is readable for size bytes.
            let (data, size, id) = unsafe {
                (
                    (*st).attached_pic.data,
                    (*st).attached_pic.size,
                    ffi::codec_id((*st).codecpar),
                )
            };
            // SAFETY: as above.
            if let Some(data) = unsafe { copy_bytes(data, size, limits) } {
                out.push(AttachedPicture {
                    stream_index: info.index,
                    source: PictureSource::AttachedPicture,
                    mime_type: mime_for_codec(id).map(str::to_owned),
                    description: info
                        .tags
                        .get("comment")
                        .or_else(|| info.tags.get("filename"))
                        .map(str::to_owned),
                    data,
                });
            }
        } else if info.kind == StreamKind::Attachment {
            let mime = info.tags.get("mimetype").map(str::to_owned);
            if !mime.as_deref().is_some_and(|m| m.starts_with("image/")) {
                continue;
            }
            // SAFETY: a valid stream; an attachment's bytes are its codec
            // parameters' extradata, readable for extradata_size bytes.
            let (data, size) = unsafe {
                (
                    (*(*st).codecpar).extradata,
                    (*(*st).codecpar).extradata_size,
                )
            };
            // SAFETY: as above.
            if let Some(data) = unsafe { copy_bytes(data, size, limits) } {
                out.push(AttachedPicture {
                    stream_index: info.index,
                    source: PictureSource::Attachment,
                    mime_type: mime,
                    description: info.tags.get("filename").map(str::to_owned),
                    data,
                });
            }
        }
    }
    out
}
