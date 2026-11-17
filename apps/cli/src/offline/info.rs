// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli info`.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Args;
use helikodu_model::library::TrackId;
use helikodu_model::player::Loudness;
use helikodu_service::library::{
    AudioProperties, LibraryEntry, TagOrigin, TrackInfo, TrackKind, TrackStats, TrackTags,
    read_file_info,
};
use serde_json::{Map, Value, json};

use super::database::{DatabaseArgs, insert_path};
use super::format;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct InfoArgs {
    /// A file, or the number of a library track (as `search` prints it). A file of
    /// that name wins over a track number.
    #[arg(value_name = "FILE|TRACK-ID")]
    target: PathBuf,
    /// Print the information as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn run(args: &InfoArgs) -> Result<Outcome> {
    let track_number = args
        .target
        .to_str()
        .filter(|_| !args.target.exists())
        .and_then(|text| text.parse::<u64>().ok());
    let info = match track_number {
        Some(number) => {
            let library = args.database.open_existing()?;
            match library.track_info(TrackId::new(number))? {
                Some(info) => info,
                None => bail!("there is no track {number} in the library"),
            }
        }
        None => {
            let database = args.database.path()?;
            if database.is_file() {
                args.database.open_existing()?.file_info(&args.target)?
            } else {
                read_file_info(&args.target)?
            }
        }
    };
    let mut out = io::stdout().lock();
    if args.json {
        serde_json::to_writer_pretty(&mut out, &info_json(&info))?;
        writeln!(out)?;
    } else {
        print_info(&mut out, &info)?;
    }
    out.flush()?;
    Ok(Outcome::Success)
}

fn info_json(info: &TrackInfo) -> Value {
    let mut object = Map::new();
    insert_path(&mut object, &info.path);
    object.insert("sub_index".into(), json!(info.sub_index));
    object.insert(
        "origin".into(),
        json!(match info.origin {
            TagOrigin::File => "file",
            TagOrigin::Library => "library",
        }),
    );
    object.insert("tags".into(), tags_json(&info.tags));
    object.insert("audio".into(), audio_json(&info.audio));
    object.insert(
        "loudness".into(),
        info.loudness.as_ref().map_or(Value::Null, loudness_json),
    );
    object.insert("file_error".into(), json!(info.file_error));
    object.insert(
        "library".into(),
        info.library.as_ref().map_or(Value::Null, entry_json),
    );
    Value::Object(object)
}

fn tags_json(tags: &TrackTags) -> Value {
    json!({
        "title": tags.title,
        "title_sort": tags.title_sort,
        "artists": tags.artists,
        "album_artists": tags.album_artists,
        "composers": tags.composers,
        "album": tags.album,
        "genres": tags.genres,
        "track_number": tags.track_no,
        "disc_number": tags.disc_no,
        "date": tags.date,
        "year": tags.year,
    })
}

fn audio_json(audio: &AudioProperties) -> Value {
    json!({
        "duration_ms": audio.duration.map(format::millis),
        "sample_rate": audio.sample_rate,
        "channels": audio.channels,
        "bit_depth": audio.bit_depth,
        "bitrate": audio.bitrate,
        "codec": audio.codec,
        "container": audio.container,
    })
}

fn loudness_json(loudness: &Loudness) -> Value {
    json!({
        "track_gain_db": loudness.track_gain_db,
        "track_peak": loudness.track_peak,
        "album_gain_db": loudness.album_gain_db,
        "album_peak": loudness.album_peak,
    })
}

fn entry_json(entry: &LibraryEntry) -> Value {
    json!({
        "track_id": entry.track.get(),
        "identity_id": entry.identity.get(),
        "kind": kind(entry.kind),
        "added_at": format::unix_seconds(entry.added_at),
        "missing_since": entry.missing_since.map(format::unix_seconds),
        "statistics": stats_json(&entry.stats),
    })
}

fn stats_json(stats: &TrackStats) -> Value {
    json!({
        "play_count": stats.play_count,
        "skip_count": stats.skip_count,
        "first_played": stats.first_played.map(format::unix_seconds),
        "last_played": stats.last_played.map(format::unix_seconds),
        "last_skipped": stats.last_skipped.map(format::unix_seconds),
        "rating": stats.rating.map(|rating| rating.half_stars()),
        "resume_position_ms": stats.resume_position.map(format::millis),
    })
}

fn kind(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Music => "music",
        TrackKind::Audiobook => "audiobook",
        TrackKind::Podcast => "podcast",
        TrackKind::AudioFromVideo => "audio_from_video",
        _ => "other",
    }
}

fn print_info(out: &mut impl Write, info: &TrackInfo) -> io::Result<()> {
    let mut line = |label: &str, value: String| writeln!(out, "{label:<14}{value}");
    line("File", info.path.display().to_string())?;
    if info.sub_index > 0 {
        line("Track in file", info.sub_index.to_string())?;
    }
    let tags = &info.tags;
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let list = |values: &[String]| values.join("; ");
    line("Title", text(&tags.title))?;
    line("Artists", list(&tags.artists))?;
    line("Album", text(&tags.album))?;
    line("Album artists", list(&tags.album_artists))?;
    line("Composers", list(&tags.composers))?;
    line("Genres", list(&tags.genres))?;
    let number = |value: Option<u32>| value.map(|n| n.to_string()).unwrap_or_default();
    line("Track", number(tags.track_no))?;
    line("Disc", number(tags.disc_no))?;
    line("Date", text(&tags.date))?;

    let audio = &info.audio;
    line(
        "Length",
        audio.duration.map(format::length).unwrap_or_default(),
    )?;
    let mut format_parts = Vec::new();
    format_parts.extend(audio.codec.clone());
    format_parts.extend(audio.sample_rate.map(|rate| format!("{rate} Hz")));
    format_parts.extend(audio.bit_depth.map(|bits| format!("{bits}-bit")));
    format_parts.extend(
        audio
            .channels
            .map(|channels| format!("{channels} channels")),
    );
    format_parts.extend(audio.bitrate.map(|bps| format!("{} kbit/s", bps / 1000)));
    line("Format", format_parts.join(", "))?;
    match &info.loudness {
        Some(loudness) => {
            let gain = |db: Option<f64>, peak: Option<f64>| match (db, peak) {
                (Some(db), Some(peak)) => format!("{db:+.2} dB, peak {peak:.6}"),
                (Some(db), None) => format!("{db:+.2} dB"),
                _ => "none".to_owned(),
            };
            line(
                "Track gain",
                gain(loudness.track_gain_db, loudness.track_peak),
            )?;
            line(
                "Album gain",
                gain(loudness.album_gain_db, loudness.album_peak),
            )?;
        }
        None => line(
            "File",
            format!(
                "cannot be read: {}",
                info.file_error.as_deref().unwrap_or("unknown error")
            ),
        )?,
    }
    let origin = match info.origin {
        TagOrigin::File => "the file",
        TagOrigin::Library => "the library",
    };
    line("Tags from", origin.to_owned())?;
    match &info.library {
        None => line("Library", "not in the library".to_owned())?,
        Some(entry) => {
            line(
                "Library",
                format!(
                    "track {}, identity {}, added {}",
                    entry.track.get(),
                    entry.identity.get(),
                    format::utc(entry.added_at)
                ),
            )?;
            if let Some(since) = entry.missing_since {
                line("Missing since", format::utc(since))?;
            }
            let stats = &entry.stats;
            line(
                "Plays",
                format!("{} (skipped {})", stats.play_count, stats.skip_count),
            )?;
            if let Some(last) = stats.last_played {
                line("Last played", format::utc(last))?;
            }
            line(
                "Rating",
                stats
                    .rating
                    .map_or_else(|| "not rated".to_owned(), |rating| rating.to_string()),
            )?;
        }
    }
    Ok(())
}
