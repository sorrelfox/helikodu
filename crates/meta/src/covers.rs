// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cover art: pictures embedded in audio files, image files in the album folder,
//! and safe decoding into thumbnails.
//!
//! [`read_embedded_covers`] lists a file's embedded pictures: ID3v2 `APIC`, FLAC
//! `PICTURE` blocks, Vorbis `METADATA_BLOCK_PICTURE`, MP4 `covr` and APEv2
//! `Cover Art (…)` items through lofty, and FFmpeg's attached pictures for the
//! formats lofty lacks or fails on. [`find_folder_covers`] lists the image files
//! beside a track (and in the parent of a disc folder such as `CD1`) in a fixed
//! name priority. [`find_cover`] combines the two and picks one cover.
//!
//! A [`Cover`] knows where it came from ([`CoverSource`]) and loads its bytes
//! only when asked: a folder file is not read until its bytes, format or hash
//! are needed. Its [`CoverHash`] (SHA-256 of the encoded bytes) identifies equal
//! pictures wherever they were found. The format is sniffed from the bytes
//! ([`PictureFormat::sniff`]), never taken from a file extension or a declared
//! media type.
//!
//! Pictures come from untrusted files. Every step is bounded by [`CoverLimits`]
//! (32 MiB and 64 Mi pixels by default): pictures over the byte limit are not
//! copied, and [`decode_picture`] checks the byte size, then the dimensions the
//! header declares, before it allocates any pixels. Only JPEG, PNG and WebP are
//! decoded, by memory-safe Rust decoders. [`make_thumbnail`] scales a picture to
//! fit a square, keeping its aspect ratio.

mod decode;
mod embedded;
mod error;
mod folder;
mod format;
mod hash;
mod limits;
mod picture_type;
mod select;
mod source;

pub use decode::{PictureInfo, RgbaPicture, decode_picture, make_thumbnail, probe_picture};
pub use embedded::{
    EmbeddedCovers, SkipReason, SkippedPicture, read_embedded_covers,
    read_embedded_covers_from_file,
};
pub use error::CoverError;
pub use folder::{FolderArtOptions, FolderCovers, find_folder_covers, is_disc_folder};
pub use format::{PictureFormat, SNIFF_LEN};
pub use hash::CoverHash;
pub use limits::CoverLimits;
pub use picture_type::PictureType;
pub use select::{CoverOptions, CoverPreference, choose_front, find_cover};
pub use source::{Cover, CoverSource, EmbeddedTag};
