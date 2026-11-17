// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli play`.

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::error::ErrorKind;
use clap::{Args, CommandFactory};
use helikodu_model::audio::{
    BackendSelection, DeviceSelection, OutputBackendKind, OutputSelection,
};
use helikodu_model::player::Volume;
use helikodu_model::settings::DeviceLostPolicy;
use helikodu_service::player::{
    EndReason, Player, PlayerCommand, PlayerConfig, PlayerEvent, SinkPacing,
};

use super::keys::{self, Action, RawMode};
use super::status::{Status, describe_output};
use super::{BackendArg, ModeArg};
use crate::{Cli, Outcome};

/// How long the interactive loop waits for keys before it looks at events again.
const KEY_WAIT: Duration = Duration::from_millis(30);

#[derive(Debug, Args)]
pub(crate) struct PlayArgs {
    /// The files to play, in order.
    #[arg(required = true, value_name = "FILE")]
    files: Vec<PathBuf>,
    /// The audio backend.
    #[arg(long, value_enum, default_value_t = BackendArg::Auto)]
    backend: BackendArg,
    /// The device's stable name, as `list-outputs` shows it [default: the backend's
    /// default device]
    #[arg(long, value_name = "ID")]
    device: Option<String>,
    /// Shared or exclusive use of the device.
    #[arg(long, value_enum, default_value_t = ModeArg::Shared)]
    mode: ModeArg,
    /// The volume control's position, from 0 to 100 [default: 100]
    #[arg(long, value_name = "0..100", value_parser = clap::value_parser!(u8).range(0..=100))]
    volume: Option<u8>,
    /// The file the wav backend writes. Needs `--backend wav`.
    #[arg(long, value_name = "PATH", required_if_eq("backend", "wav"))]
    wav_out: Option<PathBuf>,
}

impl PlayArgs {
    /// The backend, device and mode; a combination that makes no sense is a usage
    /// error, which ends the program.
    fn output(&self) -> OutputSelection {
        let usage = |kind: ErrorKind, message: &str| -> ! {
            let mut cli = Cli::command();
            cli.build();
            match cli.find_subcommand_mut("play") {
                Some(play) => play.error(kind, message).exit(),
                None => cli.error(kind, message).exit(),
            }
        };
        let device = match (&self.wav_out, self.backend) {
            (Some(path), BackendArg::Wav) => {
                if self.device.is_some() {
                    usage(
                        ErrorKind::ArgumentConflict,
                        "the wav backend writes to --wav-out; --device cannot be used with it",
                    );
                }
                let Some(name) = path.to_str() else {
                    usage(
                        ErrorKind::InvalidValue,
                        "the --wav-out path is not valid UTF-8",
                    );
                };
                name.parse()
                    .unwrap_or_else(|_| usage(ErrorKind::InvalidValue, "--wav-out is empty"))
            }
            (Some(_), _) => usage(
                ErrorKind::ArgumentConflict,
                "--wav-out can only be used with --backend wav",
            ),
            (None, _) => match &self.device {
                Some(name) => name
                    .parse()
                    .unwrap_or_else(|_| usage(ErrorKind::InvalidValue, "--device is empty")),
                None => DeviceSelection::SystemDefault,
            },
        };
        OutputSelection {
            backend: self.backend.into(),
            device,
            mode: self.mode.into(),
        }
    }
}

pub(crate) fn run(args: PlayArgs) -> Result<Outcome> {
    let output = args.output();
    let interactive = io::stdin().is_terminal();
    let mut config = PlayerConfig::new(output.clone());
    if let Some(volume) = args.volume {
        config.volume = Volume::clamped(f32::from(volume) / 100.0);
    }
    if output.backend == BackendSelection::Backend(OutputBackendKind::Wav) {
        // A file has no clock: write it as fast as it is decoded.
        config.pacing = SinkPacing::Freewheel;
    }
    if !interactive {
        // Nobody can choose another device or quit by key: end instead of pausing.
        config.output.on_device_lost = DeviceLostPolicy::Stop;
    }
    let volume = config.volume;
    let (player, events) = Player::spawn(config).context("cannot start playback")?;

    let raw = if interactive {
        Some(RawMode::enable().context("cannot set up the terminal")?)
    } else {
        None
    };
    let mut status = Status::new(
        raw.is_some(),
        io::stderr().is_terminal(),
        output.clone(),
        volume,
    );
    status.message(&format!("Output: {}", describe_output(&output)));
    if raw.is_some() {
        status.message(keys::HELP);
    }
    player.send(PlayerCommand::PlayFiles {
        files: args.files,
        start: 0,
    })?;
    let outcome = if raw.is_some() {
        interactive_loop(&player, &events, &mut status)?
    } else {
        loop {
            let event = events.recv().context("the player stopped unexpectedly")?;
            status.show(&event);
            if let Some(outcome) = outcome(&event) {
                break outcome;
            }
        }
    };
    drop(raw);
    Ok(outcome)
}

fn interactive_loop(
    player: &Player,
    events: &Receiver<PlayerEvent>,
    status: &mut Status,
) -> Result<Outcome> {
    loop {
        loop {
            match events.try_recv() {
                Ok(event) => {
                    status.show(&event);
                    if let Some(outcome) = outcome(&event) {
                        return Ok(outcome);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    bail!("the player stopped unexpectedly")
                }
            }
        }
        for action in keys::pressed(KEY_WAIT).context("cannot read the keyboard")? {
            match action {
                Action::Quit => {
                    status.clear();
                    return Ok(Outcome::Success);
                }
                Action::Command(command) => player.send(command)?,
            }
        }
    }
}

/// How the program ends after `event`, if it does.
fn outcome(event: &PlayerEvent) -> Option<Outcome> {
    let PlayerEvent::Ended {
        reason,
        played,
        failed,
    } = event
    else {
        return None;
    };
    Some(if *played == 0 && *failed > 0 {
        Outcome::NothingPlayable
    } else if *reason == EndReason::OutputFailed {
        Outcome::Failure
    } else {
        Outcome::Success
    })
}
