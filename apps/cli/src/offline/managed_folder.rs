// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli managed-folder`: the managed media folder's setting, imports into
//! it, consolidation of the library into it, and runs that were cut short.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use helikodu_model::settings::{LibrarySettings, ManagedMode};
use helikodu_service::bus::EventBus;
use helikodu_service::files::AppDirs;
use helikodu_service::library::{
    CancelToken, ItemAction, ItemResult, ManagedItem, ManagedOperation, ManagedOutcome,
    ManagedPlan, ManagedReport, MusicLibrary, SkipReason, Transfer, managed_options,
    set_managed_folder, turn_off_managed_folder,
};
use helikodu_service::settings::{SETTINGS_FILE, SettingsStore};
use serde_json::{Map, Value, json};

use super::database::{DatabaseArgs, insert_path, path_object};
use super::format;
use crate::Outcome;

/// The word written for a field that is empty.
const UNKNOWN: &str = "Unknown";

#[derive(Debug, Args)]
pub(crate) struct ManagedFolderArgs {
    #[command(subcommand)]
    command: ManagedCommand,
    /// The settings file [default: $XDG_CONFIG_HOME/helikodu/settings.toml]
    #[arg(long, value_name = "PATH", global = true)]
    settings: Option<PathBuf>,
    #[command(flatten)]
    database: DatabaseArgs,
}

#[derive(Debug, Subcommand)]
enum ManagedCommand {
    /// Show the managed folder, its pattern and mode, and a run that was cut short.
    Show(JsonArg),
    /// Turn the managed folder on, or change its folder, pattern or mode. A folder
    /// outside every library folder becomes a library folder.
    Set(SetArgs),
    /// Turn the managed folder off. Its files stay where they are.
    Off,
    /// Copy (or move) files and folders from outside the library into the managed
    /// folder, named by its pattern, with their covers, CUE sheets, lyrics and logs.
    Import(ImportArgs),
    /// Move (or copy) the library files outside the managed folder into it; tracks
    /// keep their ratings, play counts and history.
    Consolidate(ConsolidateArgs),
    /// Continue the run that was cut short.
    Resume(JsonArg),
    /// Roll back the run that was cut short: copies go, moved files go back.
    RollBack(JsonArg),
}

#[derive(Debug, Args)]
struct JsonArg {
    /// Print the result as JSON, for scripts.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    /// Imports copy the files.
    Copy,
    /// Imports move the files.
    Move,
}

#[derive(Debug, Args)]
struct SetArgs {
    /// The managed folder.
    #[arg(value_name = "FOLDER")]
    folder: Option<PathBuf>,
    /// The naming pattern below the folder, as in the file organizer.
    #[arg(long, value_name = "PATTERN")]
    pattern: Option<String>,
    /// Whether imports copy or move the files.
    #[arg(long, value_enum)]
    mode: Option<ModeArg>,
    /// Which characters names may hold: portable (names Windows can read), as-is or
    /// ascii.
    #[arg(long, value_name = "SET")]
    characters: Option<String>,
}

#[derive(Debug, Args)]
struct TransferArgs {
    /// Move the files, whatever the setting says.
    #[arg(long = "move", conflicts_with = "copy")]
    move_files: bool,
    /// Copy the files, whatever the setting says.
    #[arg(long)]
    copy: bool,
    /// Only show the plan: every file, where it goes, names that are taken, and the
    /// space needed and free. Nothing changes.
    #[arg(long)]
    dry_run: bool,
    /// Print the plan or the result as JSON, for scripts.
    #[arg(long)]
    json: bool,
}

impl TransferArgs {
    fn transfer(&self) -> Option<Transfer> {
        if self.move_files {
            Some(Transfer::Move)
        } else if self.copy {
            Some(Transfer::Copy)
        } else {
            None
        }
    }
}

#[derive(Debug, Args)]
struct ImportArgs {
    /// The files and folders to import.
    #[arg(value_name = "PATH", required = true)]
    paths: Vec<PathBuf>,
    #[command(flatten)]
    transfer: TransferArgs,
}

#[derive(Debug, Args)]
struct ConsolidateArgs {
    #[command(flatten)]
    transfer: TransferArgs,
}

pub(crate) fn run(args: &ManagedFolderArgs) -> Result<Outcome> {
    let store = SettingsStore::open(settings_path(args.settings.as_deref())?, EventBus::new());
    match &args.command {
        ManagedCommand::Show(json) => show(args, &store, json.json),
        ManagedCommand::Set(set) => {
            let mut added = false;
            let mut failure = None;
            store
                .update(|settings| {
                    match set_managed_folder(
                        &mut settings.library,
                        set.folder.as_deref(),
                        set.pattern.as_deref(),
                        set.mode.map(|m| match m {
                            ModeArg::Copy => ManagedMode::Copy,
                            ModeArg::Move => ManagedMode::Move,
                        }),
                        set.characters.as_deref(),
                    ) {
                        Ok(a) => added = a,
                        Err(e) => failure = Some(e),
                    }
                })
                .context("cannot save the settings")?;
            if let Some(error) = failure {
                bail!(error);
            }
            let settings = store.get();
            if settings.library.managed.folder.is_none() {
                bail!("name the managed folder");
            }
            if added {
                eprintln!("The folder is a library folder now.");
            }
            print_setting(&mut io::stdout().lock(), &settings.library)?;
            Ok(Outcome::Success)
        }
        ManagedCommand::Off => {
            store
                .update(|settings| turn_off_managed_folder(&mut settings.library))
                .context("cannot save the settings")?;
            println!("The managed folder is off.");
            Ok(Outcome::Success)
        }
        ManagedCommand::Import(import) => {
            let settings = store.get().library.clone();
            let options = managed_options(&settings, import.transfer.transfer(), UNKNOWN)?;
            let library = args.database.open()?;
            let plan = library.plan_managed_import(
                &import.paths,
                &settings,
                &options,
                None,
                &CancelToken::new(),
            )?;
            carry_out(&library, &settings, &plan, &import.transfer)
        }
        ManagedCommand::Consolidate(consolidate) => {
            let settings = store.get().library.clone();
            let options = managed_options(&settings, consolidate.transfer.transfer(), UNKNOWN)?;
            let library = args.database.open_existing()?;
            let plan = library.plan_consolidate(&options)?;
            carry_out(&library, &settings, &plan, &consolidate.transfer)
        }
        ManagedCommand::Resume(json) => {
            let settings = store.get().library.clone();
            let library = args.database.open_existing()?;
            let Some(waiting) = library.interrupted_managed_run()? else {
                bail!("no run of the managed folder was cut short");
            };
            let progress = Progress::new();
            let report = library.resume_managed_run(
                waiting.run,
                &settings,
                None,
                &|done, total| progress.show("Placed", done, total),
                &CancelToken::new(),
            )?;
            progress.clear();
            print_report(&report, json.json)
        }
        ManagedCommand::RollBack(json) => {
            let library = args.database.open_existing()?;
            let Some(waiting) = library.interrupted_managed_run()? else {
                bail!("no run of the managed folder was cut short");
            };
            let progress = Progress::new();
            let report = library.roll_back_managed_run(waiting.run, &|done, total| {
                progress.show("Rolled back", done, total);
            })?;
            progress.clear();
            print_report(&report, json.json)
        }
    }
}

fn settings_path(given: Option<&Path>) -> Result<PathBuf> {
    match given {
        Some(path) => Ok(path.to_path_buf()),
        None => Ok(AppDirs::from_env()?.config.join(SETTINGS_FILE)),
    }
}

fn show(args: &ManagedFolderArgs, store: &SettingsStore, json: bool) -> Result<Outcome> {
    let settings = store.get();
    let managed = &settings.library.managed;
    // A run cut short, when there is a library.
    let waiting = args
        .database
        .open_existing()
        .ok()
        .and_then(|library| library.interrupted_managed_run().ok().flatten());
    let mut out = io::stdout().lock();
    if json {
        let mut object = Map::new();
        object.insert("on".into(), json!(managed.is_on()));
        if let Some(folder) = &managed.folder {
            object.insert("folder".into(), path_object(folder));
        }
        object.insert("pattern".into(), json!(managed.pattern));
        object.insert("mode".into(), json!(mode_name(managed.mode)));
        object.insert("characters".into(), json!(managed.characters));
        object.insert(
            "interrupted_run".into(),
            waiting.map_or(Value::Null, |w| {
                json!({
                    "run": w.run.get(),
                    "operation": operation_name(w.operation),
                    "move": w.transfer == Transfer::Move,
                    "started_at": w.started_at,
                    "done": w.done,
                    "waiting": w.waiting,
                    "skipped": w.skipped,
                })
            }),
        );
        serde_json::to_writer_pretty(&mut out, &Value::Object(object))?;
        writeln!(out)?;
    } else {
        print_setting(&mut out, &settings.library)?;
        if let Some(w) = waiting {
            writeln!(
                out,
                "A run was cut short: {} of {} files done. Continue it with \
                 `helikodu-cli managed-folder resume` or undo it with \
                 `helikodu-cli managed-folder roll-back`.",
                w.done,
                w.done + w.waiting + w.skipped
            )?;
        }
    }
    Ok(Outcome::Success)
}

fn mode_name(mode: ManagedMode) -> &'static str {
    match mode {
        ManagedMode::Copy => "copy",
        ManagedMode::Move => "move",
    }
}

fn operation_name(operation: ManagedOperation) -> &'static str {
    match operation {
        ManagedOperation::Import => "import",
        ManagedOperation::Consolidate => "consolidate",
    }
}

fn print_setting(out: &mut impl Write, settings: &LibrarySettings) -> io::Result<()> {
    let managed = &settings.managed;
    match &managed.folder {
        Some(folder) => writeln!(out, "Folder:     {}", folder.display())?,
        None => writeln!(out, "The managed folder is off.")?,
    }
    writeln!(out, "Pattern:    {}", managed.pattern)?;
    writeln!(out, "Imports:    {}", mode_name(managed.mode))?;
    writeln!(out, "Characters: {}", managed.characters)
}

/// Shows the plan and, unless it is a dry run, carries it out.
fn carry_out(
    library: &MusicLibrary,
    settings: &LibrarySettings,
    plan: &ManagedPlan,
    args: &TransferArgs,
) -> Result<Outcome> {
    if args.dry_run {
        let mut out = io::stdout().lock();
        if args.json {
            serde_json::to_writer_pretty(&mut out, &plan_json(plan))?;
            writeln!(out)?;
        } else {
            print_plan(&mut out, plan)?;
        }
        return Ok(Outcome::Success);
    }
    if !plan.fits() {
        bail!(
            "the files need {}, but only {} are free",
            format::size(plan.bytes_needed),
            format::size(plan.free_bytes.unwrap_or(0))
        );
    }
    let progress = Progress::new();
    let report = library.apply_managed(
        plan,
        settings,
        None,
        &|done, total| progress.show("Placed", done, total),
        &CancelToken::new(),
    )?;
    progress.clear();
    print_report(&report, args.json)
}

fn action_name(item: &ManagedItem) -> &'static str {
    match &item.action {
        ItemAction::Copy => "copy",
        ItemAction::Move => "move",
        ItemAction::CopyKeepingSource => "copy (source is read-only)",
        ItemAction::Skip(SkipReason::Identical) => "skip (identical)",
        ItemAction::Skip(_) => "skip",
    }
}

fn print_plan(out: &mut impl Write, plan: &ManagedPlan) -> io::Result<()> {
    for item in &plan.items {
        match &item.action {
            ItemAction::Skip(reason) if *reason != SkipReason::Identical => {
                writeln!(out, "skip  {}: {reason}", item.from.display())?;
            }
            _ => {
                let mut notes = Vec::new();
                if item.renamed {
                    notes.push("name taken, numbered");
                }
                if item.rewritten {
                    notes.push("FILE lines changed");
                }
                let notes = if notes.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", notes.join("; "))
                };
                writeln!(
                    out,
                    "{}  {}\n      -> {}{notes}",
                    action_name(item),
                    item.from.display(),
                    item.to.display()
                )?;
            }
        }
    }
    writeln!(
        out,
        "{} files to place ({}), {} numbered because the name was taken, {} skipped; \
         {} needed, {} free.",
        plan.to_transfer(),
        format::size(plan.total_bytes()),
        plan.renamed(),
        plan.skipped(),
        format::size(plan.bytes_needed),
        plan.free_bytes
            .map_or_else(|| "unknown".to_owned(), format::size)
    )
}

fn plan_json(plan: &ManagedPlan) -> Value {
    let items: Vec<Value> = plan
        .items
        .iter()
        .map(|item| {
            let mut object = Map::new();
            object.insert(
                "kind".into(),
                json!(format!("{:?}", item.kind).to_lowercase()),
            );
            object.insert("action".into(), json!(action_name(item)));
            if let ItemAction::Skip(reason) = &item.action {
                object.insert("reason".into(), json!(reason.to_string()));
            }
            object.insert("from".into(), path_object(&item.from));
            object.insert("to".into(), path_object(&item.to));
            object.insert("size_bytes".into(), json!(item.size));
            object.insert("renamed".into(), json!(item.renamed));
            object.insert("rewritten".into(), json!(item.rewritten));
            object.insert("file_id".into(), json!(item.file.map(|f| f.get())));
            Value::Object(object)
        })
        .collect();
    json!({
        "operation": operation_name(plan.operation),
        "move": plan.options.transfer == Transfer::Move,
        "folder": path_object(&plan.options.folder),
        "items": items,
        "to_place": plan.to_transfer(),
        "renamed": plan.renamed(),
        "skipped": plan.skipped(),
        "total_bytes": plan.total_bytes(),
        "bytes_needed": plan.bytes_needed,
        "free_bytes": plan.free_bytes,
    })
}

fn result_name(outcome: &ManagedOutcome) -> String {
    match &outcome.result {
        ItemResult::Copied => "copied".into(),
        ItemResult::Moved => "moved".into(),
        ItemResult::SourceKept(reason) => format!("copied; the source stays: {reason}"),
        ItemResult::Skipped(reason) => format!("skipped: {reason}"),
        ItemResult::Waiting => "waiting".into(),
        ItemResult::RolledBack => "rolled back".into(),
        ItemResult::NotRolledBack(reason) => format!("not rolled back: {reason}"),
        _ => "unknown".into(),
    }
}

fn print_report(report: &ManagedReport, json: bool) -> Result<Outcome> {
    let mut out = io::stdout().lock();
    let problems = report.problems().count();
    if json {
        let outcomes: Vec<Value> = report
            .outcomes
            .iter()
            .map(|o| {
                let mut object = Map::new();
                insert_path(&mut object, &o.from);
                object.insert("to".into(), path_object(&o.to));
                object.insert("result".into(), json!(result_name(o)));
                Value::Object(object)
            })
            .collect();
        serde_json::to_writer_pretty(
            &mut out,
            &json!({
                "run": report.run.map(|r| r.get()),
                "finished": report.finished,
                "transferred": report.transferred(),
                "problems": problems,
                "library_error": report.library_error,
                "outcomes": outcomes,
            }),
        )?;
        writeln!(out)?;
    } else {
        for outcome in &report.outcomes {
            if !matches!(
                outcome.result,
                ItemResult::Copied | ItemResult::Moved | ItemResult::RolledBack
            ) {
                writeln!(out, "{}: {}", outcome.from.display(), result_name(outcome))?;
            }
        }
        if let Some(error) = &report.library_error {
            writeln!(
                out,
                "The library could not read the new files yet ({error}); a scan will."
            )?;
        }
        writeln!(
            out,
            "{} files placed, {} rolled back, {problems} problems.",
            report.transferred(),
            report
                .outcomes
                .iter()
                .filter(|o| o.result == ItemResult::RolledBack)
                .count()
        )?;
    }
    out.flush()?;
    Ok(if problems > 0 || !report.finished {
        Outcome::Failure
    } else {
        Outcome::Success
    })
}

/// A line on standard error that updates itself on a terminal.
struct Progress {
    terminal: bool,
}

impl Progress {
    fn new() -> Self {
        Self {
            terminal: io::stderr().is_terminal(),
        }
    }

    fn show(&self, what: &str, done: usize, total: usize) {
        if self.terminal {
            let mut err = io::stderr().lock();
            let _ = write!(err, "\r\x1b[2K{what} {done} of {total} files");
            let _ = err.flush();
        }
    }

    fn clear(&self) {
        if self.terminal {
            eprint!("\r\x1b[2K");
        }
    }
}
