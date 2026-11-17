// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask stress`: the engine's underrun stress test, built with
//! optimisations.
//!
//! Plays near-silent material in real time for `--secs` seconds (default 60;
//! the extended checks use 3600) while every processor is busy, and fails on
//! any underrun. `--backend null` (the default) paces the null sink like a
//! device; `--backend pipewire` plays into a temporary PipeWire null sink, loaded
//! and unloaded with `pactl`. `--load N` sets the number of busy threads (default:
//! one per processor). See `crates/engine/tests/stress.rs`.

use std::process::Command;

use anyhow::{Context, Result, bail};

pub(crate) fn run(args: &[String]) -> Result<()> {
    let mut secs = String::from("60");
    let mut backend = String::from("null");
    let mut load = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .cloned()
                .with_context(|| format!("`{name}` needs a value"))
        };
        match arg.as_str() {
            "--secs" => secs = value("--secs")?,
            "--backend" => backend = value("--backend")?,
            "--load" => load = Some(value("--load")?),
            other => bail!("unknown option `{other}` for `stress`"),
        }
    }
    if secs.parse::<u64>().is_err() {
        bail!("`--secs` takes a whole number of seconds, not `{secs}`");
    }
    if !matches!(backend.as_str(), "null" | "pipewire") {
        bail!("`--backend` is `null` or `pipewire`, not `{backend}`");
    }

    let mut command = Command::new(crate::cargo());
    command
        .args([
            "test",
            "-p",
            "helikodu-engine",
            "--release",
            "--locked",
            "--test",
            "stress",
            "--",
            "--ignored",
            "--nocapture",
        ])
        .current_dir(crate::repository_root()?)
        .env("HELIKODU_STRESS_SECS", &secs)
        .env("HELIKODU_STRESS_BACKEND", &backend);
    if let Some(load) = load {
        command.env("HELIKODU_STRESS_LOAD", load);
    }
    println!("==> underrun stress test: {secs} s on {backend}");
    let status = command.status().context("could not start cargo")?;
    if !status.success() {
        bail!("the stress test failed");
    }
    Ok(())
}
