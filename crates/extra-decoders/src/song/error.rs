// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The errors of the synthesizing decoders.

/// What can go wrong while opening or rendering a tune.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SongError {
    /// The file could not be read.
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),

    /// The library does not recognise the file or refuses it.
    #[error("{0}")]
    Unsupported(String),

    /// The subsong asked for does not exist.
    #[error("there is no subsong {index} (the file has {count})")]
    NoSuchSubsong {
        /// The subsong asked for, counted from 0.
        index: u32,
        /// How many the file has.
        count: u32,
    },

    /// MIDI files need a SoundFont, and none is set or installed.
    #[error("MIDI files need a SoundFont: choose one in the settings")]
    NeedsSoundFont,

    /// The library is not part of this build, or was not found when it was built.
    #[error("{0} is not available in this build")]
    Unavailable(&'static str),

    /// Rendering failed after the file was opened.
    #[error("{0}")]
    Render(String),
}
