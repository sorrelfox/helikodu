// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Commands that use the core directly, without a running application.

pub(crate) mod duplicates;
pub(crate) mod info;
pub(crate) mod list_outputs;
pub(crate) mod loudness;
pub(crate) mod play;
pub(crate) mod scan;
pub(crate) mod search;
pub(crate) mod stats;

mod database;
pub(crate) mod format;
mod keys;
mod status;

use clap::ValueEnum;
use helikodu_model::audio::{BackendSelection, OutputBackendKind, OutputMode};
use helikodu_service::player::ReplayGainMode;

/// The `--backend` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum BackendArg {
    /// PipeWire if it is running, otherwise ALSA.
    Auto,
    /// The PipeWire sound server.
    Pipewire,
    /// Direct ALSA access.
    Alsa,
    /// Discards the audio.
    Null,
    /// Writes the audio to the WAV file given with `--wav-out`.
    Wav,
}

impl From<BackendArg> for BackendSelection {
    fn from(arg: BackendArg) -> Self {
        match arg {
            BackendArg::Auto => Self::Automatic,
            BackendArg::Pipewire => Self::Backend(OutputBackendKind::PipeWire),
            BackendArg::Alsa => Self::Backend(OutputBackendKind::Alsa),
            BackendArg::Null => Self::Backend(OutputBackendKind::Null),
            BackendArg::Wav => Self::Backend(OutputBackendKind::Wav),
        }
    }
}

/// The `--mode` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    /// Share the device with other applications.
    Shared,
    /// Hold the device alone, for bit-perfect output where the backend supports it.
    Exclusive,
}

impl From<ModeArg> for OutputMode {
    fn from(arg: ModeArg) -> Self {
        match arg {
            ModeArg::Shared => Self::Shared,
            ModeArg::Exclusive => Self::Exclusive,
        }
    }
}

/// The `--replaygain` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ReplayGainArg {
    /// No loudness normalization.
    Off,
    /// The track gain of each file.
    Track,
    /// The album gain, for files in album order.
    Album,
    /// The album gain where neighbouring files share their album, the track gain
    /// otherwise.
    Auto,
}

impl From<ReplayGainArg> for ReplayGainMode {
    fn from(arg: ReplayGainArg) -> Self {
        match arg {
            ReplayGainArg::Off => Self::Off,
            ReplayGainArg::Track => Self::Track,
            ReplayGainArg::Album => Self::Album,
            ReplayGainArg::Auto => Self::Auto,
        }
    }
}
