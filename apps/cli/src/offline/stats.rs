// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli stats`.

use std::io::{self, Write};

use anyhow::Result;
use clap::Args;
use helikodu_service::library::{LibraryOverview, PlayedTrack};
use serde_json::{Value, json};

use super::database::{DatabaseArgs, path_object};
use super::format;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct StatsArgs {
    /// How many of the most played tracks to list.
    #[arg(long, value_name = "N", default_value_t = 10,
          value_parser = clap::value_parser!(u16).range(0..=1000))]
    top: u16,
    /// Print the statistics as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn run(args: &StatsArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let overview = library.overview(usize::from(args.top))?;
    let mut out = io::stdout().lock();
    if args.json {
        serde_json::to_writer_pretty(&mut out, &overview_json(&overview))?;
        writeln!(out)?;
    } else {
        print_overview(&mut out, &overview)?;
    }
    out.flush()?;
    Ok(Outcome::Success)
}

fn overview_json(overview: &LibraryOverview) -> Value {
    let t = &overview.totals;
    let roots: Vec<Value> = overview
        .roots
        .iter()
        .map(|root| path_object(root))
        .collect();
    let top: Vec<Value> = overview.most_played.iter().map(played_json).collect();
    json!({
        "roots": roots,
        "totals": {
            "roots": t.roots,
            "files": t.files,
            "tracks": t.tracks,
            "albums": t.albums,
            "artists": t.artists,
            "genres": t.genres,
            "duration_ms": format::millis(t.duration),
            "size_bytes": t.size,
            "missing_files": t.missing_files,
            "unreadable_files": t.unreadable_files,
            "plays": t.plays,
            "played_tracks": t.played_tracks,
            "rated_tracks": t.rated_tracks,
        },
        "most_played": top,
    })
}

fn played_json(track: &PlayedTrack) -> Value {
    json!({
        "track_id": track.track.get(),
        "title": track.title,
        "artist": track.artist,
        "album": track.album,
        "play_count": track.play_count,
        "last_played": track.last_played.map(format::unix_seconds),
    })
}

fn print_overview(out: &mut impl Write, overview: &LibraryOverview) -> io::Result<()> {
    let t = &overview.totals;
    writeln!(out, "Library folders:")?;
    for root in &overview.roots {
        writeln!(out, "  {}", root.display())?;
    }
    writeln!(
        out,
        "{} tracks in {} files ({}, {}), {} albums, {} artists, {} genres.",
        t.tracks,
        t.files,
        format::length(t.duration),
        format::size(t.size),
        t.albums,
        t.artists,
        t.genres
    )?;
    if t.missing_files > 0 || t.unreadable_files > 0 {
        writeln!(
            out,
            "{} files are gone, {} could not be read.",
            t.missing_files, t.unreadable_files
        )?;
    }
    writeln!(
        out,
        "{} plays; {} tracks played, {} rated.",
        t.plays, t.played_tracks, t.rated_tracks
    )?;
    if !overview.most_played.is_empty() {
        writeln!(out, "Most played:")?;
        for track in &overview.most_played {
            writeln!(
                out,
                "{:>6}x {:>7}  {} - {}",
                track.play_count,
                track.track.get(),
                track.artist.as_deref().unwrap_or("?"),
                track.title.as_deref().unwrap_or("?"),
            )?;
        }
    }
    Ok(())
}
