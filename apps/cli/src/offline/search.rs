// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli search`.

use std::io::{self, Write};

use anyhow::Result;
use clap::Args;
use helikodu_service::library::{FoundTrack, MatchRank};
use serde_json::{Map, Value, json};

use super::database::{DatabaseArgs, insert_path};
use super::format;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct SearchArgs {
    /// The text to find in titles, artists, albums, genres, composers and paths.
    /// Several words must all match.
    #[arg(required = true, value_name = "TEXT")]
    text: Vec<String>,
    /// The most results to print.
    #[arg(long, value_name = "N", default_value_t = 50,
          value_parser = clap::value_parser!(u16).range(1..=1000))]
    limit: u16,
    /// How many of the best results to leave out, to page through them.
    #[arg(long, value_name = "N", default_value_t = 0)]
    offset: usize,
    /// Print the results as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn run(args: &SearchArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let text = args.text.join(" ");
    let limit = usize::from(args.limit);
    let page = library.search(&text, args.offset, limit)?;
    let mut out = io::stdout().lock();
    if args.json {
        let hits: Vec<Value> = page.hits.iter().map(hit_json).collect();
        let value = json!({
            "query": text,
            "total": page.total,
            "offset": args.offset,
            "limit": limit,
            "hits": hits,
        });
        serde_json::to_writer_pretty(&mut out, &value)?;
        writeln!(out)?;
    } else {
        for found in &page.hits {
            let hit = &found.hit;
            let length = hit.duration.map(format::length).unwrap_or_default();
            writeln!(
                out,
                "{:>7}  {} - {}{}  {length}",
                hit.track.get(),
                hit.artist.as_deref().unwrap_or("?"),
                hit.title.as_deref().unwrap_or("?"),
                hit.album
                    .as_deref()
                    .map(|album| format!(" [{album}]"))
                    .unwrap_or_default(),
            )?;
        }
        let first = args.offset.saturating_add(1);
        let last = args.offset.saturating_add(page.hits.len());
        if page.hits.is_empty() {
            writeln!(out, "No tracks found ({} in all).", page.total)?;
        } else {
            writeln!(out, "Tracks {first} to {last} of {}.", page.total)?;
        }
    }
    out.flush()?;
    Ok(Outcome::Success)
}

fn hit_json(found: &FoundTrack) -> Value {
    let hit = &found.hit;
    let mut object = Map::new();
    object.insert("track_id".into(), json!(hit.track.get()));
    object.insert("rank".into(), json!(rank(hit.rank)));
    object.insert("title".into(), json!(hit.title));
    object.insert("artist".into(), json!(hit.artist));
    object.insert("album".into(), json!(hit.album));
    object.insert(
        "duration_ms".into(),
        json!(hit.duration.map(format::millis)),
    );
    match &found.path {
        Some(path) => insert_path(&mut object, path),
        None => {
            object.insert("path".into(), Value::Null);
        }
    }
    Value::Object(object)
}

fn rank(rank: MatchRank) -> &'static str {
    match rank {
        MatchRank::Exact => "exact",
        MatchRank::Prefix => "prefix",
        MatchRank::WordPrefix => "word_prefix",
        MatchRank::Substring => "substring",
        MatchRank::Terms => "terms",
    }
}
