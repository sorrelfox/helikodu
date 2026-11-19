// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The command-line tool.
//!
//! Exit codes: 0 success, 1 a runtime error, 2 a usage error, 3 none of the files
//! could be played, 4 the application is not running (the commands for the
//! running application).

#![forbid(unsafe_code)]

mod offline;
mod remote;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use self::offline::analyze::AnalyzeArgs;
use self::offline::duplicates::DuplicatesArgs;
use self::offline::info::InfoArgs;
use self::offline::interchange::{
    CarryStatsArgs, ExportUserDataArgs, ImportArgs, ImportUserDataArgs, UndoImportArgs,
    WriteStatsArgs,
};
use self::offline::list_outputs::ListOutputsArgs;
use self::offline::loudness::LoudnessArgs;
use self::offline::play::PlayArgs;
use self::offline::scan::ScanArgs;
use self::offline::search::SearchArgs;
use self::offline::stats::StatsArgs;
use self::remote::commands::{
    EnqueueArgs, ExportArgs, JsonArgs, PlayQueryArgs, RateArgs, RepeatArgs, SeekArgs, ShuffleArgs,
    Transport, VolumeArgs,
};

/// The exit code of a runtime error.
const EXIT_FAILURE: u8 = 1;
/// The exit code when none of the files given to `play` could be played.
const EXIT_NOTHING_PLAYABLE: u8 = 3;
/// The exit code when a command for the running application finds none.
const EXIT_NOT_RUNNING: u8 = 4;

/// Plays audio files, lists output devices, scans, searches and measures the music
/// library, and controls the running application.
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
    /// Measure the loudness of library tracks (EBU R128, ReplayGain 2.0) and store
    /// it in the library, optionally also in the files' tags.
    Loudness(LoudnessArgs),
    /// Analyse the library tracks not analysed yet (silence and loudness envelope),
    /// as the application does in the background, and store the results.
    Analyze(AnalyzeArgs),
    /// List the groups of copies (byte-identical files) in the music library.
    Duplicates(DuplicatesArgs),
    /// Import play counts, ratings and playlists from another player (its files
    /// are only read). Shows what would change; --apply imports.
    Import(ImportArgs),
    /// Undo the newest import (or carried statistics), or list the imports.
    UndoImport(UndoImportArgs),
    /// Write ratings, play counts, history, playlists, favourites and saved
    /// searches to a JSON file, to move them to another installation.
    ExportUserData(ExportUserDataArgs),
    /// Read a file written by export-user-data into the library. Shows what
    /// would change; --apply imports.
    ImportUserData(ImportUserDataArgs),
    /// List missing files whose statistics a file of the same recording could
    /// take over; --apply carries them.
    CarryStats(CarryStatsArgs),
    /// Write the play counts and ratings that changed into the files' tags.
    WriteStats(WriteStatsArgs),
    /// Show what the running application plays.
    Status(JsonArgs),
    /// Show the queue of the running application.
    Queue(JsonArgs),
    /// List the playlists of the running application.
    Playlists(JsonArgs),
    /// Play or pause in the running application.
    PlayPause,
    /// Pause the running application.
    Pause,
    /// Go on playing in the running application.
    Resume,
    /// Stop the running application.
    Stop,
    /// Play the next track in the running application.
    Next,
    /// Play the previous track (or the start of this one) in the running
    /// application.
    Previous,
    /// Seek in the track the running application plays.
    Seek(SeekArgs),
    /// Show or set the volume of the running application.
    Volume(VolumeArgs),
    /// Set how the running application shuffles.
    Shuffle(ShuffleArgs),
    /// Set what the running application repeats.
    Repeat(RepeatArgs),
    /// Add files or the tracks of a query to the queue of the running
    /// application.
    Enqueue(EnqueueArgs),
    /// Play the library tracks a query finds in the running application,
    /// replacing its queue.
    PlayQuery(PlayQueryArgs),
    /// Rate the playing track, or a library track, in the running application.
    Rate(RateArgs),
    /// Write a playlist of the running application to a file.
    ExportPlaylist(ExportArgs),
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
        Command::Loudness(args) => offline::loudness::run(args),
        Command::Analyze(args) => offline::analyze::run(&args),
        Command::Duplicates(args) => offline::duplicates::run(&args),
        Command::Import(args) => offline::interchange::import(&args),
        Command::UndoImport(args) => offline::interchange::undo_import(&args),
        Command::ExportUserData(args) => offline::interchange::export_user_data(&args),
        Command::ImportUserData(args) => offline::interchange::import_user_data(&args),
        Command::CarryStats(args) => offline::interchange::carry_stats(&args),
        Command::WriteStats(args) => offline::interchange::write_stats(&args),
        Command::Status(args) => remote::commands::status(&args),
        Command::Queue(args) => remote::commands::queue(&args),
        Command::Playlists(args) => remote::commands::playlists(&args),
        Command::PlayPause => remote::commands::transport(Transport::PlayPause),
        Command::Pause => remote::commands::transport(Transport::Pause),
        Command::Resume => remote::commands::transport(Transport::Resume),
        Command::Stop => remote::commands::transport(Transport::Stop),
        Command::Next => remote::commands::transport(Transport::Next),
        Command::Previous => remote::commands::transport(Transport::Previous),
        Command::Seek(args) => remote::commands::seek(&args),
        Command::Volume(args) => remote::commands::volume(&args),
        Command::Shuffle(args) => remote::commands::shuffle(&args),
        Command::Repeat(args) => remote::commands::repeat(&args),
        Command::Enqueue(args) => remote::commands::enqueue(&args),
        Command::PlayQuery(args) => remote::commands::play_query(&args),
        Command::Rate(args) => remote::commands::rate(&args),
        Command::ExportPlaylist(args) => remote::commands::export_playlist(&args),
    };
    match result {
        Ok(Outcome::Success) => ExitCode::SUCCESS,
        Ok(Outcome::Failure) => ExitCode::from(EXIT_FAILURE),
        Ok(Outcome::NothingPlayable) => ExitCode::from(EXIT_NOTHING_PLAYABLE),
        Err(error) if error.is::<remote::NotRunning>() => {
            eprintln!("helikodu-cli: {error}");
            ExitCode::from(EXIT_NOT_RUNNING)
        }
        Err(error) => {
            eprintln!("helikodu-cli: {error:#}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}
