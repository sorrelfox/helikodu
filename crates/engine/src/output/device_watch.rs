// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Following the device list of a backend that sends no notifications of its own
//! (cpal's PulseAudio and JACK hosts): a thread lists the devices periodically,
//! reports what was added or removed and which device became the default, and
//! marks a stream lost when its device is no longer listed.
//!
//! The list is read every 250 ms while a stream is open on a chosen device, so a
//! device that goes away is noticed quickly, and every two seconds otherwise.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use helikodu_model::audio::{DeviceId, DeviceInfo, DeviceSelection, OutputBackendKind};

use super::ring::StreamShared;
use super::{EventSender, OutputError, OutputEvent, StreamState};

/// How often the list is read while a stream plays to a chosen device.
const WATCHING: Duration = Duration::from_millis(250);
/// How often the list is read otherwise.
const IDLE: Duration = Duration::from_secs(2);

/// Reads a backend's device list.
pub(super) type Lister = Box<dyn FnMut() -> Result<Vec<DeviceInfo>, OutputError> + Send>;

/// A stream whose device is watched.
struct Watched {
    device: DeviceId,
    shared: Weak<StreamShared>,
}

/// The streams to watch, shared with the thread.
#[derive(Default)]
struct Streams(Mutex<Vec<Watched>>);

impl Streams {
    /// Forgets the streams that have ended; returns whether any is left.
    fn any_open(&self) -> bool {
        let Ok(mut streams) = self.0.lock() else {
            return false;
        };
        streams.retain(|watched| {
            watched
                .shared
                .upgrade()
                .is_some_and(|shared| !shared.state().is_finished())
        });
        !streams.is_empty()
    }

    /// The open streams whose device is not in `present`.
    fn orphaned(
        &self,
        present: &BTreeMap<DeviceId, DeviceInfo>,
    ) -> Vec<(DeviceId, Arc<StreamShared>)> {
        let Ok(streams) = self.0.lock() else {
            return Vec::new();
        };
        streams
            .iter()
            .filter(|watched| !present.contains_key(&watched.device))
            .filter_map(|watched| Some((watched.device.clone(), watched.shared.upgrade()?)))
            .collect()
    }
}

/// The thread that follows a backend's devices. Stops when dropped.
pub(super) struct DeviceWatcher {
    streams: Arc<Streams>,
    thread: Option<(JoinHandle<()>, Arc<AtomicBool>)>,
}

impl DeviceWatcher {
    /// Starts following the list `list` reads; `connect` makes the lister on the
    /// thread, so that a connection to the server is not shared with other threads.
    pub(super) fn spawn(
        kind: OutputBackendKind,
        events: EventSender,
        connect: impl FnOnce() -> Option<Lister> + Send + 'static,
    ) -> Self {
        let streams = Arc::new(Streams::default());
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new()
            .name(format!("helikodu-{kind}-devices"))
            .spawn({
                let streams = streams.clone();
                let stop = stop.clone();
                move || {
                    if let Some(list) = connect() {
                        watch(kind, &events, list, &streams, &stop);
                    }
                }
            })
            .ok()
            .map(|thread| (thread, stop));
        Self { streams, thread }
    }

    /// Watches the device of a stream that plays to a chosen device: when the device
    /// is no longer listed, the stream is lost.
    pub(super) fn watch(&self, device: &DeviceSelection, shared: &Arc<StreamShared>) {
        let DeviceSelection::Device(id) = device else {
            return;
        };
        if let Ok(mut streams) = self.streams.0.lock() {
            streams.push(Watched {
                device: id.clone(),
                shared: Arc::downgrade(shared),
            });
        }
        if let Some((thread, _)) = &self.thread {
            // Read the list at the faster pace from now on.
            thread.thread().unpark();
        }
    }
}

impl Drop for DeviceWatcher {
    fn drop(&mut self) {
        if let Some((thread, stop)) = self.thread.take() {
            stop.store(true, Ordering::Release);
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for DeviceWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceWatcher").finish_non_exhaustive()
    }
}

/// The body of the watcher thread.
fn watch(
    kind: OutputBackendKind,
    events: &EventSender,
    mut list: Lister,
    streams: &Streams,
    stop: &AtomicBool,
) {
    let mut known = Known::default();
    let mut first = true;
    while !stop.load(Ordering::Acquire) {
        // A failed listing (the server went away) changes nothing; the streams
        // report their own failure.
        if let Ok(devices) = list() {
            let changes = known.update(kind, devices);
            if !first {
                for event in changes {
                    let _ = events.send(event);
                }
            }
            first = false;
            for (device, shared) in streams.orphaned(&known.devices) {
                if shared.transition(StreamState::Lost) {
                    let _ = events.send(OutputEvent::DeviceLost {
                        backend: kind,
                        device: DeviceSelection::Device(device),
                    });
                }
            }
        }
        let pause = if streams.any_open() { WATCHING } else { IDLE };
        thread::park_timeout(pause);
    }
}

/// The devices last seen, and the default among them.
#[derive(Debug, Default)]
struct Known {
    devices: BTreeMap<DeviceId, DeviceInfo>,
    default: Option<DeviceId>,
}

impl Known {
    /// Takes a new listing and returns the events that describe the change.
    fn update(&mut self, kind: OutputBackendKind, devices: Vec<DeviceInfo>) -> Vec<OutputEvent> {
        let mut events = Vec::new();
        let default = devices
            .iter()
            .find(|device| device.is_default)
            .map(|device| device.id.clone());
        let now: BTreeMap<DeviceId, DeviceInfo> = devices
            .into_iter()
            .map(|device| (device.id.clone(), device))
            .collect();
        for id in self.devices.keys() {
            if !now.contains_key(id) {
                events.push(OutputEvent::DeviceRemoved {
                    backend: kind,
                    id: id.clone(),
                });
            }
        }
        for (id, device) in &now {
            if !self.devices.contains_key(id) {
                events.push(OutputEvent::DeviceAdded(device.clone()));
            }
        }
        if default != self.default {
            events.push(OutputEvent::DefaultDeviceChanged {
                backend: kind,
                id: default.clone(),
            });
        }
        self.devices = now;
        self.default = default;
        events
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use helikodu_model::audio::{
        AudioSpec, ChannelLayout, DeviceCapabilities, SampleFormat, SampleRate,
    };

    use super::*;
    use crate::output::ring::ring;

    const KIND: OutputBackendKind = OutputBackendKind::PulseAudio;

    fn device(name: &str, is_default: bool) -> DeviceInfo {
        DeviceInfo {
            backend: KIND,
            id: DeviceId::new(name).unwrap(),
            description: name.to_uppercase(),
            is_default,
            capabilities: DeviceCapabilities::default(),
        }
    }

    #[test]
    fn changes_become_events() {
        let mut known = Known::default();
        let first = known.update(KIND, vec![device("speakers", true)]);
        assert_eq!(first.len(), 2, "{first:?}");

        let events = known.update(
            KIND,
            vec![device("speakers", false), device("headphones", true)],
        );
        assert_eq!(
            events,
            [
                OutputEvent::DeviceAdded(device("headphones", true)),
                OutputEvent::DefaultDeviceChanged {
                    backend: KIND,
                    id: Some(DeviceId::new("headphones").unwrap()),
                },
            ]
        );
        let events = known.update(KIND, vec![device("speakers", true)]);
        assert_eq!(
            events,
            [
                OutputEvent::DeviceRemoved {
                    backend: KIND,
                    id: DeviceId::new("headphones").unwrap(),
                },
                OutputEvent::DefaultDeviceChanged {
                    backend: KIND,
                    id: Some(DeviceId::new("speakers").unwrap()),
                },
            ]
        );
        assert!(
            known
                .update(KIND, vec![device("speakers", true)])
                .is_empty()
        );
    }

    #[test]
    fn a_stream_whose_device_goes_is_lost() {
        let present = Arc::new(AtomicBool::new(true));
        let (events, received) = mpsc::channel();
        let watcher = DeviceWatcher::spawn(KIND, events, {
            let present = present.clone();
            move || {
                let list: Lister = Box::new(move || {
                    let mut devices = vec![device("speakers", true)];
                    if present.load(Ordering::Acquire) {
                        devices.push(device("usb-dac", false));
                    }
                    Ok(devices)
                });
                Some(list)
            }
        });
        let spec = AudioSpec::new(
            SampleFormat::F32(helikodu_model::audio::Endianness::NATIVE),
            SampleRate::HZ_48000,
            ChannelLayout::default_for_count(2).unwrap(),
        );
        let (_writer, _reader, shared) = ring(spec, Duration::from_millis(100));
        let (_other_writer, _other_reader, follows_default) =
            ring(spec, Duration::from_millis(100));
        let dac = DeviceSelection::Device(DeviceId::new("usb-dac").unwrap());
        watcher.watch(&dac, &shared);
        watcher.watch(&DeviceSelection::SystemDefault, &follows_default);
        thread::sleep(WATCHING * 2);
        assert_eq!(shared.state(), StreamState::Ready);

        present.store(false, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        let lost = loop {
            let event = received
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if let OutputEvent::DeviceLost { device, .. } = event {
                break device;
            }
        };
        assert_eq!(lost, dac);
        assert_eq!(shared.state(), StreamState::Lost);
        assert_eq!(follows_default.state(), StreamState::Ready);
        drop(watcher);
    }
}
