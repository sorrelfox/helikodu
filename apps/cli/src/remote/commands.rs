// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The commands for the running application: their arguments and what each
//! asks the application to do.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Args, ValueEnum};
use serde_json::Value;

use super::Remote;
use super::status;
use crate::Outcome;

/// Output as JSON, for scripts.
#[derive(Debug, Args)]
pub(crate) struct JsonArgs {
    /// Print the answer as JSON, for scripts.
    #[arg(long)]
    json: bool,
}

/// A transport command without arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    PlayPause,
    Pause,
    Resume,
    Stop,
    Next,
    Previous,
}

#[derive(Debug, Args)]
pub(crate) struct SeekArgs {
    /// A position (`83`, `1:23`) or a step from the current one (`+10`, `-5`),
    /// in seconds.
    #[arg(allow_hyphen_values = true, value_name = "POSITION")]
    position: String,
}

#[derive(Debug, Args)]
pub(crate) struct VolumeArgs {
    /// The volume control from 0 to 100, or a step (`+5`, `-5`); without it the
    /// volume is printed.
    #[arg(allow_hyphen_values = true, value_name = "LEVEL")]
    level: Option<String>,
}

/// The shuffle modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ShuffleArg {
    Off,
    Tracks,
    Albums,
    Folders,
    Groupings,
    WithinAlbum,
}

#[derive(Debug, Args)]
pub(crate) struct ShuffleArgs {
    /// How to shuffle.
    #[arg(value_enum)]
    mode: ShuffleArg,
}

/// The repeat modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RepeatArg {
    /// Playback ends after the last track.
    Off,
    /// The track plays again and again.
    One,
    /// The album plays again and again.
    Album,
    /// The queue starts again.
    All,
}

#[derive(Debug, Args)]
pub(crate) struct RepeatArgs {
    /// What to repeat.
    #[arg(value_enum)]
    mode: RepeatArg,
}

#[derive(Debug, Args)]
pub(crate) struct EnqueueArgs {
    /// Add after the playing track instead of at the end.
    #[arg(long)]
    next: bool,
    /// Add the library tracks this query finds instead of files (the query
    /// language of the search field, or words).
    #[arg(long, value_name = "QUERY", conflicts_with = "files")]
    query: Option<String>,
    /// The files to add.
    #[arg(value_name = "FILE", required_unless_present = "query")]
    files: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct PlayQueryArgs {
    /// Play the tracks in random order.
    #[arg(long)]
    shuffle: bool,
    /// The query (the query language of the search field, or words).
    #[arg(required = true, value_name = "QUERY")]
    query: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct RateArgs {
    /// From 0 to 5 stars, or `none` to remove the rating.
    #[arg(value_name = "STARS")]
    stars: String,
    /// The library track (as `search` shows it); the playing track by default.
    #[arg(long, value_name = "ID")]
    track: Option<u64>,
}

#[derive(Debug, Args)]
pub(crate) struct ExportArgs {
    /// The playlist's name or number (as `playlists` shows them).
    #[arg(value_name = "PLAYLIST")]
    playlist: String,
    /// The file to write; its extension picks the format: m3u8, m3u, pls or xspf.
    #[arg(value_name = "FILE")]
    file: PathBuf,
}

/// Plays, pauses, stops or skips.
pub(crate) fn transport(command: Transport) -> Result<Outcome> {
    let remote = Remote::connect()?;
    let method = match command {
        Transport::PlayPause => "PlayPause",
        Transport::Pause => "Pause",
        Transport::Resume => "Play",
        Transport::Stop => "Stop",
        Transport::Next => "Next",
        Transport::Previous => "Previous",
    };
    remote.run(method, &())?;
    Ok(Outcome::Success)
}

/// Prints the status.
pub(crate) fn status(args: &JsonArgs) -> Result<Outcome> {
    let remote = Remote::connect()?;
    let status = remote.json("GetStatus")?;
    let mut out = io::stdout().lock();
    if args.json {
        print_json(&mut out, &status)?;
    } else {
        status::print_status(&mut out, &status)?;
    }
    Ok(Outcome::Success)
}

/// Prints the queue.
pub(crate) fn queue(args: &JsonArgs) -> Result<Outcome> {
    let remote = Remote::connect()?;
    let queue = remote.json("GetQueue")?;
    let mut out = io::stdout().lock();
    if args.json {
        print_json(&mut out, &queue)?;
    } else {
        status::print_queue(&mut out, &queue)?;
    }
    Ok(Outcome::Success)
}

/// Prints the playlists.
pub(crate) fn playlists(args: &JsonArgs) -> Result<Outcome> {
    let remote = Remote::connect()?;
    let list = remote.json("ListPlaylists")?;
    let mut out = io::stdout().lock();
    if args.json {
        print_json(&mut out, &list)?;
    } else {
        status::print_playlists(&mut out, &list)?;
    }
    Ok(Outcome::Success)
}

/// A position as milliseconds: `83`, `1:23`, `1:02:03`, `83.5`.
fn parse_time(text: &str) -> Option<u64> {
    let mut seconds = 0.0f64;
    for part in text.split(':') {
        let value: f64 = part.parse().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        seconds = seconds * 60.0 + value;
    }
    Some((seconds * 1000.0).round() as u64)
}

/// What `seek` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeekTo {
    By(i64),
    At(u64),
}

fn parse_seek(text: &str) -> Option<SeekTo> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix('+') {
        return parse_time(rest).map(|ms| SeekTo::By(i64::try_from(ms).unwrap_or(i64::MAX)));
    }
    if let Some(rest) = text.strip_prefix('-') {
        return parse_time(rest).map(|ms| SeekTo::By(-i64::try_from(ms).unwrap_or(i64::MAX)));
    }
    parse_time(text).map(SeekTo::At)
}

/// Seeks in the playing track.
pub(crate) fn seek(args: &SeekArgs) -> Result<Outcome> {
    let Some(to) = parse_seek(&args.position) else {
        bail!(
            "{} is not a position: use 83, 1:23, +10 or -5",
            args.position
        );
    };
    let remote = Remote::connect()?;
    match to {
        SeekTo::By(ms) => remote.run("Seek", &(ms,))?,
        SeekTo::At(ms) => remote.run("SetPosition", &(ms,))?,
    }
    Ok(Outcome::Success)
}

/// What `volume` asks for, in percent.
#[derive(Debug, Clone, Copy, PartialEq)]
enum VolumeTo {
    By(f64),
    At(f64),
}

fn parse_volume(text: &str) -> Option<VolumeTo> {
    let text = text.trim().trim_end_matches('%');
    let number = |t: &str| t.parse::<f64>().ok().filter(|v| v.is_finite());
    if let Some(rest) = text.strip_prefix('+') {
        return number(rest).map(VolumeTo::By);
    }
    if let Some(rest) = text.strip_prefix('-') {
        return number(rest).map(|v| VolumeTo::By(-v));
    }
    number(text)
        .filter(|v| (0.0..=100.0).contains(v))
        .map(VolumeTo::At)
}

/// Prints or sets the volume.
pub(crate) fn volume(args: &VolumeArgs) -> Result<Outcome> {
    let remote = Remote::connect()?;
    let Some(level) = &args.level else {
        let status = remote.json("GetStatus")?;
        let volume = status["volume"].as_f64().unwrap_or(0.0);
        writeln!(io::stdout().lock(), "{}", (volume * 100.0).round())?;
        return Ok(Outcome::Success);
    };
    let Some(to) = parse_volume(level) else {
        bail!("{level} is not a volume: use 0 to 100, +5 or -5");
    };
    let percent = match to {
        VolumeTo::At(percent) => percent,
        VolumeTo::By(step) => {
            let status = remote.json("GetStatus")?;
            status["volume"].as_f64().unwrap_or(0.0) * 100.0 + step
        }
    };
    remote.run("SetVolume", &((percent / 100.0).clamp(0.0, 1.0),))?;
    Ok(Outcome::Success)
}

/// Sets the shuffle mode.
pub(crate) fn shuffle(args: &ShuffleArgs) -> Result<Outcome> {
    let name = match args.mode {
        ShuffleArg::Off => "off",
        ShuffleArg::Tracks => "tracks",
        ShuffleArg::Albums => "albums",
        ShuffleArg::Folders => "folders",
        ShuffleArg::Groupings => "groupings",
        ShuffleArg::WithinAlbum => "within-album",
    };
    Remote::connect()?.run("SetShuffle", &(name,))?;
    Ok(Outcome::Success)
}

/// Sets the repeat mode.
pub(crate) fn repeat(args: &RepeatArgs) -> Result<Outcome> {
    let name = match args.mode {
        RepeatArg::Off => "off",
        RepeatArg::One => "one",
        RepeatArg::Album => "album",
        RepeatArg::All => "all",
    };
    Remote::connect()?.run("SetRepeat", &(name,))?;
    Ok(Outcome::Success)
}

/// Adds files or the tracks of a query to the queue.
pub(crate) fn enqueue(args: &EnqueueArgs) -> Result<Outcome> {
    let remote = Remote::connect()?;
    let added: u32 = match &args.query {
        Some(query) => remote.call("EnqueueQuery", &(query.as_str(), args.next))?,
        None => {
            let mut files = Vec::with_capacity(args.files.len());
            for file in &args.files {
                let path = std::path::absolute(file)?;
                if !path.exists() {
                    bail!("{} does not exist", file.display());
                }
                let Some(text) = path.to_str() else {
                    bail!("{} is not a UTF-8 path", file.display());
                };
                files.push(text.to_owned());
            }
            remote.call("Enqueue", &(files, args.next))?
        }
    };
    writeln!(
        io::stdout().lock(),
        "{added} {} added",
        if added == 1 { "track" } else { "tracks" }
    )?;
    Ok(if added == 0 {
        Outcome::Failure
    } else {
        Outcome::Success
    })
}

/// Replaces the queue with the tracks of a query.
pub(crate) fn play_query(args: &PlayQueryArgs) -> Result<Outcome> {
    let query = args.query.join(" ");
    let remote = Remote::connect()?;
    let found: u32 = remote.call("PlayQuery", &(query.as_str(), args.shuffle))?;
    if found == 0 {
        writeln!(io::stderr().lock(), "helikodu-cli: no tracks found")?;
        return Ok(Outcome::Failure);
    }
    writeln!(
        io::stdout().lock(),
        "{found} {} playing",
        if found == 1 { "track" } else { "tracks" }
    )?;
    Ok(Outcome::Success)
}

/// Rates a track.
pub(crate) fn rate(args: &RateArgs) -> Result<Outcome> {
    let stars: i32 = match args.stars.trim() {
        "none" | "clear" => -1,
        text => match text.parse::<i32>() {
            Ok(n @ 0..=5) => n,
            _ => bail!("{text} is not a rating: use 0 to 5, or none"),
        },
    };
    Remote::connect()?.run("Rate", &(args.track.unwrap_or(0), stars))?;
    Ok(Outcome::Success)
}

/// Writes a playlist to a file.
pub(crate) fn export_playlist(args: &ExportArgs) -> Result<Outcome> {
    let path = std::path::absolute(&args.file)?;
    let Some(text) = path.to_str() else {
        bail!("{} is not a UTF-8 path", args.file.display());
    };
    let remote = Remote::connect()?;
    let written: u32 = remote.call("ExportPlaylist", &(args.playlist.as_str(), text))?;
    writeln!(
        io::stdout().lock(),
        "{written} {} written to {}",
        if written == 1 { "entry" } else { "entries" },
        path.display()
    )?;
    Ok(Outcome::Success)
}

fn print_json(out: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, value)?;
    writeln!(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_and_steps() {
        assert_eq!(parse_seek("83"), Some(SeekTo::At(83_000)));
        assert_eq!(parse_seek("1:23"), Some(SeekTo::At(83_000)));
        assert_eq!(parse_seek("1:02:03"), Some(SeekTo::At(3_723_000)));
        assert_eq!(parse_seek("+10"), Some(SeekTo::By(10_000)));
        assert_eq!(parse_seek("-0:05"), Some(SeekTo::By(-5_000)));
        assert_eq!(parse_seek("soon"), None);
        assert_eq!(parse_volume("50"), Some(VolumeTo::At(50.0)));
        assert_eq!(parse_volume("80%"), Some(VolumeTo::At(80.0)));
        assert_eq!(parse_volume("+5"), Some(VolumeTo::By(5.0)));
        assert_eq!(parse_volume("-5"), Some(VolumeTo::By(-5.0)));
        assert_eq!(parse_volume("150"), None);
    }
}
