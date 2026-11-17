// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Getting bytes into FFmpeg: custom I/O over an open file, and the locks that keep
//! FFmpeg from opening anything else.

mod custom_io;
mod lockdown;

pub(crate) use custom_io::{ByteSource, CustomIo};
pub(crate) use lockdown::{LockOptions, deny_io_open, verify_locks};
