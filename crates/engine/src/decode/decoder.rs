// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The decoder trait.

use std::fmt;

use helikodu_model::audio::{AudioSpec, MediaTime};

use super::block::PcmBlock;
use super::error::DecodeError;

/// A source being decoded to PCM.
///
/// Positions count frames at the rate of [`spec`](Self::spec), from the start of
/// the source after the encoder delay; for a section, from the start of the
/// section.
pub trait Decoder: Send + fmt::Debug {
    /// What is being decoded and by which backend.
    fn info(&self) -> &DecoderInfo;

    /// The format of the decoded audio, as of the first block. A source that
    /// changes format midway (rare: chained Ogg streams) reports the change in each
    /// block's [`PcmBlock::spec`].
    fn spec(&self) -> AudioSpec;

    /// The length of the source, when known. Exact when the container or the
    /// gapless information states it (lossless formats, MP4 edit lists, DSD files,
    /// sections with an end); otherwise the container's statement.
    fn duration(&self) -> Option<MediaTime>;

    /// The position of the next block.
    fn position(&self) -> u64;

    /// Decodes the next block into `block`. `false`, with the block untouched or
    /// empty, at the end of the source.
    fn read(&mut self, block: &mut PcmBlock) -> Result<bool, DecodeError>;

    /// Moves to `to`, exactly: the next block starts at that frame. A time past
    /// the end moves to the end.
    fn seek(&mut self, to: MediaTime) -> Result<(), DecodeError>;
}

/// What a decoder decodes: for the signal-path view and diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecoderInfo {
    /// The backend, as [`DecoderFactory::name`](super::DecoderFactory::name) gives it:
    /// `ffmpeg` or `dsd`.
    pub backend: &'static str,
    /// The container format, such as `flac`, `mov,mp4,m4a,3gp,3g2,mj2` or `dsf`.
    pub container: String,
    /// The codec, such as `flac`, `aac` or `dsd`.
    pub codec: String,
    /// The number of significant bits of lossless integer audio (24 for a 24-bit
    /// FLAC file decoded into 32-bit words), when the source states it.
    pub bits_per_sample: Option<u32>,
    /// The tags the backend reads with the audio: key and value pairs in file
    /// order, with the backend's key names. The library reads tags on its own; these
    /// are for what the engine needs from the file itself, such as `R128_TRACK_GAIN`.
    pub tags: Vec<(String, String)>,
}
