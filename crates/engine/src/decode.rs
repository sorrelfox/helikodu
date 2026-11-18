// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding: the [`Decoder`] and [`DecoderFactory`] traits, the registry that picks
//! a backend for a file, the FFmpeg backend, the DSF/DFF backend (PCM or DoP), the
//! synthesizing backends (tracker modules, game music, SID, vgmstream, MIDI) with
//! their subsongs, and the trimming of sections (CUE-style spans) to the exact
//! frame.
//!
//! [`DecoderRegistry::open`] reads the first bytes of the file, asks every
//! registered [`DecoderFactory`] how well it recognises them, and opens the file
//! with the best one; when that fails it tries the next. File names are never
//! trusted: the FFmpeg backend probes the content itself, the DSD and synthesizing
//! backends recognise their files by their signature (vgmstream, whose formats
//! mostly have none, by name, and then by content when it opens them). A
//! [`SourceSection::Subsong`] is passed to the backend; a [`SourceSection::Span`]
//! is applied on top of whichever backend opened the file.
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
//! [`SourceSection::Subsong`]: helikodu_model::library::SourceSection::Subsong

mod block;
mod decoder;
#[cfg(feature = "dsd")]
mod dsd;
mod error;
mod factory;
mod ffmpeg;
mod options;
mod registry;
mod section;
#[cfg(feature = "song")]
mod songs;

pub use block::{BlockSamples, PcmBlock};
pub use decoder::{Decoder, DecoderInfo};
#[cfg(feature = "dsd")]
pub use dsd::DsdFactory;
pub use error::DecodeError;
pub use factory::{DecoderFactory, OpenRequest, Probe, ProbeScore, SourceInput};
pub use ffmpeg::FfmpegFactory;
pub use options::{DecodeOptions, SharedOptions};
pub use registry::DecoderRegistry;
#[cfg(feature = "midi")]
pub use songs::midi_soundfont;
#[cfg(feature = "song")]
pub use songs::{SongFactory, SongLibrary, library_version};
