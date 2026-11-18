// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli loudness`.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Args;
use helikodu_model::library::TrackId;
use helikodu_service::jobs::loudness::{
    LoudnessEvent, LoudnessRequest, LoudnessSummary, TagOutcome, TrackResult, run as measure,
};
use helikodu_service::library::{
    AlbumMeasurement, CancelToken, LoudnessSelection, MeasuredLoudness, MusicLibrary,
};
use serde_json::{Map, Value, json};

use super::database::{DatabaseArgs, insert_path};
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct LoudnessArgs {
    /// Library tracks: files in the library, or track numbers (as `search` prints
    /// them). A file of that name wins over a track number.
    #[arg(
        value_name = "FILE|TRACK-ID",
        required_unless_present = "library",
        conflicts_with = "library"
    )]
    targets: Vec<PathBuf>,
    /// Measure the whole albums of the tracks given, with album gain.
    #[arg(long)]
    album: bool,
    /// Measure every library track without a current measurement, with the other
    /// tracks of their albums for the album gain.
    #[arg(long, conflicts_with = "album")]
    library: bool,
    /// Also write the values into the files' tags (ReplayGain 2.0). Without it they
    /// are kept in the library only.
    #[arg(long)]
    write_tags: bool,
    /// The number of measuring threads [default: half the processors]
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..=64))]
    threads: Option<u16>,
    /// The scan worker program that reads written files again [default:
    /// helikodu-scan-worker beside this program]
    #[arg(long, value_name = "PATH")]
    worker: Option<PathBuf>,
    /// Print the results as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn run(args: LoudnessArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let selection = if args.library {
        LoudnessRequest::unmeasured().selection
    } else {
        let tracks = resolve(&library, &args.targets)?;
        if args.album {
            albums_of(&library, tracks)?
        } else {
            LoudnessSelection::Tracks(tracks)
        }
    };
    let mut request = LoudnessRequest::new(selection);
    request.write_tags = args.write_tags;
    request.threads = args.threads.map_or(0, usize::from);
    request.worker_program = args.worker;

    let terminal = io::stderr().is_terminal();
    let summary = measure(&library, &request, &CancelToken::new(), |event| {
        progress(&event, terminal);
    });
    if terminal {
        eprint!("\r\x1b[2K");
    }
    if let Some(error) = &summary.error {
        bail!("the measurement failed: {error}");
    }

    let mut out = io::stdout().lock();
    if args.json {
        serde_json::to_writer_pretty(&mut out, &summary_json(&summary))?;
        writeln!(out)?;
    } else {
        print_summary(&mut out, &summary)?;
    }
    out.flush()?;
    let tags_failed = summary
        .results
        .iter()
        .any(|r| matches!(r.tags, TagOutcome::Failed(_)));
    Ok(if summary.failed > 0 || tags_failed {
        Outcome::Failure
    } else {
        Outcome::Success
    })
}

/// The library tracks the targets name.
fn resolve(library: &MusicLibrary, targets: &[PathBuf]) -> Result<Vec<TrackId>> {
    let mut tracks = Vec::new();
    for target in targets {
        let number = target
            .to_str()
            .filter(|_| !target.exists())
            .and_then(|text| text.parse::<u64>().ok());
        let track = match number {
            Some(number) => {
                let track = TrackId::new(number);
                if library.track_source(track)?.is_none() {
                    bail!("there is no track {number} in the library");
                }
                track
            }
            None => {
                let path = std::path::absolute(target).unwrap_or_else(|_| target.clone());
                match library.track_at(&path)? {
                    Some(track) => track,
                    None => bail!("{} is not in the library", target.display()),
                }
            }
        };
        tracks.push(track);
    }
    Ok(tracks)
}

/// The albums of `tracks`.
fn albums_of(library: &MusicLibrary, tracks: Vec<TrackId>) -> Result<LoudnessSelection> {
    let targets = library.loudness_targets(&LoudnessSelection::Tracks(tracks.clone()))?;
    let mut albums = Vec::new();
    for track in tracks {
        match targets.iter().find(|t| t.track == track) {
            Some(target) => match target.album {
                Some(album) if !albums.contains(&album) => albums.push(album),
                Some(_) => {}
                None => bail!("track {track} has no album"),
            },
            None => bail!("the file of track {track} is missing"),
        }
    }
    Ok(LoudnessSelection::Albums(albums))
}

/// Progress on standard error: a line that updates itself on a terminal,
/// otherwise only the failures.
fn progress(event: &LoudnessEvent, terminal: bool) {
    let mut err = io::stderr().lock();
    match event {
        LoudnessEvent::Progress { done, total, .. } if terminal => {
            let _ = write!(err, "\r\x1b[2KMeasured {done} of {total} tracks");
            let _ = err.flush();
        }
        LoudnessEvent::Stored(results) => {
            for result in results {
                if let Err(error) = &result.measured {
                    let clear = if terminal { "\r\x1b[2K" } else { "" };
                    let _ = writeln!(
                        err,
                        "{clear}Cannot measure {}: {error}",
                        result.path.display()
                    );
                }
            }
        }
        _ => {}
    }
}

fn summary_json(summary: &LoudnessSummary) -> Value {
    json!({
        "total": summary.total,
        "measured": summary.measured,
        "failed": summary.failed,
        "tags_written": summary.tags_written,
        "cancelled": summary.cancelled,
        "tracks": summary.results.iter().map(track_json).collect::<Vec<_>>(),
    })
}

fn track_json(result: &TrackResult) -> Value {
    let mut object = Map::new();
    object.insert("track_id".into(), json!(result.track.get()));
    insert_path(&mut object, &result.path);
    object.insert("album_id".into(), json!(result.album.map(|a| a.get())));
    match &result.measured {
        Ok(measured) => {
            object.insert("loudness".into(), measured_json(measured));
            object.insert("error".into(), Value::Null);
        }
        Err(error) => {
            object.insert("loudness".into(), Value::Null);
            object.insert("error".into(), json!(error));
        }
    }
    let (tags, reason) = match &result.tags {
        TagOutcome::NotAsked => ("not_asked", None),
        TagOutcome::Written => ("written", None),
        TagOutcome::Skipped(reason) => ("skipped", Some(reason)),
        TagOutcome::Failed(reason) => ("failed", Some(reason)),
    };
    object.insert("tags".into(), json!(tags));
    object.insert("tags_reason".into(), json!(reason));
    Value::Object(object)
}

fn measured_json(measured: &MeasuredLoudness) -> Value {
    json!({
        "integrated_lufs": measured.integrated_lufs,
        "loudness_range_lu": measured.range_lu,
        "sample_peak": measured.sample_peak,
        "true_peak": measured.true_peak,
        "track_gain_db": measured.track_gain_db(),
        "album": measured.album.as_ref().map(album_json),
        "version": measured.version,
    })
}

fn album_json(album: &AlbumMeasurement) -> Value {
    json!({
        "integrated_lufs": album.integrated_lufs,
        "loudness_range_lu": album.range_lu,
        "sample_peak": album.sample_peak,
        "true_peak": album.true_peak,
        "gain_db": album.gain_db(),
    })
}

fn print_summary(out: &mut impl Write, summary: &LoudnessSummary) -> io::Result<()> {
    let db = |linear: f64| 20.0 * linear.log10();
    for result in &summary.results {
        match &result.measured {
            Ok(m) => {
                let lufs = m
                    .integrated_lufs
                    .map_or_else(|| "silent".to_owned(), |l| format!("{l:.2} LUFS"));
                let gain = m
                    .track_gain_db()
                    .map_or_else(|| "-".to_owned(), |g| format!("{g:+.2} dB"));
                write!(
                    out,
                    "{}: {lufs}, LRA {:.1} LU, true peak {:.2} dBTP, gain {gain}",
                    result.path.display(),
                    m.range_lu,
                    db(m.true_peak),
                )?;
                if let Some(gain) = m.album.and_then(|a| a.gain_db()) {
                    write!(out, ", album gain {gain:+.2} dB")?;
                }
                if result.tags == TagOutcome::Written {
                    write!(out, " (written to tags)")?;
                }
                writeln!(out)?;
            }
            Err(error) => writeln!(out, "{}: {error}", result.path.display())?,
        }
    }
    writeln!(
        out,
        "{} of {} tracks measured, {} failed, {} written to tags",
        summary.measured, summary.total, summary.failed, summary.tags_written
    )
}
