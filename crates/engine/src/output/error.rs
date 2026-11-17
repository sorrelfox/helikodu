// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Errors of the output backends.

use std::io;

use helikodu_model::audio::{DeviceSelection, OutputBackendKind};
use helikodu_model::player::EngineError;
use thiserror::Error;

/// Something an output backend or stream could not do.
#[derive(Debug, Error)]
pub enum OutputError {
    /// The backend cannot be used on this system.
    #[error("the {backend} backend is not available: {reason}")]
    BackendUnavailable {
        /// The backend.
        backend: OutputBackendKind,
        /// Why not.
        reason: String,
    },
    /// The device does not exist.
    #[error("{device} does not exist on {backend}")]
    DeviceNotFound {
        /// The backend.
        backend: OutputBackendKind,
        /// The device.
        device: DeviceSelection,
    },
    /// Another program holds the device.
    #[error("{device} on {backend} is busy{}", owner.as_ref().map(|o| format!(" (held by {o})")).unwrap_or_default())]
    DeviceBusy {
        /// The backend.
        backend: OutputBackendKind,
        /// The device.
        device: DeviceSelection,
        /// The program that holds it, if known, for example `pipewire (pid 4121)`.
        owner: Option<String>,
    },
    /// The device accepts none of the formats the stream could use.
    #[error("{device} on {backend} does not accept the format: {reason}")]
    FormatUnsupported {
        /// The backend.
        backend: OutputBackendKind,
        /// The device.
        device: DeviceSelection,
        /// What was refused.
        reason: String,
    },
    /// The backend has no such feature.
    #[error("{0} is not supported by this backend")]
    Unsupported(&'static str),
    /// The stream was stopped, its device went away, or its backend shut down.
    #[error("the stream is closed")]
    Closed,
    /// The operation did not finish in time.
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    /// Reading or writing a file failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The backend reported another error.
    #[error("{backend}: {reason}")]
    Backend {
        /// The backend.
        backend: OutputBackendKind,
        /// What went wrong.
        reason: String,
    },
}

impl OutputError {
    /// The error as the engine reports it in its events.
    pub fn to_engine_error(&self) -> EngineError {
        match self {
            Self::BackendUnavailable { backend, reason } => EngineError::BackendUnavailable {
                backend: *backend,
                reason: reason.clone(),
            },
            Self::DeviceNotFound { backend, device }
            | Self::DeviceBusy {
                backend, device, ..
            } => EngineError::DeviceUnavailable {
                backend: *backend,
                device: device.clone(),
                reason: self.to_string(),
            },
            _ => EngineError::Output {
                reason: self.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_message_names_the_owner() {
        let device: DeviceSelection = "hw:CARD=D10s,DEV=0".parse().unwrap();
        let error = OutputError::DeviceBusy {
            backend: OutputBackendKind::Alsa,
            device: device.clone(),
            owner: Some("pipewire (pid 4121)".into()),
        };
        assert_eq!(
            error.to_string(),
            "hw:CARD=D10s,DEV=0 on alsa is busy (held by pipewire (pid 4121))"
        );
        assert!(matches!(
            error.to_engine_error(),
            EngineError::DeviceUnavailable { device: d, .. } if d == device
        ));
        let error = OutputError::DeviceBusy {
            backend: OutputBackendKind::Alsa,
            device,
            owner: None,
        };
        assert_eq!(error.to_string(), "hw:CARD=D10s,DEV=0 on alsa is busy");
    }
}
