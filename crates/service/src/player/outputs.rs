// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Output backends and their devices: listing them and moving between them.

use std::sync::mpsc;

use helikodu_engine::output::{automatic_backend, connect_backend};
use helikodu_model::audio::{
    BackendSelection, DeviceInfo, DeviceSelection, OutputBackendKind, OutputSelection,
};
use helikodu_model::player::EngineError;

use super::PlayerError;

/// The backends built into this program that play sound, in the order the automatic
/// choice prefers them.
pub fn sound_backends() -> Vec<OutputBackendKind> {
    let mut backends = Vec::new();
    if cfg!(feature = "pipewire") {
        backends.push(OutputBackendKind::PipeWire);
    }
    if cfg!(feature = "alsa") {
        backends.push(OutputBackendKind::Alsa);
    }
    backends
}

/// The backend `selection` stands for: for [`BackendSelection::Automatic`], PipeWire
/// if its daemon is reachable, otherwise ALSA.
pub fn resolve_backend(selection: BackendSelection) -> OutputBackendKind {
    match selection {
        BackendSelection::Automatic => automatic_backend(),
        BackendSelection::Backend(kind) => kind,
    }
}

/// The devices of `backend`, as it reports them now.
///
/// The WAV backend writes to a file named when playing and has no devices to list.
pub fn list_outputs(backend: OutputBackendKind) -> Result<Vec<DeviceInfo>, PlayerError> {
    if backend == OutputBackendKind::Wav {
        return Err(PlayerError::Output(EngineError::BackendUnavailable {
            backend,
            reason: "it writes to a file and has no devices".into(),
        }));
    }
    // Device changes are not followed here; nobody listens to the backend's events.
    let (events, _) = mpsc::channel();
    let connected =
        connect_backend(backend, &events).map_err(|e| PlayerError::Output(e.to_engine_error()))?;
    connected
        .devices()
        .map_err(|e| PlayerError::Output(e.to_engine_error()))
}

/// The devices to move between from `current`: those of every sound backend when
/// it plays sound, otherwise only those of its own test backend. Backends that are
/// not available are left out.
pub(super) fn output_choices(current: &OutputSelection) -> Vec<DeviceInfo> {
    let kind = resolve_backend(current.backend);
    let backends = if kind.is_for_testing() {
        vec![kind]
    } else {
        sound_backends()
    };
    backends
        .into_iter()
        .filter_map(|backend| list_outputs(backend).ok())
        .flatten()
        .collect()
}

/// The device after `current` in `devices`, keeping the mode, or `None` when there
/// is no other device. A current choice of the default device stands for the
/// device marked as default; a device not in the list moves to the first one.
pub fn next_output(current: &OutputSelection, devices: &[DeviceInfo]) -> Option<OutputSelection> {
    let kind = resolve_backend(current.backend);
    let position = devices.iter().position(|device| {
        device.backend == kind
            && match &current.device {
                DeviceSelection::SystemDefault => device.is_default,
                DeviceSelection::Device(id) => device.id == *id,
            }
    });
    let next = match position {
        Some(at) => (at + 1) % devices.len(),
        None => 0,
    };
    let device = devices.get(next)?;
    if position == Some(next) {
        return None;
    }
    Some(OutputSelection {
        backend: BackendSelection::Backend(device.backend),
        device: DeviceSelection::Device(device.id.clone()),
        mode: current.mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use helikodu_model::audio::{DeviceCapabilities, DeviceId, OutputMode};

    fn device(backend: OutputBackendKind, id: &str, is_default: bool) -> DeviceInfo {
        DeviceInfo {
            backend,
            id: DeviceId::new(id).unwrap(),
            description: id.to_uppercase(),
            is_default,
            capabilities: DeviceCapabilities::default(),
        }
    }

    fn selection(backend: OutputBackendKind, device: &str) -> OutputSelection {
        OutputSelection {
            backend: BackendSelection::Backend(backend),
            device: device.parse().unwrap(),
            mode: OutputMode::Exclusive,
        }
    }

    fn devices() -> Vec<DeviceInfo> {
        vec![
            device(OutputBackendKind::PipeWire, "speakers", true),
            device(OutputBackendKind::PipeWire, "dac", false),
            device(OutputBackendKind::Alsa, "hw:CARD=D10s,DEV=0", false),
        ]
    }

    #[test]
    fn moves_through_every_backend_and_wraps() {
        let devices = devices();
        let pipewire = OutputBackendKind::PipeWire;
        let next = next_output(&selection(pipewire, "speakers"), &devices).unwrap();
        assert_eq!(next, selection(pipewire, "dac"));
        let next = next_output(&next, &devices).unwrap();
        assert_eq!(
            next,
            selection(OutputBackendKind::Alsa, "hw:CARD=D10s,DEV=0")
        );
        let next = next_output(&next, &devices).unwrap();
        assert_eq!(next, selection(pipewire, "speakers"));
    }

    #[test]
    fn default_device_stands_for_the_marked_one() {
        let next = next_output(
            &selection(OutputBackendKind::PipeWire, "default"),
            &devices(),
        )
        .unwrap();
        assert_eq!(next, selection(OutputBackendKind::PipeWire, "dac"));
    }

    #[test]
    fn unknown_device_moves_to_the_first() {
        let next = next_output(&selection(OutputBackendKind::Alsa, "gone"), &devices()).unwrap();
        assert_eq!(next, selection(OutputBackendKind::PipeWire, "speakers"));
    }

    #[test]
    fn nowhere_to_go() {
        let only = [device(OutputBackendKind::Null, "null", true)];
        assert_eq!(
            next_output(&selection(OutputBackendKind::Null, "null"), &only),
            None
        );
        assert_eq!(
            next_output(&selection(OutputBackendKind::Null, "null"), &[]),
            None
        );
    }

    #[test]
    fn null_backend_lists_one_device() {
        let devices = list_outputs(OutputBackendKind::Null).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].backend, OutputBackendKind::Null);
    }

    #[test]
    fn wav_backend_has_no_list() {
        assert!(matches!(
            list_outputs(OutputBackendKind::Wav),
            Err(PlayerError::Output(EngineError::BackendUnavailable { .. }))
        ));
    }

    #[test]
    fn test_backends_choose_only_among_themselves() {
        let choices = output_choices(&selection(OutputBackendKind::Null, "null"));
        assert!(
            choices
                .iter()
                .all(|device| device.backend == OutputBackendKind::Null)
        );
    }
}
