// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli list-outputs`.

use std::io::{self, Write};

use anyhow::Result;
use clap::Args;
use helikodu_model::audio::{DeviceInfo, OutputBackendKind};
use helikodu_service::player::{list_outputs, resolve_backend, sound_backends};

use super::BackendArg;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct ListOutputsArgs {
    /// The backend to list [default: every backend that plays sound]
    #[arg(long, value_enum)]
    backend: Option<BackendArg>,
    /// Print the devices as a JSON array, for scripts.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: &ListOutputsArgs) -> Result<Outcome> {
    let backends = match args.backend {
        Some(backend) => vec![resolve_backend(backend.into())],
        None => sound_backends(),
    };
    let mut listed: Vec<(OutputBackendKind, Vec<DeviceInfo>)> = Vec::new();
    for backend in backends {
        match list_outputs(backend) {
            Ok(devices) => listed.push((backend, devices)),
            Err(error) => eprintln!("helikodu-cli: cannot list {backend} devices: {error}"),
        }
    }
    let mut out = io::stdout().lock();
    if args.json {
        let devices: Vec<&DeviceInfo> = listed.iter().flat_map(|(_, list)| list).collect();
        serde_json::to_writer_pretty(&mut out, &devices)?;
        writeln!(out)?;
    } else {
        for (backend, devices) in &listed {
            print_backend(&mut out, *backend, devices)?;
        }
    }
    out.flush()?;
    Ok(if listed.is_empty() {
        Outcome::Failure
    } else {
        Outcome::Success
    })
}

/// The devices of one backend, the default one marked with `*`.
fn print_backend(
    out: &mut impl Write,
    backend: OutputBackendKind,
    devices: &[DeviceInfo],
) -> io::Result<()> {
    writeln!(out, "{backend}:")?;
    if devices.is_empty() {
        writeln!(out, "  (no devices)")?;
    }
    let width = devices
        .iter()
        .map(|device| device.id.as_str().chars().count())
        .max()
        .unwrap_or(0);
    for device in devices {
        let mark = if device.is_default { '*' } else { ' ' };
        writeln!(
            out,
            "{mark} {:width$}  {}",
            device.id.as_str(),
            device.description
        )?;
    }
    Ok(())
}
