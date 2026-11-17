// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The states of the engine's transport.

use std::fmt;

use serde::{Deserialize, Serialize};

/// What the engine's transport is doing.
///
/// ```text
/// Stopped ──play──> Loading ──first buffer──> Playing <──resume/pause──> Paused
/// Playing, Paused ──seek──> Seeking ──target reached──> Playing
/// Playing ──device or format change──> Reconfiguring ──reopened──> Playing
/// Playing ──end of the last item──> Draining ──buffer empty──> Stopped
/// Loading ──open failed──> Stopped;   Playing, Paused ──stop──> Stopped
/// ```
///
/// Serialized in lower case, for example `"playing"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportState {
    /// Nothing is playing and no device is held.
    #[default]
    Stopped,
    /// A source is being opened and its first audio decoded.
    Loading,
    /// Audio is reaching the device.
    Playing,
    /// Playback is held at a position.
    Paused,
    /// Moving to a new position; playing resumes when it is reached.
    Seeking,
    /// The output is being reopened for another device or format.
    Reconfiguring,
    /// Nothing more will be decoded; the buffered audio is still playing out.
    Draining,
}

impl TransportState {
    /// The name in lower case, as serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Loading => "loading",
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Seeking => "seeking",
            Self::Reconfiguring => "reconfiguring",
            Self::Draining => "draining",
        }
    }
}

impl fmt::Display for TransportState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_name_matches_as_str() {
        for state in [
            TransportState::Stopped,
            TransportState::Loading,
            TransportState::Playing,
            TransportState::Paused,
            TransportState::Seeking,
            TransportState::Reconfiguring,
            TransportState::Draining,
        ] {
            assert_eq!(
                serde_json::to_string(&state).unwrap(),
                format!("\"{state}\"")
            );
        }
    }
}
