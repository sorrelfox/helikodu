// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs a program under a seccomp filter that kills it, with `SIGSYS`, as soon
//! as it tries to create an `AF_INET` or `AF_INET6` socket.
//!
//! `cargo xtask offline-audit` starts every test program through this launcher,
//! inside a network namespace with no interfaces. The launcher installs the
//! filter on itself and then replaces itself with the program, so the program
//! and everything it starts inherit the filter.
//!
//! `--probe <kind>` makes one socket call and reports what happened; the
//! audit's self-test uses it to prove that the filter catches what it should.

#![forbid(unsafe_code)]

mod filter;
mod probe;

use std::env;
use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: helikodu-offline-launcher [--] <program> [<argument>...]
       helikodu-offline-launcher --probe <kind>

Runs <program> under a seccomp filter that kills it with SIGSYS when it calls
socket() or socketpair() with the domain AF_INET or AF_INET6. Other sockets,
such as AF_UNIX (D-Bus, PipeWire) and AF_NETLINK, are not affected.

--probe makes one socket call and prints what happened:
  inet         socket(AF_INET)
  inet6        socket(AF_INET6)
  unix         socket(AF_UNIX) and socketpair(AF_UNIX); fails if either is refused
  interfaces   lists the network interfaces; fails unless only `lo` exists
";

/// Exit code when the launcher itself fails, as with `env` and container tools.
const LAUNCHER_FAILED: u8 = 125;
/// Exit code when the program exists but cannot be run.
const CANNOT_EXECUTE: u8 = 126;
/// Exit code when the program does not exist.
const NOT_FOUND: u8 = 127;

fn main() -> ExitCode {
    let mut args: Vec<OsString> = env::args_os().skip(1).collect();
    match args.first().and_then(|arg| arg.to_str()) {
        None | Some("--help" | "-h") => {
            print!("{USAGE}");
            return if args.is_empty() {
                ExitCode::from(LAUNCHER_FAILED)
            } else {
                ExitCode::SUCCESS
            };
        }
        Some("--probe") => {
            let kind = args.get(1).and_then(|kind| kind.to_str()).unwrap_or("");
            return probe::run(kind);
        }
        Some("--") => {
            args.remove(0);
        }
        Some(option) if option.starts_with('-') => {
            eprint!("helikodu-offline-launcher: unknown option `{option}`\n\n{USAGE}");
            return ExitCode::from(LAUNCHER_FAILED);
        }
        Some(_) => {}
    }

    let Some((program, program_args)) = args.split_first() else {
        eprint!("helikodu-offline-launcher: no program given\n\n{USAGE}");
        return ExitCode::from(LAUNCHER_FAILED);
    };

    if let Err(error) = filter::install() {
        eprintln!("helikodu-offline-launcher: could not install the seccomp filter: {error}");
        return ExitCode::from(LAUNCHER_FAILED);
    }

    // `exec` only returns if the program could not be started.
    let error = Command::new(program).args(program_args).exec();
    eprintln!(
        "helikodu-offline-launcher: could not run {}: {error}",
        program.to_string_lossy()
    );
    if error.kind() == io::ErrorKind::NotFound {
        ExitCode::from(NOT_FOUND)
    } else {
        ExitCode::from(CANNOT_EXECUTE)
    }
}
