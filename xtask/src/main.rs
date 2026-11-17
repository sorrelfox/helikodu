// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Developer commands for the Helikodu repository.
//!
//! Run them as `cargo xtask <command>`. This program never links the product
//! crates; where it needs them, it runs Cargo or the built programs.

#![forbid(unsafe_code)]

mod check_deps;
mod ci;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  ci            run every check that CI runs, in the same order
  check-deps    check the crate dependency rules and the unsafe-code policy
  help          print this message
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("ci") => ci::run(),
        Some("check-deps") => check_deps::run(),
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
