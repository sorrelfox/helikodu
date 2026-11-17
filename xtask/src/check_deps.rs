// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask check-deps`: the crate dependency rules and the unsafe-code policy.
//!
//! The rules:
//! - every package may depend only on the workspace crates listed for it in [`RULES`];
//! - `helikodu-testkit` is only ever a development dependency;
//! - every package outside [`UNSAFE_ALLOWED`] has `#![forbid(unsafe_code)]` in
//!   the root file of each of its targets;
//! - every package has a README.
//!
//! Network, TLS and DNS crates are banned by `cargo deny` (see `deny.toml`).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// The workspace packages each package may depend on (normal and build dependencies).
const RULES: &[(&str, &[&str])] = &[
    ("helikodu-model", &[]),
    ("helikodu-ffmpeg-bridge", &["helikodu-model"]),
    ("helikodu-openmpt-sys", &[]),
    ("helikodu-gme-sys", &[]),
    ("helikodu-vgmstream-sys", &[]),
    ("helikodu-sidplayfp-sys", &[]),
    (
        "helikodu-extra-decoders",
        &[
            "helikodu-model",
            "helikodu-openmpt-sys",
            "helikodu-gme-sys",
            "helikodu-vgmstream-sys",
            "helikodu-sidplayfp-sys",
        ],
    ),
    (
        "helikodu-engine",
        &[
            "helikodu-model",
            "helikodu-ffmpeg-bridge",
            "helikodu-extra-decoders",
        ],
    ),
    (
        "helikodu-meta",
        &["helikodu-model", "helikodu-ffmpeg-bridge"],
    ),
    ("helikodu-library", &["helikodu-model", "helikodu-meta"]),
    (
        "helikodu-service",
        &["helikodu-model", "helikodu-engine", "helikodu-library"],
    ),
    ("helikodu-desktop", &["helikodu-model", "helikodu-service"]),
    (
        "helikodu-ui-gtk",
        &["helikodu-model", "helikodu-service", "helikodu-desktop"],
    ),
    ("helikodu-testkit", &["helikodu-model"]),
    (
        "helikodu",
        &[
            "helikodu-model",
            "helikodu-service",
            "helikodu-desktop",
            "helikodu-ui-gtk",
        ],
    ),
    ("helikodu-cli", &["helikodu-model", "helikodu-service"]),
    (
        "helikodu-scan-worker",
        &[
            "helikodu-model",
            "helikodu-meta",
            "helikodu-ffmpeg-bridge",
            "helikodu-extra-decoders",
        ],
    ),
    ("helikodu-offline-launcher", &[]),
    ("xtask", &[]),
];

/// The only packages that may contain `unsafe` code: the FFI bridges and the binding crates.
const UNSAFE_ALLOWED: &[&str] = &[
    "helikodu-ffmpeg-bridge",
    "helikodu-extra-decoders",
    "helikodu-openmpt-sys",
    "helikodu-gme-sys",
    "helikodu-vgmstream-sys",
    "helikodu-sidplayfp-sys",
];

const TESTKIT: &str = "helikodu-testkit";
const FORBID_UNSAFE: &str = "#![forbid(unsafe_code)]";

pub(crate) fn run() -> Result<()> {
    let root = crate::repository_root()?;
    let output = Command::new(crate::cargo())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(&root)
        .output()
        .context("could not run `cargo metadata`")?;
    if !output.status.success() {
        bail!(
            "`cargo metadata` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: Value =
        serde_json::from_slice(&output.stdout).context("`cargo metadata` printed invalid JSON")?;
    let packages = metadata["packages"]
        .as_array()
        .context("`cargo metadata` output has no package list")?;

    let workspace: BTreeSet<&str> = packages
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();

    let mut problems = Vec::new();
    for package in packages {
        check_package(package, &workspace, &mut problems)?;
    }
    for (name, _) in RULES {
        if !workspace.contains(name) {
            problems.push(format!(
                "`{name}` has a rule but is not in the workspace; remove its rule"
            ));
        }
    }

    if problems.is_empty() {
        println!("check-deps: {} packages follow the rules", workspace.len());
        Ok(())
    } else {
        for problem in &problems {
            eprintln!("check-deps: {problem}");
        }
        bail!("{} dependency rule violation(s)", problems.len())
    }
}

fn check_package(
    package: &Value,
    workspace: &BTreeSet<&str>,
    problems: &mut Vec<String>,
) -> Result<()> {
    let name = package["name"].as_str().context("a package has no name")?;
    let Some(allowed) = RULES
        .iter()
        .find(|(rule_name, _)| *rule_name == name)
        .map(|(_, allowed)| *allowed)
    else {
        problems.push(format!(
            "`{name}` has no dependency rule; add it to RULES in xtask/src/check_deps.rs"
        ));
        return Ok(());
    };

    for dependency in package["dependencies"].as_array().into_iter().flatten() {
        let dep_name = dependency["name"].as_str().unwrap_or_default();
        if !workspace.contains(dep_name) {
            continue;
        }
        let is_dev = dependency["kind"].as_str() == Some("dev");
        if dep_name == TESTKIT {
            if !is_dev {
                problems.push(format!(
                    "`{name}` depends on `{TESTKIT}` outside [dev-dependencies]"
                ));
            }
            continue;
        }
        if !allowed.contains(&dep_name) {
            problems.push(format!("`{name}` must not depend on `{dep_name}`"));
        }
    }

    let manifest = Path::new(
        package["manifest_path"]
            .as_str()
            .context("a package has no manifest path")?,
    );
    let package_dir = manifest.parent().context("a manifest has no folder")?;
    if !package_dir.join("README.md").is_file() {
        problems.push(format!("`{name}` has no README.md"));
    }

    if !UNSAFE_ALLOWED.contains(&name) {
        for target in package["targets"].as_array().into_iter().flatten() {
            let kinds = target["kind"].as_array().into_iter().flatten();
            let is_root = kinds
                .filter_map(Value::as_str)
                .any(|kind| matches!(kind, "lib" | "bin" | "proc-macro"));
            if !is_root {
                continue;
            }
            let src = target["src_path"]
                .as_str()
                .context("a target has no source path")?;
            let text = fs::read_to_string(src).with_context(|| format!("could not read {src}"))?;
            if !text.lines().any(|line| line.trim() == FORBID_UNSAFE) {
                problems.push(format!("`{name}`: {src} lacks `{FORBID_UNSAFE}`"));
            }
        }
    }
    Ok(())
}
