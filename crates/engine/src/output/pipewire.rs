// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The native PipeWire backend.
//!
//! PipeWire objects are not thread-safe, so one thread of ours owns the
//! connection: it runs PipeWire's main loop, keeps the device list up to date
//! from the registry, and creates and controls the streams. Other threads talk
//! to it through a channel attached to the loop. Each stream's process callback
//! runs on PipeWire's own real-time data thread.
//!
//! Devices are sink nodes, named by their `node.name`; object ids and serials
//! change when the session manager re-creates a node, so they are never stored.
//! A stream for a chosen device targets that name with `target.object` and sets
//! both `node.dont-reconnect` and `node.dont-fallback`, so it fails instead of
//! moving to another device when its device is missing or goes away. "System
//! default" sets no target and follows the desktop's default.

mod connection;
mod stream;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;
use std::{env, fmt};

use ::pipewire as pw;
use helikodu_model::audio::{AudioSpec, DeviceInfo, Endianness, OutputBackendKind, SampleFormat};

use self::connection::{Command, DeviceTable, OpenRequest};
use self::stream::PipeWireStream;
use super::ring::ring;
use super::{EventSender, OpenedStream, OutputBackend, OutputError, StreamConfig};

const KIND: OutputBackendKind = OutputBackendKind::PipeWire;
/// How long to wait for the connection and the first registry listing.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for a stream to be linked, or to fail.
const OPEN_TIMEOUT: Duration = Duration::from_secs(3);
/// How long to wait for the PipeWire thread to carry out a command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether a PipeWire daemon socket seems to be there.
pub(super) fn socket_exists() -> bool {
    let remote = env::var_os("PIPEWIRE_REMOTE").unwrap_or_else(|| "pipewire-0".into());
    let remote = PathBuf::from(remote);
    if remote.is_absolute() {
        return remote.exists();
    }
    ["PIPEWIRE_RUNTIME_DIR", "XDG_RUNTIME_DIR"]
        .iter()
        .filter_map(env::var_os)
        .any(|dir| PathBuf::from(dir).join(&remote).exists())
}

/// The way to the PipeWire thread, shared by the backend and its streams.
struct Link {
    commands: pw::channel::Sender<Command>,
    /// False once the PipeWire thread has ended.
    alive: Arc<AtomicBool>,
}

impl Link {
    /// Sends a command and waits for its reply.
    fn call(
        &self,
        timeout: Duration,
        what: &'static str,
        command: impl FnOnce(mpsc::Sender<Result<(), OutputError>>) -> Command,
    ) -> Result<(), OutputError> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(OutputError::Closed);
        }
        let (reply, answer) = mpsc::channel();
        if self.commands.send(command(reply)).is_err() {
            return Err(OutputError::Closed);
        }
        match answer.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(OutputError::Timeout(what)),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(OutputError::Closed),
        }
    }
}

/// A connection to the PipeWire daemon.
///
/// Device additions, removals and default-device changes are reported on the
/// event channel from the moment the backend is connected; streams report a lost
/// device and failures there too.
pub struct PipeWireBackend {
    link: Arc<Link>,
    devices: Arc<Mutex<DeviceTable>>,
    next_stream: AtomicU64,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for PipeWireBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PipeWireBackend")
            .field("alive", &self.link.alive.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl PipeWireBackend {
    /// Connects to the PipeWire daemon and reads the device list.
    pub fn connect(events: EventSender) -> Result<Self, OutputError> {
        pw::init();
        let (commands, receiver) = pw::channel::channel();
        let devices = Arc::new(Mutex::new(DeviceTable::default()));
        let alive = Arc::new(AtomicBool::new(true));
        let (ready, connected) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("helikodu-pipewire".into())
            .spawn({
                let devices = devices.clone();
                let alive = alive.clone();
                move || connection::run(receiver, devices, events, ready, &alive)
            })?;
        let backend = Self {
            link: Arc::new(Link { commands, alive }),
            devices,
            next_stream: AtomicU64::new(1),
            thread: Some(thread),
        };
        let unavailable = |reason: String| OutputError::BackendUnavailable {
            backend: KIND,
            reason,
        };
        match connected.recv_timeout(CONNECT_TIMEOUT) {
            Ok(Ok(())) => Ok(backend),
            Ok(Err(reason)) => Err(unavailable(reason)),
            Err(_) => Err(unavailable("no answer from the PipeWire daemon".into())),
        }
    }
}

impl Drop for PipeWireBackend {
    fn drop(&mut self) {
        if self.link.alive.load(Ordering::Acquire) {
            let _ = self.link.commands.send(Command::Quit);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl OutputBackend for PipeWireBackend {
    fn kind(&self) -> OutputBackendKind {
        KIND
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        if !self.link.alive.load(Ordering::Acquire) {
            return Err(OutputError::Closed);
        }
        let table = self.devices.lock().map_err(|_| OutputError::Closed)?;
        Ok(table.list())
    }

    fn open(&self, config: &StreamConfig) -> Result<OpenedStream, OutputError> {
        // PipeWire works in 32-bit float and converts for the device; offering
        // float means the server never needs to convert our samples first.
        let spec = AudioSpec {
            format: SampleFormat::F32(Endianness::NATIVE),
            ..config.spec
        };
        let (writer, reader, shared) = ring(spec, config.buffer);
        let id = self.next_stream.fetch_add(1, Ordering::Relaxed);
        let request = OpenRequest {
            config: config.clone(),
            spec,
            reader,
            shared: shared.clone(),
        };
        let stream =
            PipeWireStream::new(id, self.link.clone(), spec, shared, config.device.clone());
        // Dropping the handle on failure closes whatever was created.
        stream.open(request)?;
        Ok(OpenedStream {
            stream: Box::new(stream),
            writer,
        })
    }
}

/// The node name in a `default.audio.sink` metadata value, `{"name":"<node>"}`.
fn default_sink_name(value: &str) -> Option<&str> {
    let start = value.find("\"name\"")? + "\"name\"".len();
    let rest = value[start..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end]).filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_default_sink_metadata() {
        assert_eq!(
            default_sink_name(r#"{ "name": "alsa_output.pci-0000_00_1f.3.analog-stereo" }"#),
            Some("alsa_output.pci-0000_00_1f.3.analog-stereo")
        );
        assert_eq!(default_sink_name(r#"{"name":"x"}"#), Some("x"));
        assert_eq!(default_sink_name(r#"{"name":""}"#), None);
        assert_eq!(default_sink_name("{}"), None);
    }
}
