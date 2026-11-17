// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The error type of decoding, and its mapping to the model's [`EngineError`].

use std::fmt::Write as _;

use helikodu_model::player::EngineError;

/// Everything that can go wrong while opening, decoding or seeking a source.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// The file could not be opened or read.
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),

    /// A backend recognised the file but could not open it.
    #[error("{backend} cannot open the file: {reason}")]
    Open {
        /// The backend.
        backend: &'static str,
        /// What went wrong.
        reason: String,
    },

    /// The backend has no subsongs, but one was asked for.
    #[error("{backend} does not support subsongs")]
    SubsongUnsupported {
        /// The backend.
        backend: &'static str,
    },

    /// No backend could open the file. Each backend that tried says why.
    #[error("no decoder can open the file{}", attempt_list(attempts))]
    NoDecoder {
        /// Each backend that tried, with its error message.
        attempts: Vec<(&'static str, String)>,
    },

    /// The section to play is not within the file.
    #[error("the section is not in the file: {reason}")]
    InvalidSection {
        /// What is wrong with it.
        reason: String,
    },

    /// Decoding or seeking failed after the source was opened.
    #[error("{backend} failed while decoding: {reason}")]
    Decode {
        /// The backend.
        backend: &'static str,
        /// What went wrong.
        reason: String,
    },
}

fn attempt_list(attempts: &[(&'static str, String)]) -> String {
    let mut out = String::new();
    for (i, (backend, reason)) in attempts.iter().enumerate() {
        let sep = if i == 0 { " (" } else { "; " };
        let _ = write!(out, "{sep}{backend}: {reason}");
    }
    if !attempts.is_empty() {
        out.push(')');
    }
    out
}

impl DecodeError {
    /// The engine error to report: [`EngineError::Unplayable`] for everything that
    /// keeps a source from being opened, [`EngineError::Decode`] for failures after
    /// it was.
    pub fn to_engine_error(&self) -> EngineError {
        let reason = self.to_string();
        match self {
            Self::Decode { .. } => EngineError::Decode { reason },
            Self::Io(_)
            | Self::Open { .. }
            | Self::SubsongUnsupported { .. }
            | Self::NoDecoder { .. }
            | Self::InvalidSection { .. } => EngineError::Unplayable { reason },
        }
    }
}

impl From<DecodeError> for EngineError {
    fn from(error: DecodeError) -> Self {
        error.to_engine_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_to_engine_errors() {
        let e = DecodeError::NoDecoder {
            attempts: vec![
                ("dsd", "unsupported DSD file: DST compression".into()),
                ("ffmpeg", "invalid data".into()),
            ],
        };
        assert_eq!(
            EngineError::from(e),
            EngineError::Unplayable {
                reason: "no decoder can open the file (dsd: unsupported DSD file: DST \
                         compression; ffmpeg: invalid data)"
                    .into()
            }
        );
        let e = DecodeError::Decode {
            backend: "ffmpeg",
            reason: "the stream is damaged".into(),
        };
        assert!(matches!(e.to_engine_error(), EngineError::Decode { .. }));
        let e = DecodeError::NoDecoder { attempts: vec![] };
        assert_eq!(e.to_string(), "no decoder can open the file");
        let e = DecodeError::SubsongUnsupported { backend: "ffmpeg" };
        assert!(matches!(
            e.to_engine_error(),
            EngineError::Unplayable { .. }
        ));
    }
}
