// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Isolated metadata extraction for the library scanner.
//!
//! Tracker modules, game music, SID tunes and MIDI files are read by their own
//! decoders, which also report their subsongs (see `long_tail.rs`); everything else
//! by `helikodu_meta::worker`.
//!
//! The scanner starts this program with pipes on its standard input and output and
//! sends it one file at a time. The program limits its own address space and turns
//! core dumps off before it reads anything, then serves requests until its input
//! closes. The protocol is in `helikodu_meta::worker`.
//!
//! ```text
//! helikodu-scan-worker [--memory-limit BYTES]
//! ```
//!
//! `--memory-limit` sets the address-space limit (default 1 GiB; 0 for none).

#![forbid(unsafe_code)]

mod faults;
mod limits;
mod long_tail;

use std::io::{self, BufReader, BufWriter};
use std::process::ExitCode;

use helikodu_ffmpeg_bridge::{LogLevel, set_log_level};
use helikodu_meta::worker::{Response, handle, serve};

fn main() -> ExitCode {
    let memory_limit = match limits::memory_limit_from_args(std::env::args().skip(1)) {
        Ok(limit) => limit,
        Err(message) => {
            eprintln!("helikodu-scan-worker: {message}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = limits::apply(memory_limit) {
        eprintln!("helikodu-scan-worker: cannot set resource limits: {e}");
        return ExitCode::FAILURE;
    }
    set_log_level(LogLevel::Quiet);
    let faults = faults::Faults::from_env();

    let input = BufReader::new(io::stdin().lock());
    let output = BufWriter::new(io::stdout().lock());
    match serve(input, output, |request| {
        faults.inject(&request.path);
        match long_tail::read(request) {
            Some(result) => Response {
                id: request.id,
                result,
            },
            None => handle(request),
        }
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("helikodu-scan-worker: {e}");
            ExitCode::FAILURE
        }
    }
}
