// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli analyze`.

use std::io::{self, IsTerminal, Write};

use anyhow::{Result, bail};
use clap::Args;
use helikodu_service::jobs::analysis::{
    AnalysisEvent, AnalysisOptions, AnalysisSummary, coverage, run as analyze,
};
use helikodu_service::library::{AnalysisCoverage, CancelToken};
use serde_json::json;

use super::database::DatabaseArgs;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct AnalyzeArgs {
    /// Do not measure the loudness in the same pass (by default it is measured for
    /// tracks that have no loudness values at all).
    #[arg(long)]
    no_loudness: bool,
    /// Print the results as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn run(args: &AnalyzeArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let options = AnalysisOptions {
        measure_loudness: !args.no_loudness,
        ..AnalysisOptions::default()
    };
    let terminal = io::stderr().is_terminal();
    let mut done = 0;
    let mut total = 0;
    let summary = analyze(&library, &options, &CancelToken::new(), |event| {
        progress(&event, terminal, &mut done, &mut total);
    });
    if terminal {
        eprint!("\r\x1b[2K");
    }
    if let Some(error) = &summary.error {
        bail!("the analysis failed: {error}");
    }
    let after = coverage(&library)?;
    let mut out = io::stdout().lock();
    if args.json {
        serde_json::to_writer_pretty(&mut out, &summary_json(&summary, &after))?;
        writeln!(out)?;
    } else {
        writeln!(
            out,
            "{} tracks analysed, {} failed, {} measured for loudness; \
             {} of {} library tracks analysed, {} failed",
            summary.analyzed,
            summary.failed,
            summary.loudness_measured,
            after.analyzed,
            after.tracks,
            after.failed,
        )?;
    }
    out.flush()?;
    Ok(if summary.failed > 0 {
        Outcome::Failure
    } else {
        Outcome::Success
    })
}

/// Progress on standard error: a line that updates itself on a terminal, and the
/// failures.
fn progress(event: &AnalysisEvent, terminal: bool, done: &mut usize, total: &mut usize) {
    let mut err = io::stderr().lock();
    match event {
        AnalysisEvent::Started(found) => *total = found.remaining(),
        AnalysisEvent::Track {
            path,
            pending,
            result,
            ..
        } => {
            if *pending {
                *done += 1;
            }
            let clear = if terminal { "\r\x1b[2K" } else { "" };
            if let Err(error) = result {
                let _ = writeln!(err, "{clear}Cannot analyse {}: {error}", path.display());
            }
            if terminal {
                let _ = write!(err, "\r\x1b[2KAnalysed {done} of {total} tracks");
                let _ = err.flush();
            }
        }
        AnalysisEvent::Finished(_) => {}
    }
}

fn summary_json(summary: &AnalysisSummary, after: &AnalysisCoverage) -> serde_json::Value {
    json!({
        "analyzed": summary.analyzed,
        "failed": summary.failed,
        "loudness_measured": summary.loudness_measured,
        "cancelled": summary.cancelled,
        "library": {
            "tracks": after.tracks,
            "analyzed": after.analyzed,
            "failed": after.failed,
            "remaining": after.remaining(),
        },
    })
}
