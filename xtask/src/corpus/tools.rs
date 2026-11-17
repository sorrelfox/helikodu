// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The external programs: whether they are installed, their versions, the
//! encoders and muxers FFmpeg was built with, and running them.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Output options FFmpeg gets for every file, so that the same input and the
/// same FFmpeg version give the same bytes: no metadata copied from the
/// input, no version strings, random IDs or dates, and one thread.
const FFMPEG_OUTPUT_OPTIONS: &[&str] = &[
    "-map_metadata",
    "-1",
    "-map_chapters",
    "-1",
    "-fflags",
    "+bitexact",
    "-flags",
    "+bitexact",
    "-threads",
    "1",
];

/// What is installed on this machine.
#[derive(Debug, Default)]
pub(crate) struct Toolbox {
    /// Program name to version line, or `None` when it is not installed.
    programs: BTreeMap<String, Option<String>>,
    encoders: BTreeSet<String>,
    muxers: BTreeSet<String>,
}

impl Toolbox {
    /// The version line of `program`, or `None` if it is not installed.
    pub(crate) fn version(&mut self, program: &str) -> Option<String> {
        if !self.programs.contains_key(program) {
            let version = find_program(program).map(|_| version_line(program));
            self.programs.insert(program.to_owned(), version);
            if program == "ffmpeg" && self.programs[program].is_some() {
                self.encoders = ffmpeg_list("-encoders");
                self.muxers = ffmpeg_list("-muxers");
            }
        }
        self.programs[program].clone()
    }

    /// The first of `needs` that FFmpeg lacks, if any.
    pub(crate) fn missing_need(&mut self, needs: &[String]) -> Option<String> {
        if needs.is_empty() {
            return None;
        }
        if self.version("ffmpeg").is_none() {
            return Some(String::from("`ffmpeg` is not installed"));
        }
        for need in needs {
            let present = match need.split_once(':') {
                Some(("encoder", name)) => self.encoders.contains(name),
                Some(("muxer", name)) => self.muxers.contains(name),
                _ => false,
            };
            if !present {
                return Some(format!("FFmpeg has no {}", need.replace(':', " ")));
            }
        }
        None
    }

    /// The versions of every program that was looked up and is installed.
    pub(crate) fn versions(&self) -> BTreeMap<String, String> {
        self.programs
            .iter()
            .filter_map(|(name, version)| Some((name.clone(), version.clone()?)))
            .collect()
    }
}

/// Runs a recipe's external tool. `input` and `output` replace the
/// `{input}` and `{output}` placeholders; for FFmpeg the input is added as
/// the first `-i` and the output, after the bit-exact options, as the last
/// argument.
pub(crate) fn run(tool: &str, args: &[String], input: Option<&Path>, output: &Path) -> Result<()> {
    let mut command = Command::new(tool);
    if tool == "ffmpeg" {
        command.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]);
        if let Some(input) = input {
            command.arg("-i").arg(input);
        }
        command.args(args.iter().map(|arg| substitute(arg, input, output)));
        command.args(FFMPEG_OUTPUT_OPTIONS).arg(output);
    } else {
        command.args(args.iter().map(|arg| substitute(arg, input, output)));
    }
    let result = command
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("could not start `{tool}`"))?;
    if !result.status.success() {
        bail!(
            "`{tool}` failed ({}):\n{}{}",
            result.status,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(())
}

fn substitute(arg: &str, input: Option<&Path>, output: &Path) -> OsString {
    match arg {
        "{input}" => input.map_or_else(OsString::new, |path| path.as_os_str().to_owned()),
        "{output}" => output.as_os_str().to_owned(),
        _ => OsString::from(arg),
    }
}

/// The full path of `program` if it is on `PATH`.
pub(crate) fn find_program(program: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|folder| folder.join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// The first line a program prints when asked for its version. The program
/// is started by name, not by path, as some print their own path.
fn version_line(program: &str) -> String {
    let flag = if matches!(program, "ffmpeg" | "ffprobe") {
        "-version"
    } else {
        "--version"
    };
    let Ok(output) = Command::new(program)
        .arg(flag)
        .stdin(Stdio::null())
        .output()
    else {
        return String::from("unknown");
    };
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

/// The names in `ffmpeg -encoders` or `ffmpeg -muxers`.
fn ffmpeg_list(flag: &str) -> BTreeSet<String> {
    let Ok(output) = Command::new("ffmpeg")
        .args(["-hide_banner", flag])
        .stdin(Stdio::null())
        .output()
    else {
        return BTreeSet::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut names = BTreeSet::new();
    let mut in_list = false;
    for line in text.lines() {
        if line.trim_start().starts_with("--") {
            in_list = true;
            continue;
        }
        if !in_list {
            continue;
        }
        // "<flags> <name[,alias…]> <description>"
        if let Some(field) = line.split_whitespace().nth(1) {
            names.extend(field.split(',').map(str::to_owned));
        }
    }
    names
}
