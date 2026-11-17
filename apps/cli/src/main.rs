// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The command-line tool.
//!
//! Exit codes: 0 success, 1 a runtime error, 2 a usage error, 3 none of the files
//! could be played.

#![forbid(unsafe_code)]

mod offline;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use self::offline::info::InfoArgs;
use self::offline::list_outputs::ListOutputsArgs;
use self::offline::play::PlayArgs;
use self::offline::scan::ScanArgs;
use self::offline::search::SearchArgs;
use self::offline::stats::StatsArgs;

/// The exit code of a runtime error.
const EXIT_FAILURE: u8 = 1;
/// The exit code when none of the files given to `play` could be played.
const EXIT_NOTHING_PLAYABLE: u8 = 3;

/// Plays audio files, lists output devices, and scans and searches the music library.
#[derive(Debug, Parser)]
#[command(name = "helikodu-cli", version, max_term_width = 100)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Play files in the order given, without gaps between them.
    Play(PlayArgs),
    /// List the output devices of the audio backends.
    ListOutputs(ListOutputsArgs),
    /// Add folders to the music library and read their new and changed files.
    Scan(ScanArgs),
    /// Find tracks in the music library.
    Search(SearchArgs),
    /// Show the tags, audio properties, loudness values and statistics of a file or
    /// library track.
    Info(InfoArgs),
    /// Show the totals of the music library and its most played tracks.
    Stats(StatsArgs),
}

/// How a command ended, when it did not fail outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Success,
    Failure,
    NothingPlayable,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Play(args) => offline::play::run(args),
        Command::ListOutputs(args) => offline::list_outputs::run(&args),
        Command::Scan(args) => offline::scan::run(args),
        Command::Search(args) => offline::search::run(&args),
        Command::Info(args) => offline::info::run(&args),
        Command::Stats(args) => offline::stats::run(&args),
    };
    match result {
        Ok(Outcome::Success) => ExitCode::SUCCESS,
        Ok(Outcome::Failure) => ExitCode::from(EXIT_FAILURE),
        Ok(Outcome::NothingPlayable) => ExitCode::from(EXIT_NOTHING_PLAYABLE),
        Err(error) => {
            eprintln!("helikodu-cli: {error:#}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}
