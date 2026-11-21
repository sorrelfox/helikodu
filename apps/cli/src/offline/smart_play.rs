// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli related`, `auto-dj` and `generator`: what endless play and the
//! Auto-DJ would add, and the generator playlists, over the library directly.
//! `generator play` hands the result to the running application.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use helikodu_model::library::TrackId;
use helikodu_service::library::{
    AlbumId, Generator, GeneratorSteps, MusicLibrary, NoSoundSimilarity, PersonId, PlaySeed,
    time_seed,
};
use serde_json::{Value, json};

use super::database::{DatabaseArgs, app_settings};
use super::format;
use crate::Outcome;
use crate::remote::Remote;

/// What the related tracks start from: exactly one.
#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub(crate) struct SeedArgs {
    /// A library track, by its number (as `search` shows it).
    #[arg(long, value_name = "ID")]
    track: Option<u64>,
    /// An album, by its number.
    #[arg(long, value_name = "ID")]
    album: Option<i64>,
    /// An artist, by their number.
    #[arg(long, value_name = "ID")]
    artist: Option<i64>,
}

impl SeedArgs {
    pub(crate) fn seed(&self) -> PlaySeed {
        match (self.track, self.album, self.artist) {
            (Some(track), _, _) => PlaySeed::Track(TrackId::new(track)),
            (_, Some(album), _) => PlaySeed::Album(AlbumId::from_raw(album)),
            (_, _, Some(artist)) => PlaySeed::Artist(PersonId::from_raw(artist)),
            _ => unreachable!("clap requires one"),
        }
    }

    /// The kind and number as the D-Bus interface takes them.
    pub(crate) fn remote(&self) -> (&'static str, u64) {
        match (self.track, self.album, self.artist) {
            (Some(track), _, _) => ("track", track),
            (_, Some(album), _) => ("album", u64::try_from(album).unwrap_or(0)),
            (_, _, Some(artist)) => ("artist", u64::try_from(artist).unwrap_or(0)),
            _ => unreachable!("clap requires one"),
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct RelatedArgs {
    #[command(flatten)]
    seed: SeedArgs,
    /// How many tracks to pick.
    #[arg(long, value_name = "N", default_value_t = 20)]
    count: usize,
    /// The random seed; the same one picks the same tracks. From the time by
    /// default.
    #[arg(long, value_name = "N")]
    random_seed: Option<u64>,
    /// Print the tracks as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

#[derive(Debug, Args)]
pub(crate) struct AutoDjArgs {
    /// How many tracks to pick; `playback.auto_dj.ahead` by default.
    #[arg(long, value_name = "N")]
    count: Option<usize>,
    /// The random seed; the same one picks the same tracks.
    #[arg(long, value_name = "N")]
    random_seed: Option<u64>,
    /// Print the tracks as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

/// `yes` or `no`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum YesNo {
    Yes,
    No,
}

#[derive(Debug, Args)]
pub(crate) struct GeneratorArgs {
    #[command(subcommand)]
    command: GeneratorCommand,
    #[command(flatten)]
    database: DatabaseArgs,
}

/// The steps, as text or from a file (`-` reads them from the standard input).
#[derive(Debug, Args)]
#[group(multiple = false)]
struct StepsArgs {
    /// The steps, one per line (see the user guide).
    #[arg(long, value_name = "TEXT")]
    steps: Option<String>,
    /// A file with the steps; `-` for the standard input.
    #[arg(long, value_name = "FILE")]
    steps_file: Option<PathBuf>,
}

impl StepsArgs {
    fn read(&self) -> Result<Option<GeneratorSteps>> {
        let text = match (&self.steps, &self.steps_file) {
            (Some(text), _) => text.clone(),
            (_, Some(path)) if path.as_os_str() == "-" => {
                let mut text = String::new();
                io::stdin().read_to_string(&mut text)?;
                text
            }
            (_, Some(path)) => fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?,
            _ => return Ok(None),
        };
        Ok(Some(text.parse()?))
    }
}

#[derive(Debug, Subcommand)]
enum GeneratorCommand {
    /// List the generator playlists.
    List {
        /// Print them as JSON, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Show a generator's steps, in the text form `add` and `edit` take.
    Show {
        /// The generator's name or number.
        generator: String,
    },
    /// Add a generator playlist.
    Add {
        /// Its name.
        name: String,
        #[command(flatten)]
        steps: StepsArgs,
        /// Keep the result until `run` makes it again, instead of making it
        /// again each time it is opened.
        #[arg(long)]
        keep: bool,
    },
    /// Change a generator's name, steps or whether it is made again on open.
    Edit {
        /// The generator's name or number.
        generator: String,
        /// A new name.
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        steps: StepsArgs,
        /// Whether it is made again each time it is opened.
        #[arg(long, value_enum)]
        on_open: Option<YesNo>,
    },
    /// Remove a generator playlist.
    Remove {
        /// The generator's name or number.
        generator: String,
    },
    /// Make a generator's tracks again, store and print them.
    Run {
        /// The generator's name or number.
        generator: String,
        /// The seed of `sort random`; from the time by default.
        #[arg(long, value_name = "N")]
        random_seed: Option<u64>,
        /// Print the tracks as JSON, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Print a generator's tracks as opening it shows them (made again when it
    /// is made on open).
    Open {
        /// The generator's name or number.
        generator: String,
        /// Print the tracks as JSON, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Run steps without storing anything.
    Preview {
        #[command(flatten)]
        steps: StepsArgs,
        /// The seed of `sort random`.
        #[arg(long, value_name = "N")]
        random_seed: Option<u64>,
        /// Print the tracks as JSON, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Open a generator and play its tracks in the running application,
    /// replacing its queue.
    Play {
        /// The generator's name or number.
        generator: String,
    },
}

/// Prints `tracks` in order, one per line, or as JSON.
fn print_tracks(library: &MusicLibrary, tracks: &[TrackId], json: bool) -> Result<()> {
    let summaries = library.summaries_of(tracks)?;
    let mut out = io::stdout().lock();
    if json {
        let list: Vec<Value> = summaries
            .iter()
            .map(|t| {
                json!({
                    "track_id": t.id.get(),
                    "title": t.title,
                    "artist": t.artist,
                    "album": t.album_title,
                    "year": t.year,
                    "duration_ms": t.duration.map(format::millis),
                })
            })
            .collect();
        serde_json::to_writer_pretty(&mut out, &list)?;
        writeln!(out)?;
    } else {
        for t in &summaries {
            writeln!(
                out,
                "{:>7}  {} - {}{}  {}",
                t.id.get(),
                t.artist.as_deref().unwrap_or("?"),
                t.title.as_deref().unwrap_or("?"),
                t.album_title
                    .as_deref()
                    .map(|album| format!(" [{album}]"))
                    .unwrap_or_default(),
                t.duration.map(format::length).unwrap_or_default(),
            )?;
        }
        if summaries.is_empty() {
            writeln!(out, "No tracks.")?;
        }
    }
    out.flush()?;
    Ok(())
}

pub(crate) fn related(args: &RelatedArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let settings = app_settings().playback.endless;
    let tracks = library.related_tracks(
        args.seed.seed(),
        &settings,
        &HashSet::new(),
        args.count,
        args.random_seed.unwrap_or_else(time_seed),
        &NoSoundSimilarity,
    )?;
    print_tracks(&library, &tracks, args.json)?;
    Ok(Outcome::Success)
}

pub(crate) fn auto_dj(args: &AutoDjArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let settings = app_settings().playback.auto_dj;
    let count = args.count.unwrap_or(settings.ahead as usize);
    let tracks = library.auto_dj_tracks(
        &settings,
        &HashSet::new(),
        count,
        args.random_seed.unwrap_or_else(time_seed),
    )?;
    print_tracks(&library, &tracks, args.json)?;
    Ok(Outcome::Success)
}

fn find(library: &MusicLibrary, name: &str) -> Result<Generator> {
    match library.find_generator(name)? {
        Some(generator) => Ok(generator),
        None => bail!("there is no generator {name:?}"),
    }
}

pub(crate) fn generator(args: &GeneratorArgs) -> Result<Outcome> {
    let library = args.database.open()?;
    match &args.command {
        GeneratorCommand::List { json } => {
            let generators = library.generators()?;
            let mut out = io::stdout().lock();
            if *json {
                let list: Vec<Value> = generators
                    .iter()
                    .map(|g| {
                        json!({
                            "id": g.id.get(),
                            "name": g.name,
                            "on_open": g.on_open,
                            "tracks": g.tracks.as_ref().map(Vec::len),
                            "generated": g.generated.map(format::utc),
                            "steps": g.steps.to_string(),
                        })
                    })
                    .collect();
                serde_json::to_writer_pretty(&mut out, &list)?;
                writeln!(out)?;
            } else {
                for g in &generators {
                    let tracks = g.tracks.as_ref().map_or_else(
                        || "not made yet".to_owned(),
                        |t| format!("{} tracks", t.len()),
                    );
                    let when = if g.on_open {
                        "made on open"
                    } else {
                        "kept until run"
                    };
                    writeln!(out, "{:>5}  {}  ({tracks}, {when})", g.id.get(), g.name)?;
                }
                if generators.is_empty() {
                    writeln!(out, "No generator playlists.")?;
                }
            }
        }
        GeneratorCommand::Show { generator } => {
            print!("{}", find(&library, generator)?.steps);
        }
        GeneratorCommand::Add { name, steps, keep } => {
            let steps = steps.read()?.unwrap_or_default();
            let id = library.create_generator(name, &steps, !keep)?;
            println!("Added the generator {name:?} ({id}).");
        }
        GeneratorCommand::Edit {
            generator,
            name,
            steps,
            on_open,
        } => {
            let found = find(&library, generator)?;
            let steps = steps.read()?;
            library.update_generator(
                found.id,
                name.as_deref(),
                steps.as_ref(),
                on_open.map(|o| o == YesNo::Yes),
            )?;
            println!(
                "Changed the generator {:?}.",
                name.as_deref().unwrap_or(&found.name)
            );
        }
        GeneratorCommand::Remove { generator } => {
            let found = find(&library, generator)?;
            library.remove_generator(found.id)?;
            println!("Removed the generator {:?}.", found.name);
        }
        GeneratorCommand::Run {
            generator,
            random_seed,
            json,
        } => {
            let found = find(&library, generator)?;
            let tracks = library.regenerate(found.id, random_seed.unwrap_or_else(time_seed))?;
            print_tracks(&library, &tracks, *json)?;
        }
        GeneratorCommand::Open { generator, json } => {
            let found = find(&library, generator)?;
            let tracks = library.open_generator(found.id, time_seed())?;
            print_tracks(&library, &tracks, *json)?;
        }
        GeneratorCommand::Preview {
            steps,
            random_seed,
            json,
        } => {
            let Some(steps) = steps.read()? else {
                bail!("give the steps with --steps or --steps-file");
            };
            let tracks =
                library.preview_generator(&steps, random_seed.unwrap_or_else(time_seed))?;
            print_tracks(&library, &tracks, *json)?;
        }
        GeneratorCommand::Play { generator } => {
            let found = find(&library, generator)?;
            let tracks = library.open_generator(found.id, time_seed())?;
            if tracks.is_empty() {
                bail!("the generator {:?} has no tracks", found.name);
            }
            let ids: Vec<u64> = tracks.iter().map(|t| t.get()).collect();
            let count: u32 = Remote::connect()?.call("PlayTracks", &(ids, found.name.clone()))?;
            println!("Playing {count} tracks of {:?}.", found.name);
        }
    }
    Ok(Outcome::Success)
}
