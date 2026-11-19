// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli labels`: the labels of the library's tracks, albums and artists.
//! They live in the library and are never written to the files.

use std::io::{self, Write};

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use helikodu_model::library::TrackId;
use helikodu_service::library::{LabelTarget, MusicLibrary, parse_query};
use serde_json::{Value, json};

use super::database::DatabaseArgs;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct LabelsArgs {
    #[command(subcommand)]
    command: LabelsCommand,
    #[command(flatten)]
    database: DatabaseArgs,
}

#[derive(Debug, Subcommand)]
enum LabelsCommand {
    /// List the labels with how many tracks, albums and artists have them.
    List {
        /// Print the labels as JSON, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Show the labels of tracks, an album or an artist.
    Show {
        #[command(flatten)]
        target: TargetArgs,
        /// Print the labels as JSON, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Give a label to tracks, an album or an artist. A label of an album or an
    /// artist counts for all of its tracks.
    Add {
        /// The label.
        label: String,
        #[command(flatten)]
        target: TargetArgs,
    },
    /// Take a label from tracks, an album or an artist.
    Remove {
        /// The label.
        label: String,
        #[command(flatten)]
        target: TargetArgs,
    },
    /// Rename a label.
    Rename {
        /// The label.
        label: String,
        /// Its new name, which no other label may have.
        new_name: String,
    },
    /// Merge a label into another: what has the first has the second instead.
    Merge {
        /// The label that goes.
        from: String,
        /// The label that stays.
        into: String,
    },
    /// Remove a label from everything that has it.
    Delete {
        /// The label.
        label: String,
    },
}

/// What a label is given to or taken from: exactly one kind.
#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
struct TargetArgs {
    /// Library tracks, by the ids `search` and `info` print.
    #[arg(long = "track", value_name = "ID", num_args = 1..)]
    tracks: Vec<u64>,
    /// The library tracks a query finds (the search language of the app).
    #[arg(long, value_name = "QUERY")]
    query: Option<String>,
    /// The album of a library track, by the track's id.
    #[arg(long = "album-of", value_name = "ID")]
    album_of: Option<u64>,
    /// An artist, by name.
    #[arg(long, value_name = "NAME")]
    artist: Option<String>,
}

impl TargetArgs {
    fn target(&self, library: &MusicLibrary) -> Result<LabelTarget> {
        if let Some(text) = &self.query {
            let query = parse_query(text)?;
            return Ok(LabelTarget::Tracks(library.query_track_ids(&query, 0)?));
        }
        if let Some(track) = self.album_of {
            return Ok(LabelTarget::AlbumOf(TrackId::new(track)));
        }
        if let Some(artist) = &self.artist {
            return Ok(LabelTarget::Artist(artist.clone()));
        }
        if self.tracks.is_empty() {
            bail!("name the tracks, the album or the artist");
        }
        Ok(LabelTarget::Tracks(
            self.tracks.iter().copied().map(TrackId::new).collect(),
        ))
    }
}

pub(crate) fn run(args: &LabelsArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let mut out = io::stdout().lock();
    match &args.command {
        LabelsCommand::List { json } => {
            let labels = library.labels()?;
            if *json {
                let labels: Vec<Value> = labels
                    .iter()
                    .map(|l| {
                        json!({
                            "name": l.name,
                            "tracks": l.tracks,
                            "own_tracks": l.own_tracks,
                            "albums": l.albums,
                            "artists": l.artists,
                        })
                    })
                    .collect();
                serde_json::to_writer_pretty(&mut out, &json!({ "labels": labels }))?;
                writeln!(out)?;
            } else if labels.is_empty() {
                writeln!(out, "No labels.")?;
            } else {
                for l in &labels {
                    writeln!(
                        out,
                        "{}  ({} tracks; given to {} tracks, {} albums, {} artists)",
                        l.name, l.tracks, l.own_tracks, l.albums, l.artists
                    )?;
                }
            }
        }
        LabelsCommand::Show { target, json } => {
            let labels = library.labels_of(&target.target(&library)?)?;
            if *json {
                let value = json!({ "own": labels.own, "inherited": labels.inherited });
                serde_json::to_writer_pretty(&mut out, &value)?;
                writeln!(out)?;
            } else {
                for name in &labels.own {
                    writeln!(out, "{name}")?;
                }
                for name in &labels.inherited {
                    writeln!(out, "{name}  (from the album or an artist)")?;
                }
            }
        }
        LabelsCommand::Add { label, target } => {
            let changed = library.add_label(&target.target(&library)?, label)?;
            writeln!(out, "Labelled {changed} more.")?;
        }
        LabelsCommand::Remove { label, target } => {
            let changed = library.remove_label(&target.target(&library)?, label)?;
            writeln!(out, "Took the label from {changed}.")?;
        }
        LabelsCommand::Rename { label, new_name } => library.rename_label(label, new_name)?,
        LabelsCommand::Merge { from, into } => library.merge_labels(from, into)?,
        LabelsCommand::Delete { label } => library.delete_label(label)?,
    }
    out.flush()?;
    Ok(Outcome::Success)
}
