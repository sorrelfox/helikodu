// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The decoder trait.

use std::fmt;

use helikodu_model::audio::{AudioSpec, MediaTime};
use helikodu_model::player::AudioStreamInfo;

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

    /// For a decoder of DSD over PCM (DoP), a decoder of the same file converted to
    /// PCM at the same position, for a path that would change the samples (the DoP
    /// markers must reach the DAC untouched). `None` for every other decoder.
    fn pcm_fallback(&self) -> Option<Result<Box<dyn Decoder>, DecodeError>> {
        None
    }

    /// The damaged parts skipped so far: how many packets or frames could not be
    /// read or decoded, and the position of the first, in frames. Backends that do
    /// not skip damage report none.
    fn skipped_errors(&self) -> (u64, Option<u64>) {
        (0, None)
    }
}

/// What a decoder decodes: for the signal-path view and diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecoderInfo {
    /// The backend, as [`DecoderFactory::name`](super::DecoderFactory::name) gives it:
    /// `ffmpeg`, `dsd`, `openmpt`, `gme`, `sidplayfp`, `vgmstream` or `rustysynth`.
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
    /// The file's audio streams when it has more than one (for the stream selector);
    /// empty otherwise.
    pub streams: Vec<AudioStreamInfo>,
    /// The index in the file of the audio stream being decoded, when the file has
    /// streams.
    pub stream: Option<u32>,
    /// The subsong being decoded, counted from 0, for backends with subsongs.
    pub subsong: Option<u32>,
    /// Whether the samples are DSD over PCM (DoP): 24-bit words with DoP markers,
    /// which must reach the device unchanged.
    pub dop: bool,
    /// Whether the file was read into memory whole before decoding.
    pub in_memory: bool,
}
