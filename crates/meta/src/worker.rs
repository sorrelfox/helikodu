// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The messages between the library scanner and its `helikodu-scan-worker`
//! processes, and the worker's side of the exchange.
//!
//! A worker reads one file at a time for the scanner, in its own process, so a reader
//! that crashes or hangs on a broken file takes only the worker down. The scanner
//! sends a [`Request`] (a path, as bytes, and [`RequestOptions`]) on the worker's
//! standard input; the worker answers with a [`Response`] on its standard output:
//! a [`FileReport`] with the tags, the audio properties, the quick hash, the CUE
//! sheet's virtual tracks and the chapters, or a [`Failure`].
//!
//! The exchange is binary: each message is a frame, a 32-bit little-endian length
//! and that many bytes, at most [`MAX_FRAME_BYTES`]. A worker sends a [`Hello`]
//! with its [`PROTOCOL_VERSION`] before anything else; the scanner refuses a worker
//! of another version. Paths are bytes all the way, never text. Decoding is
//! bounded and never panics, whichever side sent the bytes.
//!
//! The worker's main loop is [`serve`] with [`handle`].

mod extract;
mod message;
mod quick_hash;
mod serve;
mod wire;

pub use extract::{MAX_EXTRA_VALUE_BYTES, handle, read_file};
pub use message::{
    CueReport, Failure, FailureKind, FileReport, Hello, Message, PROTOCOL_VERSION, Request,
    RequestOptions, Response,
};
pub use quick_hash::{QUICK_HASH_LEN, QUICK_HASH_SPAN, quick_hash};
pub use serve::serve;
pub use wire::{DecodeError, MAX_FRAME_BYTES, read_frame, write_frame};
