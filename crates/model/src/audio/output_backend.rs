// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Output backends and how one is chosen.

use std::fmt;
use std::str::FromStr;

use crate::text_form::{ParseError, serde_as_text};

/// A way of sending audio to a device.
///
/// The text form is the lower-case name: `pipewire`, `pulseaudio`, `alsa`, `jack`,
/// `null` or `wav`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutputBackendKind {
    /// The PipeWire sound server.
    PipeWire,
    /// The PulseAudio sound server, or PipeWire's PulseAudio service.
    PulseAudio,
    /// Direct ALSA access.
    Alsa,
    /// The JACK server, or PipeWire's JACK service.
    Jack,
    /// Discards the audio. For tests.
    Null,
    /// Writes the audio to a WAV file. For tests.
    Wav,
}

impl OutputBackendKind {
    /// Every backend, in a fixed order.
    pub const ALL: [Self; 6] = [
        Self::PipeWire,
        Self::PulseAudio,
        Self::Alsa,
        Self::Jack,
        Self::Null,
        Self::Wav,
    ];

    /// The text form, for example `pipewire`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PipeWire => "pipewire",
            Self::PulseAudio => "pulseaudio",
            Self::Alsa => "alsa",
            Self::Jack => "jack",
            Self::Null => "null",
            Self::Wav => "wav",
        }
    }

    /// Whether the backend exists only for tests and development, and does not play
    /// sound.
    pub const fn is_for_testing(self) -> bool {
        matches!(self, Self::Null | Self::Wav)
    }
}

impl fmt::Display for OutputBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OutputBackendKind {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| ParseError::new("output backend", s))
    }
}

serde_as_text!(OutputBackendKind);

/// Which backend to use: a chosen one, or the first that is available.
///
/// The text form is `auto` or the backend's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BackendSelection {
    /// PipeWire if its daemon is reachable, otherwise PulseAudio if a server answers,
    /// otherwise ALSA.
    #[default]
    Automatic,
    /// This backend.
    Backend(OutputBackendKind),
}

impl fmt::Display for BackendSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Automatic => f.write_str("auto"),
            Self::Backend(kind) => kind.fmt(f),
        }
    }
}

impl FromStr for BackendSelection {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "auto" {
            return Ok(Self::Automatic);
        }
        s.parse()
            .map(Self::Backend)
            .map_err(|_| ParseError::new("output backend", s))
    }
}

serde_as_text!(BackendSelection);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_round_trips() {
        for kind in OutputBackendKind::ALL {
            assert_eq!(kind.as_str().parse::<OutputBackendKind>(), Ok(kind));
            let selection = BackendSelection::Backend(kind);
            assert_eq!(
                selection.to_string().parse::<BackendSelection>(),
                Ok(selection)
            );
        }
        assert_eq!(
            "auto".parse::<BackendSelection>(),
            Ok(BackendSelection::Automatic)
        );
        assert!("PipeWire".parse::<BackendSelection>().is_err());
    }
}
