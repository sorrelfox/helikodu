// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Errors the engine reports.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::audio::{DeviceSelection, OutputBackendKind};

/// A problem the engine reports in an event. The messages are technical details for
/// logs and diagnostics; front ends show their own translated text.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineError {
    /// No decoder could open the item, or it holds no audio.
    #[error("cannot play the file: {reason}")]
    Unplayable {
        /// What went wrong.
        reason: String,
    },
    /// Decoding stopped part of the way through the item.
    #[error("decoding failed: {reason}")]
    Decode {
        /// What went wrong.
        reason: String,
    },
    /// The command needs a loaded item and there is none.
    #[error("nothing is loaded")]
    NothingLoaded,
    /// The backend is not available on this system.
    #[error("the {backend} backend is not available: {reason}")]
    BackendUnavailable {
        /// The backend.
        backend: OutputBackendKind,
        /// What went wrong.
        reason: String,
    },
    /// The device could not be opened.
    #[error("cannot open {device} on {backend}: {reason}")]
    DeviceUnavailable {
        /// The backend.
        backend: OutputBackendKind,
        /// The device.
        device: DeviceSelection,
        /// What went wrong, for example that another program holds the device.
        reason: String,
    },
    /// The device went away during playback.
    #[error("{device} on {backend} disappeared")]
    DeviceLost {
        /// The backend.
        backend: OutputBackendKind,
        /// The device.
        device: DeviceSelection,
    },
    /// The output failed in another way.
    #[error("output failed: {reason}")]
    Output {
        /// What went wrong.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages() {
        let error = EngineError::DeviceUnavailable {
            backend: OutputBackendKind::Alsa,
            device: "hw:CARD=D10s,DEV=0".parse().unwrap(),
            reason: "busy".into(),
        };
        assert_eq!(
            error.to_string(),
            "cannot open hw:CARD=D10s,DEV=0 on alsa: busy"
        );
    }
}
