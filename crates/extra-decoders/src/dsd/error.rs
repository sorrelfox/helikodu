// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The error type of the DSD reader.

/// Everything that can go wrong while reading a DSD file.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DsdError {
    /// The file could not be opened or read.
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),

    /// The file is neither a DSF nor a DFF file.
    #[error("not a DSF or DFF file")]
    NotDsd,

    /// The file is a DSD file, but one this reader does not handle, such as a
    /// DST-compressed DFF file.
    #[error("unsupported DSD file: {0}")]
    Unsupported(String),

    /// The file's structure is damaged.
    #[error("damaged DSD file: {0}")]
    Malformed(String),
}

impl DsdError {
    pub(crate) fn malformed(reason: impl Into<String>) -> Self {
        Self::Malformed(reason.into())
    }

    pub(crate) fn unsupported(reason: impl Into<String>) -> Self {
        Self::Unsupported(reason.into())
    }
}
