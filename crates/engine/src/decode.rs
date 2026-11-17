// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding: the [`Decoder`] and [`DecoderFactory`] traits, the registry that picks
//! a backend for a file, the FFmpeg backend and the DSF/DFF backend, and the
//! trimming of sections (CUE-style spans) to the exact frame.
//!
//! [`DecoderRegistry::open`] reads the first bytes of the file, asks every
//! registered [`DecoderFactory`] how well it recognises them, and opens the file
//! with the best one; when that fails it tries the next. File names are never
//! trusted: the FFmpeg backend probes the content itself, and the DSD backend
//! recognises its files by their signature. A [`SourceSection::Span`] is applied on
//! top of whichever backend opened the file.
//!
//! # Block format
//!
//! A [`Decoder`] fills a [`PcmBlock`]: interleaved samples in canonical (WAVE)
//! channel order, in the sample format the source decodes to. Integer sources stay
//! integers with their exact values (8, 16 or 32 bits; 24-bit sources in the top
//! 24 bits of 32-bit words), so a later bit-perfect path can pass them to the
//! device untouched; floating-point sources are `f32`. The render side converts a
//! block to the `f32` the output ring buffer carries with
//! [`PcmBlock::append_f32`]; that conversion is exact for 8-, 16- and 24-bit
//! integers.
//!
//! # Positions and gapless playback
//!
//! Positions count frames from the start of the source (of the section, for a
//! span), after the encoder delay. Encoder delay and padding are trimmed by the
//! backend: FFmpeg, through the bridge, for MP3, AAC, Opus, Vorbis and the other
//! formats that carry the information. Seeking lands on the exact frame.
//!
//! [`SourceSection::Span`]: helikodu_model::library::SourceSection::Span

mod block;
mod decoder;
#[cfg(feature = "dsd")]
mod dsd;
mod error;
mod factory;
mod ffmpeg;
mod registry;
mod section;

pub use block::{BlockSamples, PcmBlock};
pub use decoder::{Decoder, DecoderInfo};
#[cfg(feature = "dsd")]
pub use dsd::DsdFactory;
pub use error::DecodeError;
pub use factory::{DecoderFactory, Probe, ProbeScore};
pub use ffmpeg::FfmpegFactory;
pub use registry::DecoderRegistry;
