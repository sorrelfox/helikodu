// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Entries of a backend's device list.

use serde::{Deserialize, Serialize};

use super::{DeviceId, DeviceKey, OutputBackendKind, SampleFormat, SampleRate};

/// An output device as a backend reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// The backend the device belongs to.
    pub backend: OutputBackendKind,
    /// The device's stable name, as stored in the settings.
    pub id: DeviceId,
    /// The description the backend gives for people, for example
    /// `Topping D10s Analog Stereo`. Not translated.
    pub description: String,
    /// Whether this is currently the backend's default device.
    pub is_default: bool,
    /// What the device accepts, as far as the backend can tell.
    pub capabilities: DeviceCapabilities,
}

impl DeviceInfo {
    /// The key of the device's per-device settings.
    pub fn key(&self) -> DeviceKey {
        DeviceKey::new(self.backend, self.id.clone())
    }
}

/// The formats a device accepts. An empty list means the backend cannot tell.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    /// Accepted sample rates, in ascending order.
    pub sample_rates: Vec<SampleRate>,
    /// Accepted sample formats.
    pub formats: Vec<SampleFormat>,
    /// Accepted channel counts, in ascending order.
    pub channel_counts: Vec<u16>,
}
