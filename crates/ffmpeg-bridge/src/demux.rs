// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening and probing a file, its streams, and the choice of the audio stream.

mod media_file;
mod stream_info;

pub(crate) use media_file::Input;
pub use media_file::MediaFile;
pub use stream_info::{Disposition, StreamInfo, StreamKind};

/// Which audio stream of a file to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StreamChoice {
    /// The audio stream marked default when only some are, otherwise FFmpeg's best
    /// audio stream.
    #[default]
    Auto,
    /// The stream with this index, for example the user's choice for this file. It
    /// must be an audio stream.
    Index(usize),
}
