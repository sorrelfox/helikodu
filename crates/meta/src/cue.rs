// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! CUE sheets and container chapters as virtual tracks.
//!
//! A CUE sheet splits audio files into tracks. [`read_cue`] reads a `.cue` file,
//! [`read_embedded_cue`] the sheet embedded in an audio file (a `CUESHEET` tag or
//! the FLAC cue sheet block), and [`read_chapters`] turns a file's chapters into
//! the same [`VirtualTrack`] form: a [`MediaSource`] with a
//! [`SourceSection::Span`] and the track's [`TrackTags`].
//!
//! The parser is this crate's own: tolerant of the sheets found in the wild,
//! bounded in what it accepts, and never panicking. What it skips or repairs is
//! reported as line-numbered [`CueWarning`]s. Charsets: a byte order mark, then
//! UTF-8, then the legacy charset the user chose or else a guessed one (see
//! [`EncodingSource`]). `FILE` paths are kept as bytes and must stay inside the
//! sheet's folder (see [`relative_path`]). CUE times are frames of 1/75 s and
//! convert exactly to [`MediaTime`].
//!
//! A track starts at `INDEX 01` and ends at the next track's `INDEX 01` in the
//! same file; `INDEX 00` is [`VirtualTrack::pregap_start`]. [`VirtualTrack`] has
//! the whole pregap policy.
//!
//! [`MediaSource`]: helikodu_model::library::MediaSource
//! [`SourceSection::Span`]: helikodu_model::library::SourceSection::Span
//! [`MediaTime`]: helikodu_model::audio::MediaTime
//! [`TrackTags`]: crate::tags::TrackTags

mod chapters;
mod embedded;
mod fields;
mod flac_block;
mod parse;
mod path;
mod reading;
mod resolve;
mod sheet;
mod text;
mod warning;

pub use chapters::{chapters_to_tracks, read_chapters};
pub use embedded::read_embedded_cue;
pub use parse::MAX_SHEET_BYTES;
pub use path::{PathRejection, relative_path};
pub use reading::{CueError, CueOptions, CueOrigin, CueTracks, Resolution, read_cue};
pub use resolve::VirtualTrack;
pub use sheet::{CueFile, CueFlags, CueIndex, CueSheet, CueTime, CueTrack, Remark};
pub use text::{EncodingSource, SheetEncoding};
pub use warning::{CueWarning, CueWarningKind};
