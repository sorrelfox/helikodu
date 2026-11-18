// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the synthesizing decoders have in common: subsongs, lengths, loops, the fade
//! at the end, the options and the errors.
//!
//! Tracker modules, game music, SID tunes, game audio streams and MIDI files are
//! not stored as samples but synthesized, often without a stated end. Each decoder
//! renders interleaved `f32` frames at [`SongOptions::rate`] and stops at a length it
//! decides: the length the file states, or for looping tunes the intro and
//! [`SongOptions::loops`] passes of the loop, or [`SongOptions::default_length`];
//! then a fade of [`SongOptions::fade`] where the music would go on.

mod envelope;
mod error;
mod info;
mod options;

use std::fmt;

pub use envelope::Envelope;
pub use error::SongError;
pub(crate) use info::text;
pub use info::{SongInfo, SubsongInfo};
pub use options::{Interpolation, SongOptions};

/// One subsong of a file being rendered.
///
/// Positions count frames at [`rate`](Self::rate) from the start of the subsong.
pub trait SongDecoder: Send + fmt::Debug {
    /// The file and its subsongs.
    fn info(&self) -> &SongInfo;

    /// The subsong being rendered, counted from 0.
    fn subsong(&self) -> u32;

    /// The sample rate.
    fn rate(&self) -> u32;

    /// The channel count. Channels are in WAVE order (left, right, ...).
    fn channels(&self) -> u16;

    /// The number of frames the subsong plays, fade included, when known.
    fn frames(&self) -> Option<u64>;

    /// The position of the next frame.
    fn position(&self) -> u64;

    /// Renders up to `max_frames` frames into `out`, which is cleared first, and
    /// returns how many; 0 at the end.
    fn read(&mut self, max_frames: usize, out: &mut Vec<f32>) -> Result<usize, SongError>;

    /// Moves to `frame`; past the end moves to the end. Lands on the exact frame
    /// unless the decoder says otherwise.
    fn seek(&mut self, frame: u64) -> Result<(), SongError>;
}
