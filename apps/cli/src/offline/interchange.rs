// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli import`, `undo-import`, `export-user-data`, `import-user-data`,
//! `carry-stats` and `write-stats`.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Args, ValueEnum};
use helikodu_service::library::interchange::{
    ImportPlan, ImportSource, MergeOptions, PathMapping, PlannedPlaylistKind, Player,
    default_sources,
};
use helikodu_service::library::{CancelToken, MusicLibrary, STATS_BATCH, stats_to_files};
use serde_json::{Value, json};

use super::database::{DatabaseArgs, app_settings, path_object};
use super::format;
use crate::Outcome;

/// The `<PLAYER>` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum PlayerArg {
    /// Rhythmbox (rhythmdb.xml and playlists.xml).
    Rhythmbox,
    /// Strawberry (its SQLite database).
    Strawberry,
    /// Clementine (its SQLite database).
    Clementine,
    /// Lollypop (lollypop.db and playlists.db).
    Lollypop,
    /// iTunes or Apple Music (an exported Library.xml).
    Itunes,
}

impl From<PlayerArg> for Player {
    fn from(arg: PlayerArg) -> Self {
        match arg {
            PlayerArg::Rhythmbox => Player::Rhythmbox,
            PlayerArg::Strawberry => Player::Strawberry,
            PlayerArg::Clementine => Player::Clementine,
            PlayerArg::Lollypop => Player::Lollypop,
            PlayerArg::Itunes => Player::ITunes,
        }
    }
}

/// How an import merges, shared by `import` and `import-user-data`.
#[derive(Debug, Args)]
pub(crate) struct MergeArgs {
    /// Carry out the import. Without it, only show what it would do.
    #[arg(long, conflicts_with = "preview")]
    apply: bool,
    /// Only show what the import would do (the default).
    #[arg(long)]
    preview: bool,
    /// A path prefix of the other library and the folder it is now, as FROM=TO
    /// (for example 'C:\Music=/home/me/Music'). May be given several times.
    #[arg(long = "map", value_name = "FROM=TO", value_parser = parse_mapping)]
    mappings: Vec<PathMapping>,
    /// Take the imported rating also for tracks that have one.
    #[arg(long)]
    replace_ratings: bool,
    /// Leave the playlists out.
    #[arg(long)]
    no_playlists: bool,
    /// Print the plan (and the run) as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

impl MergeArgs {
    fn options(&self) -> MergeOptions {
        MergeOptions {
            mappings: self.mappings.clone(),
            replace_ratings: self.replace_ratings,
            skip_playlists: self.no_playlists,
        }
    }
}

fn parse_mapping(text: &str) -> Result<PathMapping, String> {
    PathMapping::parse(text).ok_or_else(|| "expected FROM=TO".to_owned())
}

#[derive(Debug, Args)]
pub(crate) struct ImportArgs {
    /// The player whose statistics, ratings and playlists to import. Its files
    /// are only read.
    player: PlayerArg,
    /// Its library file or the folder holding it [default: where the player
    /// keeps it, Flatpak folders included]
    #[arg(long, value_name = "PATH")]
    path: Option<PathBuf>,
    #[command(flatten)]
    merge: MergeArgs,
}

pub(crate) fn import(args: &ImportArgs) -> Result<Outcome> {
    let player = Player::from(args.player);
    let path = match &args.path {
        Some(path) => path.clone(),
        None => {
            let found: Vec<PathBuf> = default_sources()
                .into_iter()
                .filter(|s| s.player == player)
                .map(|s| s.path)
                .collect();
            match found.as_slice() {
                [] => bail!(
                    "no {} library was found where {} keeps it; name it with --path",
                    player.name(),
                    player.name()
                ),
                [one] => one.clone(),
                several => bail!(
                    "{} libraries were found; choose one with --path:\n  {}",
                    several.len(),
                    several
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n  ")
                ),
            }
        }
    };
    let library = args.merge.database.open_existing()?;
    let plan = library.plan_import(&ImportSource { player, path }, &args.merge.options())?;
    finish(&library, &plan, &args.merge)
}

#[derive(Debug, Args)]
pub(crate) struct ImportUserDataArgs {
    /// The file written by export-user-data.
    file: PathBuf,
    #[command(flatten)]
    merge: MergeArgs,
}

pub(crate) fn import_user_data(args: &ImportUserDataArgs) -> Result<Outcome> {
    let library = args.merge.database.open_existing()?;
    let plan = library.plan_user_data_import(&args.file, &args.merge.options())?;
    finish(&library, &plan, &args.merge)
}

/// Prints the plan and, with `--apply`, carries it out.
fn finish(library: &MusicLibrary, plan: &ImportPlan, args: &MergeArgs) -> Result<Outcome> {
    let run = if args.apply && !plan.is_empty() {
        Some(library.apply_import(plan)?)
    } else {
        None
    };
    let mut out = io::stdout().lock();
    if args.json {
        let mut value = plan_json(plan);
        if let Value::Object(object) = &mut value {
            object.insert("applied".into(), json!(args.apply));
            object.insert("run".into(), json!(run.map(|r| r.get())));
        }
        serde_json::to_writer_pretty(&mut out, &value)?;
        writeln!(out)?;
    } else {
        print_plan(&mut out, plan)?;
        writeln!(out)?;
        match (args.apply, run) {
            (true, Some(run)) => writeln!(
                out,
                "Imported (run {}). Undo it with: helikodu-cli undo-import",
                run.get()
            )?,
            (true, None) => writeln!(out, "Nothing to import.")?,
            (false, _) => writeln!(
                out,
                "Nothing was changed. Run again with --apply to import."
            )?,
        }
    }
    out.flush()?;
    Ok(Outcome::Success)
}

fn print_plan(out: &mut impl Write, plan: &ImportPlan) -> io::Result<()> {
    let m = &plan.matching;
    let t = &plan.totals;
    writeln!(out, "Source: {}", plan.source)?;
    writeln!(
        out,
        "Entries: {} read; {} matched ({} by path, {} by tags), {} ambiguous, {} unmatched",
        m.entries,
        m.matched(),
        m.by_path,
        m.by_tags,
        m.ambiguous,
        m.unmatched
    )?;
    writeln!(
        out,
        "Tracks changed: {} ({} unchanged, for example imported before)",
        t.tracks, t.unchanged
    )?;
    writeln!(
        out,
        "  plays: +{} on {} tracks; skips: +{} on {} tracks",
        t.plays_added, t.play_counts, t.skips_added, t.skip_counts
    )?;
    writeln!(
        out,
        "  ratings set: {}; kept because the track has its own: {}; loved: {}",
        t.ratings, t.rating_conflicts, t.loved
    )?;
    writeln!(
        out,
        "  first plays earlier: {}; last plays later: {}; dates added earlier: {}",
        t.first_played, t.last_played, t.added
    )?;
    if t.history + t.favourites + t.saved_searches > 0 {
        writeln!(
            out,
            "  history entries: {}; favourites: {}; saved searches: {}",
            t.history, t.favourites, t.saved_searches
        )?;
    }
    if !plan.playlists.is_empty() {
        writeln!(out, "Playlists:")?;
        for p in &plan.playlists {
            let mut name = p.folder.join(" / ");
            if !name.is_empty() {
                name.push_str(" / ");
            }
            name.push_str(&p.name);
            let what = match &p.kind {
                PlannedPlaylistKind::Static {
                    items,
                    matched,
                    from_rules,
                } => format!(
                    "{items} entries, {matched} in the library{}",
                    if *from_rules {
                        "; smart there, imported as a list"
                    } else {
                        ""
                    }
                ),
                PlannedPlaylistKind::Smart(query) => format!("smart: {query}"),
                PlannedPlaylistKind::Unmapped(reason) => format!("left out: {reason}"),
                PlannedPlaylistKind::Exists => "imported before, left out".to_owned(),
            };
            writeln!(out, "  {name}: {what}")?;
        }
    }
    for note in &plan.notes {
        writeln!(out, "Note: {note}")?;
    }
    if !plan.ambiguous.is_empty() {
        writeln!(out, "Ambiguous (left out):")?;
        for entry in plan.ambiguous.iter().take(20) {
            writeln!(out, "  {entry}")?;
        }
    }
    if !plan.unmatched.is_empty() {
        writeln!(out, "Not in the library:")?;
        for entry in plan.unmatched.iter().take(20) {
            writeln!(out, "  {entry}")?;
        }
        if m.unmatched > 20 {
            writeln!(out, "  … and {} more", m.unmatched - 20)?;
        }
    }
    Ok(())
}

fn plan_json(plan: &ImportPlan) -> Value {
    let m = &plan.matching;
    let t = &plan.totals;
    let playlists: Vec<Value> = plan
        .playlists
        .iter()
        .map(|p| {
            let (kind, detail) = match &p.kind {
                PlannedPlaylistKind::Static {
                    items,
                    matched,
                    from_rules,
                } => (
                    "static",
                    json!({"items": items, "matched": matched, "from_rules": from_rules}),
                ),
                PlannedPlaylistKind::Smart(query) => ("smart", json!({"query": query})),
                PlannedPlaylistKind::Unmapped(reason) => ("left_out", json!({"reason": reason})),
                PlannedPlaylistKind::Exists => ("exists", json!({})),
            };
            json!({"name": p.name, "folder": p.folder, "kind": kind, "detail": detail})
        })
        .collect();
    json!({
        "source": plan.source,
        "player": plan.player.map(|p| p.id()),
        "matching": {
            "entries": m.entries, "by_path": m.by_path, "by_tags": m.by_tags,
            "ambiguous": m.ambiguous, "unmatched": m.unmatched,
        },
        "changes": {
            "tracks": t.tracks, "unchanged": t.unchanged,
            "play_counts": t.play_counts, "plays_added": t.plays_added,
            "skip_counts": t.skip_counts, "skips_added": t.skips_added,
            "ratings": t.ratings, "rating_conflicts": t.rating_conflicts, "loved": t.loved,
            "first_played": t.first_played, "last_played": t.last_played, "added": t.added,
            "history": t.history, "favourites": t.favourites,
            "saved_searches": t.saved_searches,
        },
        "playlists": playlists,
        "ambiguous": plan.ambiguous,
        "unmatched": plan.unmatched,
        "notes": plan.notes,
    })
}

#[derive(Debug, Args)]
pub(crate) struct UndoImportArgs {
    /// Only list the import runs, newest first.
    #[arg(long)]
    list: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn undo_import(args: &UndoImportArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let runs = library.import_runs()?;
    let mut out = io::stdout().lock();
    if args.list {
        if runs.is_empty() {
            writeln!(out, "Nothing was imported.")?;
        }
        for run in &runs {
            writeln!(
                out,
                "{:>4}  {}  {}{}  ({} tracks, {} entries)",
                run.id.get(),
                format::utc(run.at),
                run.source,
                if run.undone { "  [undone]" } else { "" },
                run.tracks,
                run.rows
            )?;
        }
        return Ok(Outcome::Success);
    }
    let Some(run) = runs.iter().find(|r| !r.undone) else {
        bail!("there is no import to undo");
    };
    library.undo_import(run.id)?;
    writeln!(out, "Undid run {}: {}", run.id.get(), run.source)?;
    Ok(Outcome::Success)
}

#[derive(Debug, Args)]
pub(crate) struct ExportUserDataArgs {
    /// The JSON file to write.
    file: PathBuf,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn export_user_data(args: &ExportUserDataArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let export = library.export_user_data_to(&args.file)?;
    println!(
        "Wrote {}: {} tracks with {} history entries, {} playlists, {} favourites, {} saved searches.",
        args.file.display(),
        export.tracks(),
        export.history(),
        export.playlists(),
        export.favourites(),
        export.saved_searches()
    );
    Ok(Outcome::Success)
}

#[derive(Debug, Args)]
pub(crate) struct CarryStatsArgs {
    /// Carry them: from the file given with --from to the file given with --to,
    /// or else for every missing track with exactly one replacement.
    #[arg(long)]
    apply: bool,
    /// The replaced (missing) file.
    #[arg(long, value_name = "PATH", requires = "to")]
    from: Option<PathBuf>,
    /// The file that replaces it.
    #[arg(long, value_name = "PATH", requires = "from")]
    to: Option<PathBuf>,
    /// Print the candidates as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn carry_stats(args: &CarryStatsArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let mut out = io::stdout().lock();
    if let (Some(from), Some(to)) = (&args.from, &args.to) {
        let Some(old) = library.track_at(from)? else {
            bail!("{} is not in the library", from.display());
        };
        let Some(new) = library.track_at(to)? else {
            bail!("{} is not in the library", to.display());
        };
        if !args.apply {
            writeln!(out, "Run again with --apply to carry the statistics.")?;
            return Ok(Outcome::Success);
        }
        let run = library.carry_stats(old, new)?;
        writeln!(out, "Carried (run {}).", run.get())?;
        return Ok(Outcome::Success);
    }
    let candidates = library.carry_candidates()?;
    let mut per_old: HashMap<_, usize> = HashMap::new();
    for c in &candidates {
        *per_old.entry(c.old).or_default() += 1;
    }
    if args.json {
        let list: Vec<Value> = candidates
            .iter()
            .map(|c| {
                json!({
                    "old": path_object(&c.old_path),
                    "new": path_object(&c.new_path),
                    "title": c.title,
                    "artist": c.artist,
                    "play_count": c.stats.play_count,
                    "rating": c.stats.rating.map(|r| r.half_stars()),
                    "history": c.history,
                    "playlist_items": c.playlist_items,
                    "unique": per_old[&c.old] == 1,
                })
            })
            .collect();
        serde_json::to_writer_pretty(&mut out, &json!({ "candidates": list }))?;
        writeln!(out)?;
    } else if candidates.is_empty() {
        writeln!(
            out,
            "No missing file has statistics that a replacement could take."
        )?;
    } else {
        for c in &candidates {
            writeln!(
                out,
                "{} → {}\n    {} plays, rating {}, {} history entries, {} playlist entries{}",
                c.old_path.display(),
                c.new_path.display(),
                c.stats.play_count,
                c.stats
                    .rating
                    .map_or_else(|| "none".to_owned(), |r| r.to_string()),
                c.history,
                c.playlist_items,
                if per_old[&c.old] > 1 {
                    "; several replacements: choose with --from and --to"
                } else {
                    ""
                }
            )?;
        }
    }
    if args.apply {
        let mut carried = 0;
        for c in candidates.iter().filter(|c| per_old[&c.old] == 1) {
            library.carry_stats(c.old, c.new)?;
            carried += 1;
        }
        if !args.json {
            writeln!(out, "Carried the statistics of {carried} tracks.")?;
        }
    } else if !args.json && !candidates.is_empty() {
        writeln!(
            out,
            "Nothing was changed. Run again with --apply to carry them."
        )?;
    }
    Ok(Outcome::Success)
}

#[derive(Debug, Args)]
pub(crate) struct WriteStatsArgs {
    /// Write ratings [default: as the settings say]
    #[arg(long)]
    ratings: bool,
    /// Write play counts [default: as the settings say]
    #[arg(long)]
    play_counts: bool,
    /// The scan worker program that reads written files again [default:
    /// helikodu-scan-worker beside this program]
    #[arg(long, value_name = "PATH")]
    worker: Option<PathBuf>,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn write_stats(args: &WriteStatsArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let mut settings = app_settings();
    if args.ratings || args.play_counts {
        settings.advanced.write_ratings_to_tags = args.ratings;
        settings.advanced.write_play_counts_to_tags = args.play_counts;
    }
    let what = stats_to_files(&settings.advanced);
    if !what.any() {
        bail!(
            "writing ratings and play counts to files is off in the settings; \
             name what to write with --ratings or --play-counts"
        );
    }
    let mut written = 0;
    let mut failed = 0;
    loop {
        let report = library.write_stats_to_files(
            &settings.library,
            &settings.advanced,
            args.worker.as_deref(),
            STATS_BATCH,
            &[],
            &CancelToken::new(),
        )?;
        written += report.written;
        for (path, reason) in &report.failed {
            eprintln!("Cannot write {}: {reason}", path.display());
        }
        failed += report.failed.len();
        if report.remaining == 0 || report.written == 0 {
            break;
        }
    }
    println!("{written} files written, {failed} failed.");
    Ok(if failed > 0 {
        Outcome::Failure
    } else {
        Outcome::Success
    })
}
