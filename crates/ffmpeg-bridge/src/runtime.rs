// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Process-wide settings and facts of the FFmpeg libraries in use.

use crate::ffi::{self, ff};

/// How much FFmpeg writes to standard error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogLevel {
    /// Nothing.
    Quiet,
    /// Only errors (FFmpeg's default is `Info`).
    Error,
    /// Errors and warnings.
    Warning,
    /// Informational messages as well.
    Info,
}

/// Sets how much FFmpeg logs, for the whole process.
pub fn set_log_level(level: LogLevel) {
    let level = match level {
        LogLevel::Quiet => ff::AV_LOG_QUIET,
        LogLevel::Error => ff::AV_LOG_ERROR,
        LogLevel::Warning => ff::AV_LOG_WARNING,
        LogLevel::Info => ff::AV_LOG_INFO,
    };
    // SAFETY: sets a global integer; FFmpeg reads it without other requirements.
    unsafe { ff::av_log_set_level(level) };
}

/// The version of the FFmpeg libraries in use, as FFmpeg reports it
/// (for example `9.0.2`), for diagnostics.
pub fn ffmpeg_version() -> String {
    // SAFETY: returns a static NUL-terminated string.
    let version = unsafe { ffi::string(ff::av_version_info()) };
    version.unwrap_or_default()
}
