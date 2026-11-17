// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The PipeWire thread: the main loop, the registry and the commands from other
//! threads.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use ::pipewire as pw;
use helikodu_model::audio::{AudioSpec, DeviceCapabilities, DeviceId, DeviceInfo};
use pw::metadata::{Metadata, MetadataListener};
use pw::registry::{GlobalObject, RegistryRc};
use pw::spa::utils::dict::DictRef;
use pw::types::ObjectType;

use super::super::ring::{SampleReader, StreamShared};
use super::super::{EventSender, OutputError, OutputEvent, StreamConfig};
use super::stream::StreamEntry;
use super::{KIND, default_sink_name};

/// A request from another thread.
pub(super) enum Command {
    Open {
        id: u64,
        request: OpenRequest,
        reply: mpsc::Sender<Result<(), OutputError>>,
    },
    Start {
        id: u64,
        reply: mpsc::Sender<Result<(), OutputError>>,
    },
    Pause {
        id: u64,
        reply: mpsc::Sender<Result<(), OutputError>>,
    },
    Volume {
        id: u64,
        amplitude: f32,
        reply: mpsc::Sender<Result<(), OutputError>>,
    },
    Close {
        id: u64,
        reply: mpsc::Sender<Result<(), OutputError>>,
    },
    Quit,
}

/// Everything needed to create a stream on the PipeWire thread.
pub(super) struct OpenRequest {
    pub(super) config: StreamConfig,
    pub(super) spec: AudioSpec,
    pub(super) reader: SampleReader,
    pub(super) shared: Arc<StreamShared>,
}

/// The sink nodes the registry has announced, and the default sink.
#[derive(Debug, Default)]
pub(super) struct DeviceTable {
    /// By registry id, which is only valid while the node exists.
    nodes: BTreeMap<u32, DeviceInfo>,
    default_sink: Option<String>,
}

impl DeviceTable {
    pub(super) fn list(&self) -> Vec<DeviceInfo> {
        let mut list: Vec<DeviceInfo> = self
            .nodes
            .values()
            .map(|device| DeviceInfo {
                is_default: self.default_sink.as_deref() == Some(device.id.as_str()),
                ..device.clone()
            })
            .collect();
        list.sort_by(|a, b| a.description.cmp(&b.description).then(a.id.cmp(&b.id)));
        list
    }

    pub(super) fn contains(&self, name: &str) -> bool {
        self.nodes.values().any(|device| device.id.as_str() == name)
    }
}

/// State of the PipeWire thread, shared by its callbacks.
struct Thread {
    devices: Arc<Mutex<DeviceTable>>,
    events: EventSender,
    /// Whether the first registry listing is complete. Device events are sent only
    /// after it.
    listed: RefCell<bool>,
    streams: RefCell<HashMap<u64, StreamEntry>>,
    /// The listener first: it must be removed before the proxy goes.
    metadata: RefCell<Option<(MetadataListener, Metadata)>>,
}

impl Thread {
    fn send(&self, event: OutputEvent) {
        let _ = self.events.send(event);
    }

    fn global_added(&self, registry: &RegistryRc, global: &GlobalObject<&DictRef>) {
        let Some(props) = global.props else {
            return;
        };
        match global.type_ {
            ObjectType::Node => {
                if !props
                    .get("media.class")
                    .is_some_and(|class| class.starts_with("Audio/Sink"))
                {
                    return;
                }
                let Some(id) = props.get("node.name").and_then(|n| DeviceId::new(n).ok()) else {
                    return;
                };
                let description = ["node.description", "node.nick"]
                    .iter()
                    .find_map(|key| props.get(key))
                    .unwrap_or(id.as_str())
                    .to_owned();
                let device = DeviceInfo {
                    backend: KIND,
                    id,
                    description,
                    is_default: false,
                    capabilities: DeviceCapabilities::default(),
                };
                let event = {
                    let Ok(mut table) = self.devices.lock() else {
                        return;
                    };
                    let is_default = table.default_sink.as_deref() == Some(device.id.as_str());
                    table.nodes.insert(global.id, device.clone());
                    OutputEvent::DeviceAdded(DeviceInfo {
                        is_default,
                        ..device
                    })
                };
                if *self.listed.borrow() {
                    self.send(event);
                }
            }
            ObjectType::Link => {
                // A stream being opened is ready once it is linked to a device.
                let Some(output) = props
                    .get("link.output.node")
                    .and_then(|node| node.parse::<u32>().ok())
                else {
                    return;
                };
                for entry in self.streams.borrow().values() {
                    if entry.node_id() == output {
                        entry.linked();
                    }
                }
            }
            ObjectType::Metadata if props.get("metadata.name") == Some("default") => {
                self.watch_default(registry, global);
            }
            _ => {}
        }
    }

    fn global_removed(&self, id: u32) {
        let removed = match self.devices.lock() {
            Ok(mut table) => table.nodes.remove(&id),
            Err(_) => None,
        };
        if let Some(device) = removed
            && *self.listed.borrow()
        {
            self.send(OutputEvent::DeviceRemoved {
                backend: KIND,
                id: device.id,
            });
        }
    }

    /// Follows `default.audio.sink` in the "default" metadata object.
    fn watch_default(self: &Thread, registry: &RegistryRc, global: &GlobalObject<&DictRef>) {
        let Ok(metadata) = registry.bind::<Metadata, _>(global) else {
            return;
        };
        let devices = self.devices.clone();
        let events = self.events.clone();
        let listener = metadata
            .add_listener_local()
            .property(move |_subject, key, _type, value| {
                if key.is_some_and(|key| key != "default.audio.sink") {
                    return 0;
                }
                let name = value.and_then(default_sink_name).map(str::to_owned);
                let changed = match devices.lock() {
                    Ok(mut table) if table.default_sink != name => {
                        table.default_sink.clone_from(&name);
                        true
                    }
                    _ => false,
                };
                if changed {
                    let _ = events.send(OutputEvent::DefaultDeviceChanged {
                        backend: KIND,
                        id: name.and_then(|n| DeviceId::new(n).ok()),
                    });
                }
                0
            })
            .register();
        *self.metadata.borrow_mut() = Some((listener, metadata));
    }

    fn command(
        &self,
        core: &pw::core::CoreRc,
        mainloop: &pw::main_loop::MainLoopRc,
        command: Command,
    ) {
        match command {
            Command::Open { id, request, reply } => {
                let target_known =
                    |name: &str| self.devices.lock().is_ok_and(|table| table.contains(name));
                match StreamEntry::create(core, request, reply, self.events.clone(), &target_known)
                {
                    Ok(entry) => {
                        self.streams.borrow_mut().insert(id, entry);
                    }
                    Err((error, reply)) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::Start { id, reply } => {
                let _ = reply.send(self.with_stream(id, |entry| entry.set_active(true)));
            }
            Command::Pause { id, reply } => {
                let _ = reply.send(self.with_stream(id, |entry| entry.set_active(false)));
            }
            Command::Volume {
                id,
                amplitude,
                reply,
            } => {
                let _ = reply.send(self.with_stream(id, |entry| entry.set_volume(amplitude)));
            }
            Command::Close { id, reply } => {
                // Remove first, so the entry is not borrowed while it is dropped.
                let entry = self.streams.borrow_mut().remove(&id);
                if let Some(entry) = entry {
                    entry.close();
                }
                let _ = reply.send(Ok(()));
            }
            Command::Quit => mainloop.quit(),
        }
    }

    fn with_stream(
        &self,
        id: u64,
        f: impl FnOnce(&StreamEntry) -> Result<(), OutputError>,
    ) -> Result<(), OutputError> {
        match self.streams.borrow().get(&id) {
            Some(entry) => f(entry),
            None => Err(OutputError::Closed),
        }
    }

    /// The connection to the daemon is gone: every stream has failed.
    fn connection_lost(&self, reason: &str) {
        for entry in self.streams.borrow().values() {
            entry.fail();
        }
        self.send(OutputEvent::Error {
            backend: KIND,
            reason: format!("connection to PipeWire lost: {reason}"),
        });
    }
}

/// The body of the PipeWire thread. Sends the connection result on `ready` once
/// the first registry listing is complete, then serves commands until
/// [`Command::Quit`] or a lost connection.
pub(super) fn run(
    commands: pw::channel::Receiver<Command>,
    devices: Arc<Mutex<DeviceTable>>,
    events: EventSender,
    ready: mpsc::Sender<Result<(), String>>,
    alive: &AtomicBool,
) {
    serve(commands, devices, events, ready);
    alive.store(false, Ordering::Release);
}

fn serve(
    commands: pw::channel::Receiver<Command>,
    devices: Arc<Mutex<DeviceTable>>,
    events: EventSender,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let connected = (|| {
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        let registry = core.get_registry_rc()?;
        Ok::<_, pw::Error>((mainloop, context, core, registry))
    })();
    let (mainloop, _context, core, registry) = match connected {
        Ok(parts) => parts,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };

    let thread = Rc::new(Thread {
        devices,
        events,
        listed: RefCell::new(false),
        streams: RefCell::new(HashMap::new()),
        metadata: RefCell::new(None),
    });

    let _registry_listener = registry
        .add_listener_local()
        .global({
            let thread = thread.clone();
            let registry = registry.downgrade();
            move |global| {
                if let Some(registry) = registry.upgrade() {
                    thread.global_added(&registry, global);
                }
            }
        })
        .global_remove({
            let thread = thread.clone();
            move |id| thread.global_removed(id)
        })
        .register();

    // The first listing is complete when the daemon answers a sync.
    let pending = match core.sync(0) {
        Ok(seq) => seq,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    let ready = RefCell::new(Some(ready));
    let _core_listener = core
        .add_listener_local()
        .done({
            let thread = thread.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    *thread.listed.borrow_mut() = true;
                    if let Some(ready) = ready.borrow_mut().take() {
                        let _ = ready.send(Ok(()));
                    }
                }
            }
        })
        .error({
            let thread = thread.clone();
            let mainloop = mainloop.downgrade();
            move |id, _seq, _res, message| {
                // Errors about other objects (such as "unknown resource" for a link
                // that went away with its device) are not fatal; the stream's own
                // state reports what matters.
                if id == pw::core::PW_ID_CORE {
                    thread.connection_lost(message);
                    if let Some(mainloop) = mainloop.upgrade() {
                        mainloop.quit();
                    }
                }
            }
        })
        .register();

    let _receiver = commands.attach(mainloop.loop_(), {
        let thread = thread.clone();
        let core = core.clone();
        let weak_loop = mainloop.downgrade();
        move |command| {
            if let Some(mainloop) = weak_loop.upgrade() {
                thread.command(&core, &mainloop, command);
            }
        }
    });

    mainloop.run();

    // Close what is left before the connection goes.
    let entries: Vec<StreamEntry> = thread
        .streams
        .borrow_mut()
        .drain()
        .map(|(_, e)| e)
        .collect();
    for entry in entries {
        entry.close();
    }
    *thread.metadata.borrow_mut() = None;
}

#[cfg(test)]
mod tests {
    use helikodu_model::audio::OutputBackendKind;

    use super::*;

    fn device(name: &str, description: &str) -> DeviceInfo {
        DeviceInfo {
            backend: OutputBackendKind::PipeWire,
            id: DeviceId::new(name).unwrap(),
            description: description.into(),
            is_default: false,
            capabilities: DeviceCapabilities::default(),
        }
    }

    #[test]
    fn list_is_sorted_and_marks_the_default() {
        let mut table = DeviceTable::default();
        table.nodes.insert(70, device("b_sink", "Speakers"));
        table.nodes.insert(50, device("a_sink", "HDMI"));
        table.default_sink = Some("b_sink".into());
        let list = table.list();
        assert_eq!(list[0].id.as_str(), "a_sink");
        assert!(!list[0].is_default);
        assert!(list[1].is_default);
        assert!(table.contains("a_sink"));
        assert!(!table.contains("c_sink"));
    }
}
