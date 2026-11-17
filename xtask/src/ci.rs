// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask ci`: every check CI runs, in the same order.
//!
//! The CI workflows only call this command, so a run on a developer machine
//! and a run in CI check the same things. Outside CI, checks whose tool is not
//! installed (`cargo deny`, `reuse`) are skipped with a warning; in CI (when
//! the `CI` variable is set) a missing tool is an error.
//!
//! `--tests-only` builds and tests the workspace and checks the dependency
//! rules, leaving out the lint tools. The distribution build matrix uses it.

use std::env;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

pub(crate) fn run(args: &[String]) -> Result<()> {
    let mut tests_only = false;
    for arg in args {
        match arg.as_str() {
            "--tests-only" => tests_only = true,
            other => bail!("unknown option `{other}` for `ci`"),
        }
    }

    let root = crate::repository_root()?;
    let cargo = crate::cargo();
    let in_ci = env::var_os("CI").is_some();

    if tests_only {
        step(
            &root,
            "build",
            &cargo,
            &[
                "build",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--locked",
            ],
        )?;
        step(
            &root,
            "tests",
            &cargo,
            &["test", "--workspace", "--all-features", "--locked"],
        )?;
        println!("==> dependency rules");
        crate::check_deps::run()?;
        println!("==> build and tests passed");
        return Ok(());
    }

    step(&root, "format", &cargo, &["fmt", "--all", "--check"])?;
    step(
        &root,
        "clippy",
        &cargo,
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    step(
        &root,
        "tests",
        &cargo,
        &["test", "--workspace", "--all-features", "--locked"],
    )?;

    println!("==> dependency rules");
    crate::check_deps::run()?;

    if tool_available(&cargo, &["deny", "--version"]) {
        step(
            &root,
            "cargo deny",
            &cargo,
            &["deny", "--all-features", "check"],
        )?;
    } else {
        missing("cargo deny", "cargo install --locked cargo-deny", in_ci)?;
    }

    if tool_available("reuse", &["--version"]) {
        step(&root, "REUSE", "reuse", &["lint"])?;
    } else {
        missing("reuse", "see https://reuse.software", in_ci)?;
    }

    println!("==> all checks passed");
    Ok(())
}

fn step(root: &Path, title: &str, program: &str, args: &[&str]) -> Result<()> {
    println!("==> {title}");
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("could not start `{program}`"))?;
    if !status.success() {
        bail!("{title} failed");
    }
    Ok(())
}

fn tool_available(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn missing(tool: &str, install_hint: &str, in_ci: bool) -> Result<()> {
    if in_ci {
        bail!("`{tool}` is not installed ({install_hint})");
    }
    println!("==> {tool}: skipped, not installed ({install_hint})");
    Ok(())
}
