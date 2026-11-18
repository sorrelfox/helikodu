// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The error type of the crate and the mapping of FFmpeg's `AVERROR` codes.

use std::ffi::{CStr, c_char, c_int};
use std::fmt;

use crate::ffi::ff;

/// Everything that can go wrong while opening, decoding or seeking a file.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The file could not be opened or read.
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),

    /// An FFmpeg function failed.
    #[error("{operation} failed: {error}")]
    Ffmpeg {
        /// The FFmpeg function that failed.
        operation: &'static str,
        /// What FFmpeg reported.
        #[source]
        error: AvError,
    },

    /// FFmpeg did not take one of the options that lock it to the local file.
    /// The file is not opened, because a lock would silently be missing.
    #[error("FFmpeg did not apply the option `{key}`")]
    OptionNotApplied {
        /// The option's name.
        key: String,
    },

    /// The file has no audio stream.
    #[error("the file has no audio stream")]
    NoAudioStream,

    /// The requested stream does not exist or is not an audio stream.
    #[error("stream {index} is not an audio stream of this file")]
    NotAnAudioStream {
        /// The index that was asked for.
        index: usize,
    },

    /// This FFmpeg build has no decoder for the stream's codec.
    #[error("no decoder for the codec `{codec}`")]
    DecoderNotFound {
        /// FFmpeg's name of the codec.
        codec: String,
    },

    /// This FFmpeg build has no encoder for the format.
    #[error("no encoder for {format}")]
    EncoderNotFound {
        /// The format's usual name.
        format: String,
    },

    /// The decoder produced samples in a format the bridge does not convert.
    #[error("the decoder produced an unsupported sample format ({format})")]
    UnsupportedSampleFormat {
        /// FFmpeg's `AVSampleFormat` value.
        format: i32,
    },

    /// The decoder produced audio without a usable sample rate or channel count.
    #[error("the decoder produced an invalid audio format: {reason}")]
    InvalidAudioFormat {
        /// What was wrong.
        reason: &'static str,
    },

    /// Too many packets in a row could not be read or decoded.
    #[error("the stream is damaged: {last}")]
    Damaged {
        /// The last error FFmpeg reported.
        last: AvError,
    },
}

impl Error {
    pub(crate) fn ffmpeg(operation: &'static str, code: c_int) -> Self {
        Self::Ffmpeg {
            operation,
            error: AvError::new(code),
        }
    }

    pub(crate) fn out_of_memory(operation: &'static str) -> Self {
        Self::ffmpeg(operation, ff::AVERROR(libc::ENOMEM))
    }
}

/// Turns a negative FFmpeg return value into an error.
pub(crate) fn check(operation: &'static str, code: c_int) -> Result<c_int, Error> {
    if code < 0 {
        Err(Error::ffmpeg(operation, code))
    } else {
        Ok(code)
    }
}

/// An FFmpeg error code (a negative `AVERROR` value).
#[derive(Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{} (code {code})", self.message())]
pub struct AvError {
    code: i32,
}

impl AvError {
    pub(crate) const fn new(code: i32) -> Self {
        Self { code }
    }

    /// The raw code as FFmpeg returned it.
    pub const fn code(self) -> i32 {
        self.code
    }

    /// What kind of error the code stands for.
    pub fn kind(self) -> AvErrorKind {
        match self.code {
            ff::AVERROR_EOF => AvErrorKind::EndOfFile,
            ff::AVERROR_INVALIDDATA => AvErrorKind::InvalidData,
            ff::AVERROR_DECODER_NOT_FOUND => AvErrorKind::DecoderNotFound,
            ff::AVERROR_DEMUXER_NOT_FOUND => AvErrorKind::DemuxerNotFound,
            ff::AVERROR_STREAM_NOT_FOUND => AvErrorKind::StreamNotFound,
            ff::AVERROR_PATCHWELCOME => AvErrorKind::NotImplemented,
            ff::AVERROR_EXTERNAL => AvErrorKind::External,
            ff::AVERROR_BUG | ff::AVERROR_BUG2 => AvErrorKind::Bug,
            code if code < 0 && code > -4096 => match -code {
                libc::EPERM | libc::EACCES => AvErrorKind::NotPermitted,
                libc::EINVAL => AvErrorKind::InvalidArgument,
                libc::ENOMEM => AvErrorKind::OutOfMemory,
                errno => AvErrorKind::Os(errno),
            },
            _ => AvErrorKind::Other,
        }
    }

    /// FFmpeg's description of the code.
    pub fn message(self) -> String {
        let mut buf = [0 as c_char; 128];
        // SAFETY: buf is writable for its full length; av_strerror always writes a
        // NUL-terminated string within errbuf_size, also for unknown codes.
        unsafe { ff::av_strerror(self.code, buf.as_mut_ptr(), buf.len()) };
        // SAFETY: av_strerror wrote a NUL-terminated string into buf.
        unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }
}

impl fmt::Debug for AvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AvError")
            .field("code", &self.code)
            .field("kind", &self.kind())
            .finish()
    }
}

/// The kinds of FFmpeg error codes the player tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AvErrorKind {
    /// The end of the file was reached.
    EndOfFile,
    /// The data is not valid for the format or codec.
    InvalidData,
    /// No decoder for the codec.
    DecoderNotFound,
    /// No demuxer recognised the file.
    DemuxerNotFound,
    /// The stream was not found.
    StreamNotFound,
    /// A feature FFmpeg does not implement.
    NotImplemented,
    /// An error inside a callback or external library, including a panic caught in
    /// one of the bridge's own callbacks.
    External,
    /// An internal FFmpeg bug.
    Bug,
    /// The operation is not permitted: for example a nested open that the offline
    /// lock refused.
    NotPermitted,
    /// An invalid argument: FFmpeg also reports a format, protocol or codec that is
    /// not on a whitelist this way.
    InvalidArgument,
    /// Out of memory.
    OutOfMemory,
    /// Another operating-system error, with its `errno` value.
    Os(i32),
    /// A code the bridge does not know.
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_codes() {
        assert_eq!(AvError::new(ff::AVERROR_EOF).kind(), AvErrorKind::EndOfFile);
        assert_eq!(
            AvError::new(ff::AVERROR_INVALIDDATA).kind(),
            AvErrorKind::InvalidData
        );
        assert_eq!(
            AvError::new(ff::AVERROR(libc::EPERM)).kind(),
            AvErrorKind::NotPermitted
        );
        assert_eq!(
            AvError::new(ff::AVERROR(libc::EIO)).kind(),
            AvErrorKind::Os(libc::EIO)
        );
        assert_eq!(AvError::new(-1_000_000).kind(), AvErrorKind::Other);
    }

    #[test]
    fn message_is_ffmpegs() {
        assert_eq!(AvError::new(ff::AVERROR_EOF).message(), "End of file");
    }
}
