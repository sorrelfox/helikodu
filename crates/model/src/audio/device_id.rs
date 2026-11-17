// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stable names of output devices.

use std::fmt;
use std::str::FromStr;

use super::OutputBackendKind;
use crate::text_form::{ParseError, serde_as_text};

/// The stable name of an output device within one backend, as it is stored in the
/// settings.
///
/// What the name is depends on the backend:
///
/// - PipeWire: the node name, for example `alsa_output.usb-Topping_D10s-00.analog-stereo`.
///   Object ids and serials change when a node is re-created and are never used.
/// - ALSA: a PCM name with the card's ID string, never its index, for example
///   `hw:CARD=D10s,DEV=0`, or a PCM defined in the ALSA configuration.
/// - PulseAudio: the sink name.
/// - JACK: the client and port names.
///
/// Never empty.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(String);

impl DeviceId {
    /// A device name, or an error if `name` is empty.
    pub fn new(name: impl Into<String>) -> Result<Self, ParseError> {
        let name = name.into();
        if name.is_empty() {
            Err(ParseError::new("device name", &name))
        } else {
            Ok(Self(name))
        }
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for DeviceId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

serde_as_text!(DeviceId);

/// Which device of a backend to play to.
///
/// The text form is `default` or the device's name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum DeviceSelection {
    /// The desktop's default device, followed when it changes. On ALSA, the `default`
    /// PCM.
    #[default]
    SystemDefault,
    /// This device. If it is missing, playback does not move to another device.
    Device(DeviceId),
}

impl fmt::Display for DeviceSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SystemDefault => f.write_str("default"),
            Self::Device(id) => id.fmt(f),
        }
    }
}

impl FromStr for DeviceSelection {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "default" {
            Ok(Self::SystemDefault)
        } else {
            s.parse().map(Self::Device)
        }
    }
}

serde_as_text!(DeviceSelection);

/// A device of a particular backend, used as the key of per-device settings.
///
/// The text form is `<backend>:<device>`, for example `alsa:hw:CARD=D10s,DEV=0`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceKey {
    /// The backend the device belongs to.
    pub backend: OutputBackendKind,
    /// The device's name within that backend.
    pub device: DeviceId,
}

impl DeviceKey {
    /// A key from its two parts.
    pub const fn new(backend: OutputBackendKind, device: DeviceId) -> Self {
        Self { backend, device }
    }
}

impl fmt::Display for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.backend, self.device)
    }
}

impl FromStr for DeviceKey {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || ParseError::new("device key", s);
        let (backend, device) = s.split_once(':').ok_or_else(invalid)?;
        Ok(Self {
            backend: backend.parse().map_err(|_| invalid())?,
            device: device.parse().map_err(|_| invalid())?,
        })
    }
}

serde_as_text!(DeviceKey);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_key_splits_at_the_first_colon() {
        let key: DeviceKey = "alsa:hw:CARD=D10s,DEV=0".parse().unwrap();
        assert_eq!(key.backend, OutputBackendKind::Alsa);
        assert_eq!(key.device.as_str(), "hw:CARD=D10s,DEV=0");
        assert_eq!(key.to_string(), "alsa:hw:CARD=D10s,DEV=0");
    }

    #[test]
    fn invalid_device_keys() {
        for text in ["", "alsa", "alsa:", "coreaudio:x", ":hw:0"] {
            assert!(text.parse::<DeviceKey>().is_err(), "{text:?}");
        }
    }

    #[test]
    fn device_selection_text_form() {
        assert_eq!("default".parse(), Ok(DeviceSelection::SystemDefault));
        let node = "alsa_output.usb-Topping_D10s-00.analog-stereo";
        let selection: DeviceSelection = node.parse().unwrap();
        assert_eq!(selection.to_string(), node);
        assert!("".parse::<DeviceSelection>().is_err());
    }
}
