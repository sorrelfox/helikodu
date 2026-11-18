// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The PulseAudio backend, through cpal's PulseAudio host. It speaks the native
//! protocol over the local socket, to PulseAudio or to PipeWire's PulseAudio service.
//!
//! Devices are the sinks, named by their sink name (for example
//! `alsa_output.usb-Topping_D10s-00.analog-stereo`). "System default" opens the
//! server's default sink as it is when the stream opens.
//!
//! The server converts: any rate, channel count up to 32, and unsigned 8-bit,
//! 16-, 24- and 32-bit integer or float samples are accepted as asked. A stream always
//! has a fixed buffer of about 20 ms: with the server's default, its first requests
//! are almost two seconds long and empty the ring buffer.
//!
//! There is no stream volume, no exclusive mode and no notification of device
//! changes in cpal: the engine applies software volume, exclusive mode opens the sink
//! like shared mode, and a [`DeviceWatcher`] lists the sinks periodically. A stream
//! on a chosen sink is lost when the sink is no longer listed; the server may have
//! moved it to another sink by then, so from that moment it plays silence.

use cpal::traits::{DeviceTrait, HostTrait};
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, DeviceCapabilities, DeviceId, DeviceInfo, DeviceSelection,
    Endianness, OutputBackendKind, SampleFormat, SampleRate,
};

use super::cpal_stream::{self, Build, cpal_format};
use super::device_watch::{DeviceWatcher, Lister};
use super::{EventSender, OpenedStream, OutputBackend, OutputError, StreamConfig};

const KIND: OutputBackendKind = OutputBackendKind::PulseAudio;
/// The most channels a PulseAudio stream has.
const MAX_CHANNELS: u16 = 32;

/// The PulseAudio backend.
pub struct PulseBackend {
    host: cpal::Host,
    events: EventSender,
    watcher: DeviceWatcher,
}

impl std::fmt::Debug for PulseBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PulseBackend").finish_non_exhaustive()
    }
}

impl PulseBackend {
    /// Connects to the server named by the environment (`PULSE_SERVER`, or the
    /// socket in the runtime directory), reporting device changes and stream events
    /// on `events`.
    pub fn connect(events: EventSender) -> Result<Self, OutputError> {
        let host = connect_host()?;
        let watcher = DeviceWatcher::spawn(KIND, events.clone(), || {
            let host = connect_host().ok()?;
            let list: Lister = Box::new(move || list(&host));
            Some(list)
        });
        Ok(Self {
            host,
            events,
            watcher,
        })
    }

    /// The sink `device` names.
    fn sink(&self, device: &DeviceSelection) -> Result<cpal::Device, OutputError> {
        let not_found = || OutputError::DeviceNotFound {
            backend: KIND,
            device: device.clone(),
        };
        match device {
            DeviceSelection::SystemDefault => {
                self.host.default_output_device().ok_or_else(not_found)
            }
            DeviceSelection::Device(id) => self
                .host
                .output_devices()
                .map_err(|e| backend_error(&e))?
                .find(|sink| sink_name(sink).as_deref() == Some(id.as_str()))
                .ok_or_else(not_found),
        }
    }
}

impl OutputBackend for PulseBackend {
    fn kind(&self) -> OutputBackendKind {
        KIND
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        list(&self.host)
    }

    fn open(&self, config: &StreamConfig) -> Result<OpenedStream, OutputError> {
        let sink = self.sink(&config.device)?;
        let device_rate = sink
            .default_output_config()
            .ok()
            .and_then(|default| SampleRate::new(default.sample_rate()));
        let spec = negotiate(config, device_rate);
        let (stream, writer) = cpal_stream::build(Build {
            kind: KIND,
            device: &sink,
            config,
            spec,
            buffer: cpal::BufferSize::Fixed(buffer_frames(spec.rate)),
            events: self.events.clone(),
        })?;
        self.watcher.watch(&config.device, stream.shared());
        Ok(stream.opened(writer))
    }
}

/// Whether a PulseAudio server can be reached: its socket exists (or `PULSE_SERVER`
/// names one).
pub(super) fn server_exists() -> bool {
    cpal::available_hosts().contains(&cpal::HostId::PulseAudio)
}

fn connect_host() -> Result<cpal::Host, OutputError> {
    cpal::host_from_id(cpal::HostId::PulseAudio).map_err(|error| OutputError::BackendUnavailable {
        backend: KIND,
        reason: error.to_string(),
    })
}

fn backend_error(error: &cpal::Error) -> OutputError {
    OutputError::Backend {
        backend: KIND,
        reason: error.to_string(),
    }
}

/// The sink name of a cpal device.
fn sink_name(device: &cpal::Device) -> Option<String> {
    device.id().ok().map(|id| id.id().to_owned())
}

/// The sinks, with the default one marked.
fn list(host: &cpal::Host) -> Result<Vec<DeviceInfo>, OutputError> {
    let default = host.default_output_device().as_ref().and_then(sink_name);
    let mut devices: Vec<DeviceInfo> = host
        .output_devices()
        .map_err(|e| backend_error(&e))?
        .filter_map(|sink| {
            let name = sink_name(&sink)?;
            let id = DeviceId::new(name.clone()).ok()?;
            let description = sink
                .description()
                .map(|d| d.name().to_owned())
                .unwrap_or_else(|_| name.clone());
            let rate = sink
                .default_output_config()
                .ok()
                .and_then(|config| SampleRate::new(config.sample_rate()));
            Some(DeviceInfo {
                backend: KIND,
                is_default: default.as_deref() == Some(name.as_str()),
                id,
                description,
                capabilities: capabilities(rate),
            })
        })
        .collect();
    devices.sort_by(|a, b| a.description.cmp(&b.description).then(a.id.cmp(&b.id)));
    Ok(devices)
}

/// What a sink takes: every rate and channel count (the server converts), and the
/// sample formats cpal passes on. The sink's own rate comes first.
fn capabilities(rate: Option<SampleRate>) -> DeviceCapabilities {
    let native = Endianness::NATIVE;
    DeviceCapabilities {
        sample_rates: rate.into_iter().collect(),
        formats: vec![
            SampleFormat::F32(native),
            SampleFormat::S32(native),
            SampleFormat::S24(native),
            SampleFormat::S16(native),
            SampleFormat::U8,
        ],
        channel_counts: (1..=MAX_CHANNELS).collect(),
    }
}

/// The format to open: the request where the server takes it, otherwise the nearest
/// it takes. Without a rate request, the sink's own rate.
fn negotiate(config: &StreamConfig, device_rate: Option<SampleRate>) -> AudioSpec {
    let requested = config.spec;
    let native = Endianness::NATIVE;
    let format = if cpal_format(requested.format).is_some() {
        requested.format
    } else {
        match requested.format {
            SampleFormat::S16(_) => SampleFormat::S16(native),
            SampleFormat::S24(_) | SampleFormat::S24Packed(_) => SampleFormat::S24(native),
            SampleFormat::S32(_) => SampleFormat::S32(native),
            _ => SampleFormat::F32(native),
        }
    };
    let rate = match (config.request_rate, device_rate) {
        (false, Some(rate)) => rate,
        _ => requested.rate,
    };
    let layout = if requested.channel_count() <= MAX_CHANNELS {
        requested.layout
    } else {
        ChannelLayout::default_for_count(2).unwrap_or(requested.layout)
    };
    AudioSpec::new(format, rate, layout)
}

/// The fixed buffer of a stream at `rate`: about 20 ms, a power of two.
fn buffer_frames(rate: SampleRate) -> u32 {
    (rate.hz() / 50).next_power_of_two().clamp(256, 8192)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(format: SampleFormat, rate: u32, channels: u16) -> StreamConfig {
        StreamConfig::new(AudioSpec::new(
            format,
            SampleRate::new(rate).unwrap(),
            ChannelLayout::default_for_count(channels).unwrap(),
        ))
    }

    #[test]
    fn the_request_is_kept_where_the_server_takes_it() {
        let native = Endianness::NATIVE;
        let request = config(SampleFormat::S16(native), 44_100, 6);
        assert_eq!(negotiate(&request, SampleRate::new(48_000)), request.spec);

        let packed = negotiate(&config(SampleFormat::S24Packed(native), 96_000, 2), None);
        assert_eq!(packed.format, SampleFormat::S24(native));
        assert_eq!(packed.rate.hz(), 96_000);
        let double = negotiate(&config(SampleFormat::F64(native), 48_000, 2), None);
        assert_eq!(double.format, SampleFormat::F32(native));
    }

    #[test]
    fn without_a_rate_request_the_sink_rate_is_used() {
        let mut request = config(SampleFormat::F32(Endianness::NATIVE), 44_100, 2);
        request.request_rate = false;
        assert_eq!(
            negotiate(&request, SampleRate::new(48_000)).rate.hz(),
            48_000
        );
        assert_eq!(negotiate(&request, None).rate.hz(), 44_100);
    }

    #[test]
    fn the_buffer_is_about_twenty_milliseconds() {
        assert_eq!(buffer_frames(SampleRate::new(44_100).unwrap()), 1024);
        assert_eq!(buffer_frames(SampleRate::HZ_48000), 1024);
        assert_eq!(buffer_frames(SampleRate::new(192_000).unwrap()), 4096);
        assert_eq!(buffer_frames(SampleRate::new(8_000).unwrap()), 256);
    }
}
