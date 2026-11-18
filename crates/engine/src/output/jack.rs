// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JACK backend: cpal's JACK host plays, a second JACK client of the backend's
//! own lists the devices and connects the ports. It works with a JACK server and
//! with PipeWire's JACK service.
//!
//! JACK has no devices, only clients with ports. A device here is a client with
//! audio input ports (on a JACK server, `system` for the sound card; on PipeWire, one
//! client per sink), named by its client name. The stream's ports are connected to
//! that client's input ports in order, and the stream gets as many channels as both
//! have. "System default" is the `system` client, or the first client with physical
//! ports when there is none.
//!
//! **Connections.** cpal connects a new stream's ports to `system:playback_*` when it
//! builds the stream, and has no working option against it (the host's
//! `set_connect_automatically` does not reach the devices it has already created).
//! The backend therefore removes every connection of the new ports before the stream
//! starts, while it plays only silence, and connects them to the chosen device
//! itself, and waits until the server shows exactly those connections, so the
//! first period the stream plays already reaches the device. On PipeWire, running
//! the program with
//! `PIPEWIRE_PROPS='{ jack.self-connect-mode = ignore-all }'` makes the server ignore
//! cpal's own connections, so not even silence reaches the default device; the
//! backend's connections come from its second client and are not affected.
//!
//! **Finding the stream's ports.** cpal does not tell the name of the client it
//! registers: it is `cpal_client_<pid>_out`, or that name with a suffix the server
//! adds while another client has it. The ports a new stream registers are the
//! ports of this process's cpal clients the backend's client did not list before.
//! The server shows them to the other clients a little later (PipeWire's JACK
//! service a few milliseconds), so the backend waits for them; and a closed stream
//! waits until its ports are no longer listed, so that a new stream that gets the
//! same names is not taken for the old one.
//!
//! **Format.** JACK runs at the server's rate with 32-bit float samples; the engine
//! resamples to that rate. The buffer is the server's period. There is no stream
//! volume and no exclusive mode.
//!
//! **Devices that go away.** A [`DeviceWatcher`] lists the clients periodically; a
//! stream whose client is gone is lost (JACK has removed its connections, so it is
//! silent). A JACK server that shuts down loses every stream.
//!
//! Building the backend needs `jack.pc` (the JACK development files), although
//! `libjack` is only loaded at run time.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ::jack as jack_lib;
use cpal::traits::HostTrait;
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, DeviceCapabilities, DeviceId, DeviceInfo, DeviceSelection,
    Endianness, OutputBackendKind, SampleFormat, SampleRate,
};
use jack_lib::PortFlags;

use super::cpal_stream::{self, Build};
use super::device_watch::{DeviceWatcher, Lister};
use super::{EventSender, OpenedStream, OutputBackend, OutputError, StreamConfig};

const KIND: OutputBackendKind = OutputBackendKind::Jack;
/// The client name of the backend's own JACK client.
const PATCH_CLIENT: &str = "helikodu-ports";
/// The client that stands for the sound card on a JACK server, and for the default
/// sink on PipeWire.
const SYSTEM_CLIENT: &str = "system";
/// How long the server may take to show a stream's ports or connections to the
/// backend's client.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the backend looks again while it waits.
const SETTLE_POLL: Duration = Duration::from_millis(2);

/// The JACK backend.
pub struct JackBackend {
    /// cpal's JACK host. Behind a lock: building and closing a stream are not meant
    /// to run at once (the port lists before and after tell the new ports apart).
    host: Arc<Mutex<cpal::Host>>,
    /// The backend's own client, never activated: it lists and connects ports.
    patch: Arc<jack_lib::Client>,
    events: EventSender,
    watcher: DeviceWatcher,
}

impl std::fmt::Debug for JackBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JackBackend").finish_non_exhaustive()
    }
}

impl JackBackend {
    /// Connects to the running JACK server (it is never started), reporting device
    /// changes and stream events on `events`.
    pub fn connect(events: EventSender) -> Result<Self, OutputError> {
        let patch = patch_client()?;
        let host = cpal::host_from_id(cpal::HostId::Jack).map_err(|error| {
            OutputError::BackendUnavailable {
                backend: KIND,
                reason: error.to_string(),
            }
        })?;
        let watcher = DeviceWatcher::spawn(KIND, events.clone(), || {
            let client = patch_client().ok()?;
            let list: Lister = Box::new(move || Ok(list(&client)));
            Some(list)
        });
        Ok(Self {
            host: Arc::new(Mutex::new(host)),
            patch: Arc::new(patch),
            events,
            watcher,
        })
    }

    /// The connections of `port`, as the backend's client sees them.
    fn connections(&self, port: &str) -> Vec<String> {
        self.patch
            .port_by_name(port)
            .map(|p| p.get_connections())
            .unwrap_or_default()
    }

    /// Removes every connection of `port` and connects it to `target`.
    fn patch(&self, port: &str, target: Option<&String>) -> Result<(), OutputError> {
        let failed = |error: jack_lib::Error| OutputError::Backend {
            backend: KIND,
            reason: format!("cannot connect {port}: {error}"),
        };
        for other in self.connections(port) {
            self.patch
                .disconnect_ports_by_name(port, &other)
                .map_err(failed)?;
        }
        if let Some(target) = target {
            self.patch
                .connect_ports_by_name(port, target)
                .map_err(failed)?;
        }
        Ok(())
    }

    /// Waits until each port is connected to its target and nothing else, removing
    /// connections that show up late (cpal's own, made before the backend's).
    fn settle(&self, pairs: &[(&String, Option<&String>)]) -> Result<(), OutputError> {
        wait_for(SETTLE_TIMEOUT, || {
            let mut settled = true;
            for &(port, target) in pairs {
                let connections = self.connections(port);
                for other in connections.iter().filter(|c| Some(*c) != target) {
                    // An error is a connection already going away.
                    let _ = self.patch.disconnect_ports_by_name(port, other);
                    settled = false;
                }
                if target.is_some_and(|t| !connections.contains(t)) {
                    settled = false;
                }
            }
            settled.then_some(())
        })
        .ok_or_else(|| OutputError::Backend {
            backend: KIND,
            reason: "the server did not show the stream's connections in time".into(),
        })
    }
}

impl OutputBackend for JackBackend {
    fn kind(&self) -> OutputBackendKind {
        KIND
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        Ok(list(&self.patch))
    }

    fn open(&self, config: &StreamConfig) -> Result<OpenedStream, OutputError> {
        let targets = target_ports(&self.patch, &config.device).ok_or_else(|| {
            OutputError::DeviceNotFound {
                backend: KIND,
                device: config.device.clone(),
            }
        })?;
        let rate = SampleRate::new(self.patch.sample_rate()).ok_or_else(|| {
            OutputError::BackendUnavailable {
                backend: KIND,
                reason: "the server reports no sample rate".into(),
            }
        })?;
        let spec = negotiate(config.spec, rate, targets.len());

        let host = self.host.lock().map_err(|_| OutputError::Closed)?;
        let device =
            host.default_output_device()
                .ok_or_else(|| OutputError::BackendUnavailable {
                    backend: KIND,
                    reason: "cpal has no JACK output".into(),
                })?;
        let channels = usize::from(spec.channel_count());
        let before = own_ports(&self.patch);
        let (mut stream, writer) = cpal_stream::build(Build {
            kind: KIND,
            device: &device,
            config,
            spec,
            buffer: cpal::BufferSize::Default,
            events: self.events.clone(),
        })?;
        let ports = wait_for(SETTLE_TIMEOUT, || {
            new_client_ports(own_ports(&self.patch), &before, channels)
        });
        drop(host);
        let Some(ports) = ports else {
            return Err(OutputError::Backend {
                backend: KIND,
                reason: format!(
                    "the server did not show the ports of a stream of {channels} channels"
                ),
            });
        };
        stream.close_with(closer(
            Arc::clone(&self.host),
            Arc::clone(&self.patch),
            ports.clone(),
        ));
        // The stream plays silence until it is started, so whatever cpal connected
        // heard nothing.
        let pairs: Vec<(&String, Option<&String>)> = ports
            .iter()
            .zip(targets.iter().map(Some).chain(std::iter::repeat(None)))
            .collect();
        for &(port, target) in &pairs {
            self.patch(port, target)?;
        }
        self.settle(&pairs)?;
        self.watcher.watch(&config.device, stream.shared());
        Ok(stream.opened(writer))
    }
}

fn patch_client() -> Result<jack_lib::Client, OutputError> {
    jack_lib::Client::new(PATCH_CLIENT, jack_lib::ClientOptions::NO_START_SERVER)
        .map(|(client, _status)| client)
        .map_err(|error| OutputError::BackendUnavailable {
            backend: KIND,
            reason: error.to_string(),
        })
}

/// Closes a stream: drops it, then waits until its ports are no longer listed, while
/// no other stream is built.
fn closer(
    host: Arc<Mutex<cpal::Host>>,
    patch: Arc<jack_lib::Client>,
    ports: Vec<String>,
) -> cpal_stream::Closer {
    Box::new(move |stream| {
        let _host = host.lock().unwrap_or_else(PoisonError::into_inner);
        drop(stream);
        // Past the timeout the next stream fails to find its ports instead.
        let _ = wait_for(SETTLE_TIMEOUT, || {
            let listed = own_ports(&patch);
            (!ports.iter().any(|port| listed.contains(port))).then_some(())
        });
    })
}

/// Calls `ready` until it has a value or `timeout` has passed.
fn wait_for<T>(timeout: Duration, mut ready: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = ready() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(SETTLE_POLL);
    }
}

/// The output ports of this process's cpal clients, in port number order.
fn own_ports(patch: &jack_lib::Client) -> Vec<String> {
    let mut ports = patch.ports(
        Some(&own_pattern()),
        Some(audio_type()),
        PortFlags::IS_OUTPUT,
    );
    ports.sort_by_key(|name| port_number(name));
    ports
}

/// The ports of a stream of `channels` channels among `listed`: those of the first
/// client that has `channels` ports none of which was listed `before`.
fn new_client_ports(
    listed: Vec<String>,
    before: &[String],
    channels: usize,
) -> Option<Vec<String>> {
    let mut clients: Vec<(String, Vec<String>)> = Vec::new();
    for port in listed.into_iter().filter(|port| !before.contains(port)) {
        let name = client_of(&port);
        match clients.iter_mut().find(|(client, _)| client == name) {
            Some((_, ports)) => ports.push(port),
            None => clients.push((name.to_owned(), vec![port])),
        }
    }
    clients
        .into_iter()
        .map(|(_, ports)| ports)
        .find(|ports| ports.len() == channels)
}

/// The JACK port type of audio ports (JACK's `JACK_DEFAULT_AUDIO_TYPE`).
fn audio_type() -> &'static str {
    "32 bit float mono audio"
}

/// A port name pattern for the ports of this process's cpal clients.
fn own_pattern() -> String {
    format!("^cpal_client_{}_", std::process::id())
}

/// Whether `client` is one of this program's own JACK clients.
fn is_own(client: &str) -> bool {
    client == PATCH_CLIENT || client.starts_with("cpal_client_")
}

/// The number at the end of a port name (`out_3` is 3), for ordering.
fn port_number(port: &str) -> u32 {
    let digits = port.len() - port.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    port[port.len() - digits..].parse().unwrap_or(u32::MAX)
}

/// The client a port belongs to: its name up to the first colon.
fn client_of(port: &str) -> &str {
    port.split_once(':').map_or(port, |(client, _)| client)
}

/// The audio input ports of every client but the program's own, grouped by client
/// in the order the server lists them.
fn input_clients(client: &jack_lib::Client) -> Vec<(String, Vec<String>)> {
    let ports = client.ports(None, Some(audio_type()), PortFlags::IS_INPUT);
    group_by_client(ports)
}

fn group_by_client(ports: Vec<String>) -> Vec<(String, Vec<String>)> {
    let mut clients: Vec<(String, Vec<String>)> = Vec::new();
    for port in ports {
        let name = client_of(&port);
        if is_own(name) {
            continue;
        }
        match clients.iter_mut().find(|(client, _)| client == name) {
            Some((_, ports)) => ports.push(port),
            None => clients.push((name.to_owned(), vec![port])),
        }
    }
    clients
}

/// The client "System default" plays to: `system`, or the first one with physical
/// ports.
fn default_client(client: &jack_lib::Client, clients: &[(String, Vec<String>)]) -> Option<String> {
    if clients.iter().any(|(name, _)| name == SYSTEM_CLIENT) {
        return Some(SYSTEM_CLIENT.to_owned());
    }
    clients
        .iter()
        .find(|(_, ports)| {
            ports.iter().any(|port| {
                client
                    .port_by_name(port)
                    .is_some_and(|p| p.flags().contains(PortFlags::IS_PHYSICAL))
            })
        })
        .map(|(name, _)| name.clone())
}

/// The devices: every client with audio input ports.
fn list(client: &jack_lib::Client) -> Vec<DeviceInfo> {
    let clients = input_clients(client);
    let default = default_client(client, &clients);
    let rate = SampleRate::new(client.sample_rate());
    clients
        .into_iter()
        .filter_map(|(name, ports)| {
            let channels = u16::try_from(ports.len()).unwrap_or(u16::MAX);
            Some(DeviceInfo {
                backend: KIND,
                is_default: default.as_deref() == Some(name.as_str()),
                id: DeviceId::new(name.clone()).ok()?,
                description: name,
                capabilities: DeviceCapabilities {
                    sample_rates: rate.into_iter().collect(),
                    formats: vec![SampleFormat::F32(Endianness::NATIVE)],
                    channel_counts: vec![channels],
                },
            })
        })
        .collect()
}

/// The input ports a stream on `device` is connected to, or `None` when the device
/// is not there.
fn target_ports(client: &jack_lib::Client, device: &DeviceSelection) -> Option<Vec<String>> {
    let clients = input_clients(client);
    let name = match device {
        DeviceSelection::SystemDefault => default_client(client, &clients)?,
        DeviceSelection::Device(id) => id.as_str().to_owned(),
    };
    clients
        .into_iter()
        .find(|(client, _)| *client == name)
        .map(|(_, ports)| ports)
        .filter(|ports| !ports.is_empty())
}

/// The format of a stream: float at the server's rate, with as many channels as the
/// request and the device both have.
fn negotiate(requested: AudioSpec, rate: SampleRate, device_ports: usize) -> AudioSpec {
    let channels = u16::try_from(device_ports)
        .unwrap_or(u16::MAX)
        .clamp(1, requested.channel_count().max(1));
    let layout = if channels == requested.channel_count() {
        requested.layout
    } else {
        ChannelLayout::default_for_count(channels).unwrap_or(requested.layout)
    };
    AudioSpec::new(SampleFormat::F32(Endianness::NATIVE), rate, layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_grouped_by_client_without_our_own() {
        let ports = [
            "system:playback_1",
            "system:playback_2",
            "Built-in Audio Analog Stereo:playback_FL",
            "cpal_client_42_out:in_0",
            "helikodu-ports:x",
            "system:playback_3",
            "Built-in Audio Analog Stereo:playback_FR",
        ]
        .map(String::from)
        .to_vec();
        let clients = group_by_client(ports);
        assert_eq!(clients.len(), 2);
        assert_eq!(clients[0].0, "system");
        assert_eq!(
            clients[0].1,
            [
                "system:playback_1",
                "system:playback_2",
                "system:playback_3"
            ]
        );
        assert_eq!(clients[1].0, "Built-in Audio Analog Stereo");
        assert_eq!(clients[1].1.len(), 2);
    }

    #[test]
    fn ports_sort_by_their_number() {
        let mut ports = ["c:out_10", "c:out_2", "c:out_0"].map(String::from);
        ports.sort_by_key(|p| port_number(p));
        assert_eq!(ports, ["c:out_0", "c:out_2", "c:out_10"]);
        assert_eq!(client_of("a:b:c"), "a");
    }

    #[test]
    fn new_ports_are_those_of_one_new_client() {
        let before = ["cpal_client_7_out:out_0", "cpal_client_7_out:out_1"].map(String::from);
        let listed = [
            "cpal_client_7_out:out_0",
            "cpal_client_7_out:out_1",
            "cpal_client_7_out-183:out_0",
        ]
        .map(String::from)
        .to_vec();
        // The server has shown only one of the new client's ports so far.
        assert_eq!(new_client_ports(listed.clone(), &before, 2), None);
        let mut listed = listed;
        listed.push("cpal_client_7_out-183:out_1".into());
        assert_eq!(
            new_client_ports(listed, &before, 2).unwrap(),
            ["cpal_client_7_out-183:out_0", "cpal_client_7_out-183:out_1"]
        );
        // Once the old client is gone, a new one may get its names back.
        let listed = ["cpal_client_7_out:out_0", "cpal_client_7_out:out_1"]
            .map(String::from)
            .to_vec();
        assert_eq!(new_client_ports(listed.clone(), &[], 2).unwrap(), listed);
    }

    #[test]
    fn waiting_ends_with_a_value_or_the_timeout() {
        let mut calls = 0;
        assert_eq!(
            wait_for(Duration::from_secs(1), || {
                calls += 1;
                (calls == 3).then_some(calls)
            }),
            Some(3)
        );
        assert_eq!(wait_for(Duration::ZERO, || None::<()>), None);
    }

    #[test]
    fn channels_follow_the_device_and_the_rate_the_server() {
        let spec = AudioSpec::new(
            SampleFormat::S16(Endianness::NATIVE),
            SampleRate::new(44_100).unwrap(),
            ChannelLayout::default_for_count(6).unwrap(),
        );
        let opened = negotiate(spec, SampleRate::HZ_48000, 2);
        assert_eq!(opened.channel_count(), 2);
        assert_eq!(opened.rate, SampleRate::HZ_48000);
        assert_eq!(opened.format, SampleFormat::F32(Endianness::NATIVE));
        let opened = negotiate(spec, SampleRate::HZ_48000, 8);
        assert_eq!(opened.layout, spec.layout);
    }
}
