// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask corpus`: the generated test corpus.
//!
//! The recipes in `tests/corpus/recipes.toml` say how each file is made. The
//! files go to `target/corpus/`, with a manifest of their SHA-256 checksums
//! and the versions of the tools that made them. A file whose recipe, tool
//! version and input have not changed is not made again.
//!
//! A recipe whose program or FFmpeg encoder is missing is skipped with a
//! notice; with `--strict` that is an error. `--check` verifies an existing
//! corpus against its manifest without making anything.

mod builtin;
mod charset;
pub(crate) mod manifest;
pub(crate) mod names;
mod probe;
mod recipe;
mod signal;
mod tagging;
pub(crate) mod tools;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use manifest::{Entry, Manifest, Skipped};
use recipe::{Job, Recipes, Source};
use tools::Toolbox;

const USAGE: &str = "\
usage: cargo xtask corpus [--check] [--strict]

  --check    verify target/corpus against its manifest; make nothing
  --strict   fail when a recipe is skipped because a tool is missing
";

/// Where the recipes are, relative to the repository root.
const RECIPES: &str = "tests/corpus/recipes.toml";
/// Where the corpus goes, relative to the repository root.
const OUTPUT: &str = "target/corpus";
/// Scratch folder inside the corpus folder; removed after each run.
const WORK: &str = ".work";
/// Changes whenever the source signal or a built-in step changes, so that
/// files made by an older `xtask` are made again.
const BUILTIN_VERSION: &str = "xtask-corpus 2";

pub(crate) fn run(args: &[String]) -> Result<()> {
    let mut check = false;
    let mut strict = false;
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--strict" => strict = true,
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => bail!("unknown option `{other}`\n\n{USAGE}"),
        }
    }
    let root = crate::repository_root()?;
    let recipes_path = root.join(RECIPES);
    let out = root.join(OUTPUT);
    if check {
        verify(&out, &recipes_path)
    } else {
        generate(&out, &recipes_path, strict)
    }
}

/// Everything one run keeps track of.
#[derive(Debug)]
struct Run {
    out: PathBuf,
    work: PathBuf,
    old: BTreeMap<String, Entry>,
    entries: Vec<Entry>,
    /// Path to checksum of every file made or kept so far.
    hashes: BTreeMap<String, String>,
    skipped: Vec<Skipped>,
    toolbox: Toolbox,
    written: usize,
    can_probe: bool,
}

fn generate(out: &Path, recipes_path: &Path, strict: bool) -> Result<()> {
    let recipes = recipe::load(recipes_path)?;
    let recipes_sha256 = manifest::sha256_file(recipes_path)?;
    let manifest_path = out.join(manifest::FILE_NAME);
    let old = Manifest::load_or_default(&manifest_path);

    let work = out.join(WORK);
    if work.exists() {
        fs::remove_dir_all(&work)
            .with_context(|| format!("could not remove {}", work.display()))?;
    }
    fs::create_dir_all(&work).with_context(|| format!("could not create {}", work.display()))?;

    let mut toolbox = Toolbox::default();
    let can_probe = toolbox.version("ffprobe").is_some() && toolbox.version("ffmpeg").is_some();
    if !can_probe {
        if strict {
            bail!("`ffprobe` and `ffmpeg` are needed to verify the corpus");
        }
        println!("notice: ffprobe or ffmpeg is not installed; files are not verified");
    }

    let mut state = Run {
        out: out.to_path_buf(),
        work: work.clone(),
        old: old
            .files
            .into_iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect(),
        entries: Vec::new(),
        hashes: BTreeMap::new(),
        skipped: Vec::new(),
        toolbox,
        written: 0,
        can_probe,
    };

    let Recipes { sources, jobs } = recipes;
    for source in &sources {
        write_source(&mut state, source)?;
    }
    // A failing recipe does not stop the run, so that one run shows every
    // failure; the run still fails at the end.
    let mut failures = Vec::new();
    for (index, job) in jobs.iter().enumerate() {
        if let Err(error) = make(&mut state, index, job) {
            println!("error {}: {error:#}", job.path);
            failures.push(job.path.clone());
        }
    }

    fs::remove_dir_all(&work).with_context(|| format!("could not remove {}", work.display()))?;
    let kept: BTreeSet<String> = state.entries.iter().map(|e| e.path.clone()).collect();
    let removed = remove_stale(out, &kept)?;

    let formats = count_formats(&state.entries);
    let manifest = Manifest {
        recipes_sha256,
        formats,
        tools: state.toolbox.versions(),
        files: state.entries,
        skipped: state.skipped,
    };
    manifest.save(&manifest_path)?;

    println!(
        "corpus: {} files in {} ({} made, {} up to date{})",
        manifest.files.len(),
        out.display(),
        state.written,
        manifest.files.len() - state.written,
        if removed > 0 {
            format!(", {removed} stale removed")
        } else {
            String::new()
        }
    );
    println!("corpus: {formats} formats produced");
    if !failures.is_empty() {
        bail!("{} recipes failed: {}", failures.len(), failures.join(", "));
    }
    if !manifest.skipped.is_empty() {
        println!("corpus: {} recipes skipped:", manifest.skipped.len());
        for skipped in &manifest.skipped {
            println!("  {}: {}", skipped.path, skipped.reason);
        }
        if strict {
            bail!("recipes were skipped and --strict is set");
        }
    }
    Ok(())
}

/// Writes a source signal and its parts, unless they are already there.
fn write_source(state: &mut Run, source: &Source) -> Result<()> {
    let samples = signal::render(source);
    let fingerprint = manifest::sha256(
        format!(
            "{BUILTIN_VERSION}\n{}",
            serde_json::to_string(source).context("could not serialise a source")?
        )
        .as_bytes(),
    );
    let mut files = vec![(
        format!("sources/{}.wav", source.name),
        signal::wav(source, &samples),
    )];
    let channels = usize::from(source.channels);
    let mut start = 0;
    for (index, length) in recipe::part_lengths(source).into_iter().enumerate() {
        if source.split.is_empty() {
            break;
        }
        let length = usize::try_from(length).context("the source is too long")?;
        let part = &samples[start * channels..(start + length) * channels];
        files.push((
            format!("sources/{}.part{}.wav", source.name, index + 1),
            signal::wav(source, part),
        ));
        start += length;
    }
    for (path, bytes) in files {
        let sha256 = manifest::sha256(&bytes);
        let target = state.out.join(&path);
        let current = fs::read(&target).ok().map(|old| manifest::sha256(&old));
        if current.as_deref() != Some(sha256.as_str()) {
            create_parent(&target)?;
            fs::write(&target, &bytes)
                .with_context(|| format!("could not write {}", target.display()))?;
            state.written += 1;
        }
        state.hashes.insert(path.clone(), sha256.clone());
        state.entries.push(Entry {
            path,
            set: String::from("sources"),
            format: None,
            sha256,
            size: bytes.len() as u64,
            tool: String::from("xtask"),
            fingerprint: fingerprint.clone(),
            expect: None,
        });
    }
    Ok(())
}

/// Makes one sample, or keeps it if it is up to date, or skips it.
fn make(state: &mut Run, index: usize, job: &Job) -> Result<()> {
    let recipe = &job.recipe;
    let skip = |state: &mut Run, reason: String| {
        println!("skip {}: {reason}", job.path);
        state.skipped.push(Skipped {
            path: job.path.clone(),
            reason,
        });
    };

    let input_hash = match &job.input {
        Some(input) => match state.hashes.get(input) {
            Some(hash) => Some(hash.clone()),
            None => {
                skip(state, format!("its input {input} was not made"));
                return Ok(());
            }
        },
        None => None,
    };
    let builtin = recipe::BUILTIN_TOOLS.contains(&recipe.tool.as_str());
    let version = if builtin {
        String::from(BUILTIN_VERSION)
    } else {
        let Some(version) = state.toolbox.version(&recipe.tool) else {
            skip(state, format!("`{}` is not installed", recipe.tool));
            return Ok(());
        };
        version
    };
    if let Some(reason) = state.toolbox.missing_need(&recipe.needs) {
        skip(state, reason);
        return Ok(());
    }

    let fingerprint = manifest::sha256(
        format!(
            "{}\n{version}\n{}",
            serde_json::to_string(job).context("could not serialise a recipe")?,
            input_hash.unwrap_or_default()
        )
        .as_bytes(),
    );

    // The main output first, then the extra files the tool writes next to it.
    let mut outputs = vec![job.path.clone()];
    outputs.extend(
        recipe
            .extra_outputs
            .iter()
            .map(|extension| recipe::with_extension(&job.path, extension)),
    );

    if let Some(kept) = up_to_date(state, &outputs, &fingerprint) {
        for entry in kept {
            state
                .hashes
                .insert(entry.path.clone(), entry.sha256.clone());
            state.entries.push(entry);
        }
        return Ok(());
    }

    // Tools write to a plain name in the scratch folder: some of the final
    // names start with a dash or contain a newline, which tools would
    // misread. The file is moved into place once it is complete.
    let extension = extension_of(&job.path_bytes);
    let scratch = |extension: &str| state.work.join(format!("{index}.{extension}"));
    let main_scratch = scratch(&extension);
    let input_path = job
        .input
        .as_ref()
        .map(|input| bytes_path(&state.out, &names::decode(input).unwrap_or_default()));
    if builtin {
        builtin::run(recipe, input_path.as_deref(), &main_scratch)?;
    } else if let Err(error) = tools::run(
        &recipe.tool,
        &recipe.args,
        input_path.as_deref(),
        &main_scratch,
    ) {
        if !recipe.optional {
            return Err(error);
        }
        let message = format!("{error:#}");
        let first_line = message.lines().nth(1).unwrap_or("failed").trim();
        // Drop FFmpeg's "[component @ 0xaddress] " prefix; the address
        // changes from run to run.
        let first_line = match first_line.split_once("] ") {
            Some((prefix, rest)) if prefix.starts_with('[') => rest,
            _ => first_line,
        }
        .to_owned();
        skip(state, format!("`{}` failed: {first_line}", recipe.tool));
        return Ok(());
    }

    if let Some(expect) = &job.expect
        && expect.needs_probe()
        && state.can_probe
    {
        probe::verify(&main_scratch, expect).context("the file is not what the recipe expects")?;
    }

    for (number, output) in outputs.iter().enumerate() {
        let from = if number == 0 {
            main_scratch.clone()
        } else {
            scratch(&recipe.extra_outputs[number - 1])
        };
        let bytes = names::decode(output)?;
        let target = bytes_path(&state.out, &bytes);
        create_parent(&target)?;
        fs::rename(&from, &target)
            .with_context(|| format!("`{}` did not write {}", recipe.tool, from.display()))?;
        let sha256 = manifest::sha256_file(&target)?;
        let size = fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        println!("made {output}");
        state.written += 1;
        state.hashes.insert(output.clone(), sha256.clone());
        state.entries.push(Entry {
            path: output.clone(),
            set: recipe.set.clone(),
            format: if number == 0 {
                recipe.format.clone()
            } else {
                None
            },
            sha256,
            size,
            tool: recipe.tool.clone(),
            fingerprint: fingerprint.clone(),
            expect: if number == 0 {
                job.expect.clone()
            } else {
                None
            },
        });
    }
    Ok(())
}

/// The manifest entries of `outputs` if all of them were made from the same
/// fingerprint and are unchanged on disk.
fn up_to_date(state: &Run, outputs: &[String], fingerprint: &str) -> Option<Vec<Entry>> {
    let mut kept = Vec::new();
    for output in outputs {
        let entry = state.old.get(output)?;
        if entry.fingerprint != fingerprint {
            return None;
        }
        let path = bytes_path(&state.out, &names::decode(output).ok()?);
        if manifest::sha256_file(&path).ok()? != entry.sha256 {
            return None;
        }
        kept.push(entry.clone());
    }
    Some(kept)
}

/// `--check`: every file in the manifest exists with its checksum, and the
/// corpus folder holds nothing else.
fn verify(out: &Path, recipes_path: &Path) -> Result<()> {
    let manifest_path = out.join(manifest::FILE_NAME);
    if !manifest_path.is_file() {
        bail!(
            "there is no corpus in {}; run `cargo xtask corpus`",
            out.display()
        );
    }
    let manifest = Manifest::load(&manifest_path)?;
    let mut problems = Vec::new();

    if manifest::sha256_file(recipes_path)? != manifest.recipes_sha256 {
        problems.push(String::from(
            "the recipes changed since the corpus was made; run `cargo xtask corpus`",
        ));
    }
    let mut listed = BTreeSet::new();
    for entry in &manifest.files {
        listed.insert(entry.path.clone());
        let path = bytes_path(out, &names::decode(&entry.path)?);
        match manifest::sha256_file(&path) {
            Ok(sha256) if sha256 == entry.sha256 => {}
            Ok(_) => problems.push(format!("{}: checksum differs", entry.path)),
            Err(_) => problems.push(format!("{}: missing", entry.path)),
        }
    }
    for path in files_below(out)? {
        if !listed.contains(&path) && path != manifest::FILE_NAME {
            problems.push(format!("{path}: not in the manifest"));
        }
    }

    let mut toolbox = Toolbox::default();
    for (tool, version) in &manifest.tools {
        let current = toolbox.version(tool);
        if current.as_deref() != Some(version.as_str()) {
            println!(
                "note: the corpus was made with `{version}`; installed: `{}`",
                current.as_deref().unwrap_or("none")
            );
        }
    }

    if !problems.is_empty() {
        for problem in &problems {
            println!("  {problem}");
        }
        bail!("the corpus does not match its manifest");
    }
    println!(
        "corpus: {} files match the manifest ({} formats, {} recipes skipped)",
        manifest.files.len(),
        manifest.formats,
        manifest.skipped.len()
    );
    Ok(())
}

/// The number of distinct `format` labels among the formats set's files.
fn count_formats(entries: &[Entry]) -> usize {
    entries
        .iter()
        .filter(|entry| entry.set == "formats")
        .filter_map(|entry| entry.format.as_deref())
        .collect::<BTreeSet<_>>()
        .len()
}

/// Removes files under `out` that are not in `keep` (and not the manifest),
/// then the folders that became empty. Returns the number of files removed.
fn remove_stale(out: &Path, keep: &BTreeSet<String>) -> Result<usize> {
    let mut removed = 0;
    for path in files_below(out)? {
        if keep.contains(&path) || path == manifest::FILE_NAME {
            continue;
        }
        let full = bytes_path(out, &names::decode(&path)?);
        fs::remove_file(&full).with_context(|| format!("could not remove {}", full.display()))?;
        println!("removed stale {path}");
        removed += 1;
    }
    remove_empty_folders(out);
    Ok(removed)
}

fn remove_empty_folders(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_empty_folders(&path);
            // Fails, as intended, when the folder is not empty.
            let _ = fs::remove_dir(&path);
        }
    }
}

/// Every file below `root`, as percent-encoded paths relative to it.
fn files_below(root: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let entries = fs::read_dir(&folder)
            .with_context(|| format!("could not read {}", folder.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("could not read {}", folder.display()))?;
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                found.push(names::encode(relative.as_os_str().as_bytes()));
            }
        }
    }
    found.sort();
    Ok(found)
}

/// `root` joined with a relative path given as raw bytes.
fn bytes_path(root: &Path, relative: &[u8]) -> PathBuf {
    root.join(OsStr::from_bytes(relative))
}

/// The extension of the last component of `path`, for scratch files; tools
/// pick the output format from it.
fn extension_of(path: &[u8]) -> String {
    let file = path.rsplit(|&byte| byte == b'/').next().unwrap_or(path);
    file.iter()
        .rposition(|&byte| byte == b'.')
        .map(|dot| String::from_utf8_lossy(&file[dot + 1..]).into_owned())
        .filter(|extension| extension.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or_else(|| String::from("bin"))
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    Ok(())
}
