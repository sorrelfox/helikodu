// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask ui-checks`: the UI checks of `tests/ui/` over a synthetic
//! library, under a headless compositor.
//!
//! Builds the programs with optimisations, makes a synthetic library
//! (`synth-library`), scans it with `helikodu-cli scan` into a fresh database,
//! then runs `tests/ui/perf.py` (start-up, memory, frame times, idle CPU),
//! `tests/ui/a11y.py` (AT-SPI audit and keyboard path) and, given `--design
//! DIR`, `tests/ui/visual.py` (the designed screens under Adwaita and Breeze,
//! light and dark, against the reference images). Each writes its report into
//! `target/ui-checks/` (or `--out`). Fails when a check fails; a check whose
//! tools are missing reports that it was skipped.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

const USAGE: &str = "\
usage: cargo xtask ui-checks [options]

  --tracks N        size of the synthetic library (default 100k)
  --library DIR     scan this folder instead of a synthetic library
  --design DIR      reference images (<Screen>.png) for the visual check;
                    without it the visual check is skipped
  --only CHECK      perf, a11y or visual (can be given more than once)
  --idle-secs N     length of each idle CPU phase (default 60)
  --out DIR         reports (default target/ui-checks)
";

pub(crate) fn run(args: &[String]) -> Result<()> {
    let mut tracks = String::from("100k");
    let mut library: Option<PathBuf> = None;
    let mut design: Option<PathBuf> = None;
    let mut only: Vec<String> = Vec::new();
    let mut idle_secs = String::from("60");
    let mut out: Option<PathBuf> = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .cloned()
                .with_context(|| format!("`{name}` needs a value\n\n{USAGE}"))
        };
        match arg.as_str() {
            "--tracks" => tracks = value("--tracks")?,
            "--library" => library = Some(PathBuf::from(value("--library")?)),
            "--design" => design = Some(PathBuf::from(value("--design")?)),
            "--only" => only.push(value("--only")?),
            "--idle-secs" => idle_secs = value("--idle-secs")?,
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => bail!("unknown option `{other}` for `ui-checks`\n\n{USAGE}"),
        }
    }
    for check in &only {
        if !matches!(check.as_str(), "perf" | "a11y" | "visual") {
            bail!("`--only` is perf, a11y or visual, not `{check}`");
        }
    }
    if idle_secs.parse::<u64>().is_err() {
        bail!("`--idle-secs` takes a whole number of seconds, not `{idle_secs}`");
    }
    let wants = |check: &str| only.is_empty() || only.iter().any(|o| o == check);
    let root = crate::repository_root()?;
    let out = out.unwrap_or_else(|| root.join("target").join("ui-checks"));
    fs::create_dir_all(&out).with_context(|| format!("cannot create {}", out.display()))?;

    // The programs. With --example arguments Cargo builds only the examples, so
    // the binaries are built in a step of their own.
    status(
        Command::new(crate::cargo())
            .args([
                "build",
                "--release",
                "--bins",
                "-p",
                "helikodu",
                "-p",
                "helikodu-cli",
                "-p",
                "helikodu-scan-worker",
            ])
            .current_dir(&root),
        "cargo build (programs)",
    )?;
    status(
        Command::new(crate::cargo())
            .args([
                "build",
                "--release",
                "-p",
                "helikodu-ui-gtk",
                "--example",
                "perf",
                "--example",
                "screenshot",
                "--example",
                "views",
                "--example",
                "dialogs",
            ])
            .current_dir(&root),
        "building the programs",
    )?;

    // The library and its database.
    let database = out.join("library.db");
    if wants("perf") || wants("a11y") {
        let folder = match library {
            Some(folder) => folder,
            None => {
                let folder = root
                    .join("target")
                    .join("synth-library")
                    .join(format!("flac-{}-seed1", tracks.to_lowercase()));
                crate::synth_library::run(&[
                    "--tracks".into(),
                    tracks.clone(),
                    "--out".into(),
                    folder.display().to_string(),
                ])?;
                folder
            }
        };
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", database.display()));
        }
        status(
            Command::new(root.join("target/release/helikodu-cli"))
                .arg("scan")
                .arg("--database")
                .arg(&database)
                .arg(&folder)
                .stdout(Stdio::null()),
            "scanning the library",
        )?;
    }

    let mut failed = Vec::new();
    let python = |script: &str, extra: &[String], report: &Path| -> Result<bool> {
        let file = fs::File::create(report)
            .with_context(|| format!("cannot create {}", report.display()))?;
        println!("ui-checks: {script} (report: {})", report.display());
        let ok = Command::new("python3")
            .arg(root.join("tests/ui").join(script))
            .args(extra)
            .arg("--no-build")
            .current_dir(&root)
            .stdout(file)
            .status()
            .with_context(|| format!("cannot run python3 {script}"))?
            .success();
        if let Ok(text) = fs::read_to_string(report) {
            print!("{text}");
        }
        Ok(ok)
    };
    if wants("perf") {
        let args = vec![
            "--database".into(),
            database.display().to_string(),
            "--idle-secs".into(),
            idle_secs.clone(),
            "--json".into(),
            out.join("perf.json").display().to_string(),
        ];
        if !python("perf.py", &args, &out.join("perf.md"))? {
            failed.push("perf");
        }
    }
    if wants("a11y") {
        let args = vec!["--database".into(), database.display().to_string()];
        if !python("a11y.py", &args, &out.join("a11y.md"))? {
            failed.push("a11y");
        }
    }
    if wants("visual") {
        match &design {
            Some(design) => {
                let args = vec![
                    "--design".into(),
                    design.display().to_string(),
                    "--out".into(),
                    out.join("visual").display().to_string(),
                ];
                if !python("visual.py", &args, &out.join("visual.md"))? {
                    failed.push("visual");
                }
            }
            None => println!("ui-checks: visual check skipped; give --design DIR"),
        }
    }
    if failed.is_empty() {
        println!("ui-checks: all passed; reports in {}", out.display());
        Ok(())
    } else {
        bail!(
            "ui-checks: {} failed; reports in {}",
            failed.join(", "),
            out.display()
        )
    }
}

fn status(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("{what}: cannot start the program"))?;
    if !status.success() {
        bail!("{what} failed ({status})");
    }
    Ok(())
}
