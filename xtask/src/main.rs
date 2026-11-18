// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Developer commands for the Helikodu repository.
//!
//! Run them as `cargo xtask <command>`. This program never links the product
//! crates; where it needs them, it runs Cargo or the built programs.

#![forbid(unsafe_code)]

mod check_deps;
mod ci;
mod codegen;
mod corpus;
mod dist;
mod offline_audit;
mod stress;
mod synth_library;
mod ui_checks;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  ci              run every check that CI runs, in the same order
                  (--tests-only, --no-offline-audit)
  check-deps      check the crate dependency rules and the unsafe-code policy
  codegen         write the files generated from the design tokens (--check)
  corpus          generate the test corpus into target/corpus (--check, --strict)
  dist            write the release source tarball with vendored crates and its
                  checksum into target/dist (--tag vX.Y.Z, --out DIR)
  offline-audit   run every test program without a network and fail on any
                  Internet socket (--self-test)
  stress          play in real time under full CPU load and fail on any underrun
                  (--secs N, --backend null|pipewire, --load THREADS)
  ui-checks       start-up, memory, frame-time, idle, accessibility and visual
                  checks of the desktop UI in a headless compositor
                  (--tracks N, --design DIR, --only perf|a11y|visual)
  help            print this message
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("ci") => ci::run(&args[1..]),
        Some("check-deps") => check_deps::run(),
        Some("codegen") => codegen::run(&args[1..]),
        Some("corpus") => corpus::run(&args[1..]),
        Some("dist") => dist::run(&args[1..]),
        Some("offline-audit") => offline_audit::run(&args[1..]),
        Some("stress") => stress::run(&args[1..]),
        Some("synth-library") => synth_library::run(&args[1..]),
        Some("ui-checks") => ui_checks::run(&args[1..]),
        Some("help" | "--help" | "-h") | None => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprint!("unknown command `{other}`\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// The root of the repository: the parent of the folder holding this crate.
fn repository_root() -> Result<PathBuf> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .context("CARGO_MANIFEST_DIR is not set; run this through `cargo xtask`")?;
    manifest_dir
        .parent()
        .map(PathBuf::from)
        .context("the xtask folder has no parent")
}

/// The Cargo executable that started this program.
fn cargo() -> String {
    env::var("CARGO").unwrap_or_else(|_| String::from("cargo"))
}
