// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Events from the output backends.

use std::sync::mpsc;

use helikodu_model::audio::{DeviceId, DeviceInfo, DeviceSelection, OutputBackendKind};

/// The sending end of the channel on which backends report [`OutputEvent`]s.
///
/// Several backends may share one channel. Events are sent from the backends'
/// own threads, never from a real-time callback.
pub type EventSender = mpsc::Sender<OutputEvent>;

/// Something a backend reports on its own.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputEvent {
    /// A device appeared.
    DeviceAdded(DeviceInfo),
    /// A device disappeared.
    DeviceRemoved {
        /// The backend.
        backend: OutputBackendKind,
        /// The device's stable name.
        id: DeviceId,
    },
    /// The backend's default device changed. `None` when there is none.
    DefaultDeviceChanged {
        /// The backend.
        backend: OutputBackendKind,
        /// The new default device.
        id: Option<DeviceId>,
    },
    /// The device of an open stream went away. The stream is now in
    /// [`StreamState::Lost`](super::StreamState::Lost); the engine applies its
    /// device-lost policy. The stream never moves to another device by itself.
    DeviceLost {
        /// The backend.
        backend: OutputBackendKind,
        /// The device the stream was playing to.
        device: DeviceSelection,
    },
    /// A stream or the backend failed.
    Error {
        /// The backend.
        backend: OutputBackendKind,
        /// What went wrong, for logs and diagnostics.
        reason: String,
    },
}
