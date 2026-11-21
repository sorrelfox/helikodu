// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli similar`.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Args;
use helikodu_model::library::TrackId;
use helikodu_service::library::{MusicLibrary, SimilarTrack, SimilaritySeed};
use serde_json::{Map, Value, json};

use super::database::{DatabaseArgs, insert_path};
use super::format;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct SimilarArgs {
    /// A library file, or the number of a library track (as `search` prints it).
    /// A file of that name wins over a track number.
    #[arg(value_name = "FILE|TRACK-ID")]
    target: PathBuf,
    /// Find the tracks most like the album of that track (its own tracks are left
    /// out).
    #[arg(long, conflicts_with = "artist")]
    album: bool,
    /// Find the tracks most like the first artist of that track (the artist's own
    /// tracks are left out).
    #[arg(long)]
    artist: bool,
    /// The most tracks to print.
    #[arg(long, value_name = "N", default_value_t = 20,
          value_parser = clap::value_parser!(u16).range(1..=1000))]
    limit: u16,
    /// Print the results as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn run(args: &SimilarArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let track = target_track(&library, args)?;
    let seed = if args.album || args.artist {
        let keys = library.play_keys(&[track])?;
        let keys = keys.get(&track);
        if args.album {
            match keys.and_then(|k| k.album) {
                Some(album) => SimilaritySeed::Album(album),
                None => bail!("track {} has no album", track.get()),
            }
        } else {
            match keys.and_then(|k| k.artist) {
                Some(artist) => SimilaritySeed::Artist(artist),
                None => bail!("track {} has no artist", track.get()),
            }
        }
    } else {
        SimilaritySeed::Track(track)
    };
    let found = library.similar_tracks(seed, usize::from(args.limit))?;
    let mut out = io::stdout().lock();
    if args.json {
        let tracks = found
            .iter()
            .map(|similar| similar_json(&library, similar))
            .collect::<Result<Vec<Value>>>()?;
        let value = json!({
            "seed": seed_json(seed, track),
            "tracks": tracks,
        });
        serde_json::to_writer_pretty(&mut out, &value)?;
        writeln!(out)?;
    } else if found.is_empty() {
        writeln!(
            out,
            "No similar tracks: the track is not analysed yet (run `helikodu-cli analyze`), \
             or it is silent."
        )?;
    } else {
        for similar in &found {
            let details = library.track_details(similar.track)?;
            let tags = details.as_ref().map(|d| &d.tags);
            let length = details
                .as_ref()
                .and_then(|d| d.audio.duration)
                .map(format::length)
                .unwrap_or_default();
            writeln!(
                out,
                "{:>7}  {:.2}  {} - {}{}  {length}",
                similar.track.get(),
                similar.similarity,
                tags.and_then(|t| t.artists.first().map(String::as_str))
                    .unwrap_or("?"),
                tags.and_then(|t| t.title.clone())
                    .or_else(|| details
                        .as_ref()
                        .and_then(|d| d.path.file_name())
                        .map(|name| name.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "?".into()),
                tags.and_then(|t| t.album.as_deref())
                    .map(|album| format!(" [{album}]"))
                    .unwrap_or_default(),
            )?;
        }
    }
    out.flush()?;
    Ok(if found.is_empty() {
        Outcome::Failure
    } else {
        Outcome::Success
    })
}

/// The library track `args` names: a file, or a track number.
fn target_track(library: &MusicLibrary, args: &SimilarArgs) -> Result<TrackId> {
    let number = args
        .target
        .to_str()
        .filter(|_| !args.target.exists())
        .and_then(|text| text.parse::<u64>().ok());
    if let Some(number) = number {
        let track = TrackId::new(number);
        if library.track_details(track)?.is_none() {
            bail!("there is no track {number} in the library");
        }
        return Ok(track);
    }
    let candidates = [
        Some(args.target.clone()),
        std::path::absolute(&args.target).ok(),
        std::fs::canonicalize(&args.target).ok(),
    ];
    for path in candidates.into_iter().flatten() {
        if let Some(track) = library.track_at(&path)? {
            return Ok(track);
        }
    }
    bail!("{} is not a library track", args.target.display())
}

fn seed_json(seed: SimilaritySeed, track: TrackId) -> Value {
    match seed {
        SimilaritySeed::Album(album) => {
            json!({"track_id": track.get(), "album_id": album.get()})
        }
        SimilaritySeed::Artist(artist) => {
            json!({"track_id": track.get(), "artist_id": artist.get()})
        }
        SimilaritySeed::Track(_) => json!({"track_id": track.get()}),
    }
}

fn similar_json(library: &MusicLibrary, similar: &SimilarTrack) -> Result<Value> {
    let mut object = Map::new();
    object.insert("track_id".into(), json!(similar.track.get()));
    object.insert("similarity".into(), json!(similar.similarity));
    match library.track_details(similar.track)? {
        Some(details) => {
            object.insert("title".into(), json!(details.tags.title));
            object.insert("artist".into(), json!(details.tags.artists.first()));
            object.insert("album".into(), json!(details.tags.album));
            insert_path(&mut object, &details.path);
        }
        None => {
            object.insert("path".into(), Value::Null);
        }
    }
    Ok(Value::Object(object))
}
