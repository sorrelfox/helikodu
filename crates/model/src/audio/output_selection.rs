// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The choice of where audio goes.

use serde::{Deserialize, Serialize};

use super::{BackendSelection, DeviceSelection, OutputMode};

/// Backend, device and mode: where the player sends audio.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct OutputSelection {
    /// The backend.
    pub backend: BackendSelection,
    /// The device of that backend.
    pub device: DeviceSelection,
    /// Shared or exclusive use of the device.
    pub mode: OutputMode,
}
