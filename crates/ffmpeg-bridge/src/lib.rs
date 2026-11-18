// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Safe wrapper over the FFmpeg C API, restricted to local files.
//!
//! Open a file with [`MediaFile::open`]: the bridge opens it itself and hands FFmpeg
//! only custom I/O, with every offline lock applied, and FFmpeg probes the content.
//! Read the streams, tags, chapters and attached pictures from the [`MediaFile`],
//! then turn it into an [`AudioDecoder`] for one audio stream, which yields
//! interleaved PCM in [`PcmBuffer`]s, trims encoder delay and padding, and seeks to
//! the exact frame.
//!
//! ```no_run
//! use helikodu_ffmpeg_bridge::{MediaFile, StreamChoice};
//!
//! # fn main() -> Result<(), helikodu_ffmpeg_bridge::Error> {
//! let file = MediaFile::open("song.flac")?;
//! println!("{:?}", file.tags().get("title"));
//! let mut decoder = file.into_decoder(StreamChoice::Auto)?;
//! while let Some(block) = decoder.read()? {
//!     println!("{} frames at {}", block.frames(), block.position());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! See the crate's README for its place in the workspace.

mod decode;
mod demux;
mod encode;
mod error;
mod ffi;
mod io;
mod metadata;
mod runtime;

pub use decode::{AudioDecoder, PcmBuffer, PcmSamples};
pub use demux::{Disposition, MediaFile, StreamChoice, StreamInfo, StreamKind};
pub use encode::{
    AudioEncoder, EncodeFormat, EncodeQuality, EncoderAvailability, EncoderConfig,
    encoder_availability, nearest_rate, supported_sample_rates,
};
pub use error::{AvError, AvErrorKind, Error};
pub use metadata::{AttachedPicture, Chapter, PictureLimits, PictureSource, Tags};
pub use runtime::{LogLevel, ffmpeg_version, set_log_level};
