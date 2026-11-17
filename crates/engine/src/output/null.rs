// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The null backend: takes audio at the device rate and discards it. For tests and
//! for running without a sound device.

use std::io;

use helikodu_model::audio::{
    AudioSpec, DeviceCapabilities, DeviceId, DeviceInfo, DeviceSelection, Endianness,
    OutputBackendKind, SampleFormat,
};

use super::software_sink::{Pacing, SinkTarget, SoftwareStream};
use super::{OpenedStream, OutputBackend, OutputError, StreamConfig};

/// The name of the null backend's only device.
const DEVICE_NAME: &str = "null";

/// A backend with one device that discards everything.
///
/// It accepts any PCM format, rate and channel count as requested. A DSD request
/// is answered with 32-bit float.
#[derive(Debug, Clone, Default)]
pub struct NullBackend {
    pacing: Pacing,
}

impl NullBackend {
    /// A null backend that takes samples in real time, like a device.
    pub fn new() -> Self {
        Self::default()
    }

    /// A null backend with the given pacing.
    pub fn with_pacing(pacing: Pacing) -> Self {
        Self { pacing }
    }

    fn device_id() -> DeviceId {
        DeviceId::new(DEVICE_NAME).expect("not empty")
    }
}

impl OutputBackend for NullBackend {
    fn kind(&self) -> OutputBackendKind {
        OutputBackendKind::Null
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        Ok(vec![DeviceInfo {
            backend: OutputBackendKind::Null,
            id: Self::device_id(),
            description: "Null output (discards audio)".into(),
            is_default: true,
            capabilities: DeviceCapabilities {
                sample_rates: Vec::new(),
                formats: SampleFormat::ALL
                    .into_iter()
                    .filter(|f| !f.is_dsd())
                    .collect(),
                channel_counts: Vec::new(),
            },
        }])
    }

    fn open(&self, config: &StreamConfig) -> Result<OpenedStream, OutputError> {
        if let DeviceSelection::Device(id) = &config.device
            && *id != Self::device_id()
        {
            return Err(OutputError::DeviceNotFound {
                backend: OutputBackendKind::Null,
                device: config.device.clone(),
            });
        }
        let mut spec = config.spec;
        if spec.format.is_dsd() {
            spec = AudioSpec {
                format: SampleFormat::F32(Endianness::NATIVE),
                ..spec
            };
        }
        SoftwareStream::open(OutputBackendKind::Null, config, spec, self.pacing, Discard)
    }
}

struct Discard;

impl SinkTarget for Discard {
    fn write(&mut self, _: &[u8]) -> io::Result<()> {
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}
