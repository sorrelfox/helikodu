// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Waveform peaks for the seek bar, computed in the background for the tracks
//! that are played or about to be.
//!
//! [`WaveformService::start`] runs one thread at a lowered priority. It follows
//! the player's queue: the current entry first, then the next few, then the few
//! played last. For each of them without peaks it decodes the file
//! ([`helikodu_engine::analysis::waveform_peaks`]), keeps the result in the
//! [`PeakStore`] and announces it with [`WaveformEvent::Ready`].
//!
//! Nothing is computed while the library is scanned (a scan started from the
//! window or a rescan of the folder watcher): a computation under way stops, and
//! the thread waits for the scan to end.
//!
//! The store is a folder of small files (`<cache>/waveforms`), one per source,
//! named after the file's path, size and modification time and the part of the
//! file played: a changed file gets new peaks. Reading peaks marks them as used;
//! when the folder grows past its limit (Settings › Library › Storage, "Analysis
//! data"), the peaks used longest ago are removed first.

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use helikodu_engine::analysis::{AnalysisError, WAVEFORM_BINS, waveform_peaks};
use helikodu_engine::decode::DecoderRegistry;
use helikodu_model::library::{MediaSource, SourceSection};
use helikodu_model::settings::SettingsGroup;

use crate::Event;
use crate::bus::{EventBus, Subscription};
use crate::files::write_atomic;
use crate::library::{LibraryEvent, WatchEvent};
use crate::player::PlayerEvent;
use crate::settings::SettingsEvent;

/// The folder of the store inside the cache folder.
pub const WAVEFORM_FOLDER: &str = "waveforms";
/// The ending of a stored waveform's file.
const EXTENSION: &str = "peaks";
/// The first bytes of a stored waveform, with the format's version.
const MAGIC: &[u8; 4] = b"HKW1";
/// How long the thread waits before it looks again while a scan runs.
const SCAN_CHECK: Duration = Duration::from_millis(500);
/// The nice value of the thread: below interactive work.
const NICE: i32 = 10;

/// What the waveform service announces.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WaveformEvent {
    /// The peaks of `source` are in the store.
    Ready {
        /// The source.
        source: MediaSource,
    },
}

/// The stored waveforms; see the module documentation.
#[derive(Debug)]
pub struct PeakStore {
    dir: PathBuf,
    limit: AtomicU64,
    /// Held while files are written or removed, so that eviction sees them whole.
    writing: Mutex<()>,
}

impl PeakStore {
    /// The store in `dir` (created when the first waveform is kept), at most
    /// `limit` bytes.
    pub fn new(dir: impl Into<PathBuf>, limit: u64) -> Self {
        Self {
            dir: dir.into(),
            limit: AtomicU64::new(limit),
            writing: Mutex::new(()),
        }
    }

    /// The folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The limit in bytes.
    pub fn limit(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }

    /// Sets the limit and removes the oldest waveforms above it.
    pub fn set_limit(&self, limit: u64) {
        if self.limit.swap(limit, Ordering::Relaxed) != limit {
            self.evict();
        }
    }

    /// The file for `source` as it is now on disk; `None` when the file is not
    /// there.
    fn path_for(&self, source: &MediaSource) -> Option<PathBuf> {
        let meta = fs::metadata(source.path()).ok()?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .unwrap_or_default();
        let mut hash = Fnv::new();
        hash.write(source.path().as_os_str().as_bytes());
        match &source.section {
            None => hash.write(&[0]),
            Some(SourceSection::Subsong(n)) => {
                hash.write(&[1]);
                hash.write(&n.to_le_bytes());
            }
            Some(SourceSection::Span { start, end }) => {
                hash.write(&[2]);
                hash.write(&start.as_duration().as_nanos().to_le_bytes());
                hash.write(
                    &end.map_or(u128::MAX, |e| e.as_duration().as_nanos())
                        .to_le_bytes(),
                );
            }
        }
        hash.write(&meta.len().to_le_bytes());
        hash.write(&modified.as_nanos().to_le_bytes());
        Some(self.dir.join(format!("{:016x}.{EXTENSION}", hash.0)))
    }

    /// The peaks of `source` (values from 0 to 255), if they are stored; marks
    /// them as used.
    pub fn get(&self, source: &MediaSource) -> Option<Arc<[u8]>> {
        let path = self.path_for(source)?;
        let bytes = fs::read(&path).ok()?;
        let peaks = bytes.strip_prefix(MAGIC.as_slice())?;
        if peaks.is_empty() {
            return None;
        }
        // Used now: eviction keeps it longest.
        if let Ok(file) = File::options().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(Arc::from(peaks))
    }

    /// Whether `source` has stored peaks, without marking them as used.
    pub fn contains(&self, source: &MediaSource) -> bool {
        self.path_for(source).is_some_and(|p| p.is_file())
    }

    /// Keeps `peaks` for `source`, then removes the oldest waveforms above the
    /// limit.
    pub fn put(&self, source: &MediaSource, peaks: &[u8]) -> io::Result<()> {
        let Some(path) = self.path_for(source) else {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        };
        let mut bytes = Vec::with_capacity(MAGIC.len() + peaks.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(peaks);
        {
            let _writing = self.writing.lock().unwrap_or_else(|e| e.into_inner());
            write_atomic(&path, &bytes)?;
        }
        self.evict();
        Ok(())
    }

    /// The stored waveforms: each file with its size and last use.
    fn files(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == EXTENSION))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                meta.is_file()
                    .then(|| (e.path(), meta.len(), meta.modified().unwrap_or(UNIX_EPOCH)))
            })
            .collect()
    }

    /// The bytes the stored waveforms take.
    pub fn usage(&self) -> u64 {
        self.files().iter().map(|(_, len, _)| len).sum()
    }

    /// Removes the waveforms used longest ago until the store is within its limit.
    pub fn evict(&self) {
        let _writing = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        let mut files = self.files();
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        let limit = self.limit();
        if total <= limit {
            return;
        }
        files.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
        for (path, len, _) in files {
            if total <= limit {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }
}

/// The 64-bit FNV-1a hash: stable from one version of the program to the next.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// The sources that want peaks now, most wanted first.
pub type WantedSources = Box<dyn Fn() -> Vec<MediaSource> + Send>;

enum Input {
    /// The queue or the current entry changed.
    Queue,
    /// A scan started (`true`) or ended.
    Scanning(bool),
    /// The "Analysis data" limit changed, in bytes.
    Limit(u64),
    Shutdown,
}

/// The running waveform service. Dropping it stops its thread.
#[derive(Debug)]
pub struct WaveformService {
    inputs: Sender<Input>,
    thread: Option<JoinHandle<()>>,
    store: Arc<PeakStore>,
    // Kept for as long as the service lives; in a mutex only so the service can
    // be shared between threads.
    _subscription: Mutex<Subscription<()>>,
}

impl WaveformService {
    /// Starts the thread. `wanted` lists the sources to give peaks, most wanted
    /// first; it is asked again whenever the player's queue or entry changes.
    /// The player's, the library's and the settings' events come from `bus`.
    pub fn start(
        store: Arc<PeakStore>,
        bus: &EventBus<Event>,
        registry: DecoderRegistry,
        wanted: WantedSources,
    ) -> io::Result<Self> {
        let (inputs, mailbox) = mpsc::channel();
        let forward = inputs.clone();
        let mut scanning = false;
        let mut rescanning = false;
        let subscription = bus.subscribe_with(
            move |event: &Event| {
                let was = scanning || rescanning;
                let input = match event {
                    Event::Player(
                        PlayerEvent::QueueChanged { .. } | PlayerEvent::TrackChanged { .. },
                    ) => Some(Input::Queue),
                    Event::Library(LibraryEvent::Scan(_)) => {
                        scanning = true;
                        None
                    }
                    Event::Library(LibraryEvent::ScanEnded(_)) => {
                        scanning = false;
                        None
                    }
                    Event::Library(LibraryEvent::Watch(WatchEvent::RescanStarted { .. })) => {
                        rescanning = true;
                        None
                    }
                    Event::Library(LibraryEvent::Watch(
                        WatchEvent::RescanFinished { .. } | WatchEvent::Error { .. },
                    )) => {
                        rescanning = false;
                        None
                    }
                    Event::Settings(SettingsEvent::Changed { groups, settings })
                        if groups.contains(&SettingsGroup::Library) =>
                    {
                        Some(Input::Limit(
                            u64::from(settings.library.storage.analysis_mib) << 20,
                        ))
                    }
                    _ => None,
                };
                let now = scanning || rescanning;
                let input = if now == was {
                    input
                } else {
                    Some(Input::Scanning(now))
                };
                let input = input?;
                forward.send(input).err().map(|_| ())
            },
            None,
        );
        let worker = Worker {
            store: Arc::clone(&store),
            bus: bus.clone(),
            registry,
            wanted,
            scanning: Cell::new(false),
            shutdown: Cell::new(false),
            queue: VecDeque::new(),
            failed: HashSet::new(),
        };
        let thread = thread::Builder::new()
            .name("helikodu-waveforms".into())
            .spawn(move || worker.run(&mailbox))?;
        // The current queue, at once.
        let _ = inputs.send(Input::Queue);
        Ok(Self {
            inputs,
            thread: Some(thread),
            store,
            _subscription: Mutex::new(subscription),
        })
    }

    /// The store.
    pub fn store(&self) -> &Arc<PeakStore> {
        &self.store
    }

    /// The peaks of `source`, if they are stored (values from 0 to 255).
    pub fn peaks(&self, source: &MediaSource) -> Option<Arc<[u8]>> {
        self.store.get(source)
    }

    /// Asks the thread to look at the wanted sources again.
    pub fn refresh(&self) {
        let _ = self.inputs.send(Input::Queue);
    }
}

impl Drop for WaveformService {
    fn drop(&mut self) {
        let _ = self.inputs.send(Input::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Worker {
    store: Arc<PeakStore>,
    bus: EventBus<Event>,
    registry: DecoderRegistry,
    wanted: WantedSources,
    scanning: Cell<bool>,
    shutdown: Cell<bool>,
    /// The sources still to do, most wanted first.
    queue: VecDeque<MediaSource>,
    /// Sources that could not be decoded; not tried again in this run.
    failed: HashSet<MediaSource>,
}

impl Worker {
    fn run(mut self, mailbox: &Receiver<Input>) {
        let thread = rustix::thread::gettid();
        let _ = rustix::process::setpriority_process(Some(thread), NICE);
        while !self.shutdown.get() {
            // Wait for an input while there is nothing to do; while a scan runs,
            // look again now and then.
            let first = if self.queue.is_empty() {
                match mailbox.recv() {
                    Ok(input) => Some(input),
                    Err(_) => return,
                }
            } else if self.scanning.get() {
                match mailbox.recv_timeout(SCAN_CHECK) {
                    Ok(input) => Some(input),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                None
            };
            let mut inputs: Vec<Input> = first.into_iter().collect();
            inputs.extend(mailbox.try_iter());
            self.handle(inputs);
            if !self.scanning.get()
                && !self.shutdown.get()
                && let Some(source) = self.queue.pop_front()
            {
                self.compute(source, mailbox);
            }
        }
    }

    fn handle(&mut self, inputs: Vec<Input>) {
        for input in inputs {
            match input {
                Input::Shutdown => self.shutdown.set(true),
                Input::Queue => self.refill(),
                Input::Scanning(on) => self.scanning.set(on),
                Input::Limit(limit) => self.store.set_limit(limit),
            }
        }
    }

    fn refill(&mut self) {
        let mut queue = VecDeque::new();
        for source in (self.wanted)() {
            if !self.failed.contains(&source)
                && !queue.contains(&source)
                && !self.store.contains(&source)
            {
                queue.push_back(source);
            }
        }
        self.queue = queue;
    }

    fn compute(&mut self, source: MediaSource, mailbox: &Receiver<Input>) {
        if self.store.contains(&source) {
            return;
        }
        // Inputs are looked at between blocks: a scan that starts meanwhile, or
        // the end, stops the computation. The others wait until it is done.
        let mut pending = Vec::new();
        let result = {
            let pending = RefCell::new(&mut pending);
            let stop = || {
                for input in mailbox.try_iter() {
                    match input {
                        Input::Scanning(on) => self.scanning.set(on),
                        Input::Shutdown => self.shutdown.set(true),
                        other => pending.borrow_mut().push(other),
                    }
                }
                self.scanning.get() || self.shutdown.get()
            };
            waveform_peaks(&self.registry, &source, WAVEFORM_BINS, &stop)
        };
        match result {
            Ok(peaks) => {
                if self.store.put(&source, &peaks).is_ok() {
                    self.bus
                        .publish(&Event::Waveform(WaveformEvent::Ready { source }));
                }
            }
            // Done again once the scan is over.
            Err(AnalysisError::Cancelled) => self.queue.push_front(source),
            Err(_) => {
                self.failed.insert(source);
            }
        }
        self.handle(pending);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::library::{RescanReason, ScanEvent, ScanSummary};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hk-waveforms-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A mono 16-bit WAV of `seconds` of a quarter-scale square wave.
    fn wav(path: &Path, seconds: u32) {
        let rate = 8_000u32;
        let frames = rate * seconds;
        let data_len = frames * 2;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            let v: i16 = if (i / 20) % 2 == 0 { 8_192 } else { -8_192 };
            b.extend_from_slice(&v.to_le_bytes());
        }
        fs::write(path, b).unwrap();
    }

    #[test]
    fn the_store_keeps_peaks_per_file_version_and_evicts_the_oldest() {
        let d = dir("store");
        let music = d.join("music");
        fs::create_dir_all(&music).unwrap();
        let sources: Vec<MediaSource> = (0..4)
            .map(|i| {
                let p = music.join(format!("{i}.wav"));
                fs::write(&p, [i as u8; 10]).unwrap();
                MediaSource::file(p)
            })
            .collect();
        // Each stored waveform takes 4 + 100 bytes; three fit.
        let store = PeakStore::new(d.join(WAVEFORM_FOLDER), 3 * 104);
        assert_eq!(store.get(&sources[0]), None);
        for (i, source) in sources.iter().take(3).enumerate() {
            store.put(source, &[i as u8; 100]).unwrap();
            // Distinct times of use, oldest first.
            let path = store.path_for(source).unwrap();
            let at = UNIX_EPOCH + Duration::from_secs(1_000 + i as u64);
            File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
        assert_eq!(store.usage(), 3 * 104);
        // Using the first makes the second the oldest.
        assert_eq!(&*store.get(&sources[0]).unwrap(), &[0u8; 100][..]);
        store.put(&sources[3], &[3; 100]).unwrap();
        assert!(store.contains(&sources[0]));
        assert!(
            !store.contains(&sources[1]),
            "the one used longest ago goes"
        );
        assert!(store.contains(&sources[2]));
        assert!(store.contains(&sources[3]));
        assert!(store.usage() <= store.limit());

        // A lower limit removes more at once.
        store.set_limit(104);
        assert_eq!(store.usage(), 104);
        assert!(store.contains(&sources[0]) || store.contains(&sources[3]));

        // A changed file has no peaks until they are computed again.
        let kept = if store.contains(&sources[0]) {
            &sources[0]
        } else {
            &sources[3]
        };
        fs::write(kept.path(), [9u8; 11]).unwrap();
        assert!(!store.contains(kept));
        // A section of a file is a waveform of its own.
        let part = sources[2].clone().with_section(SourceSection::Subsong(1));
        assert_ne!(store.path_for(&part), store.path_for(&sources[2]));
        let _ = fs::remove_dir_all(&d);
    }

    fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn peaks_are_computed_for_the_queue_but_never_during_a_scan() {
        let d = dir("service");
        let files: Vec<MediaSource> = (0..2)
            .map(|i| {
                let p = d.join(format!("{i}.wav"));
                wav(&p, 3);
                MediaSource::file(p)
            })
            .collect();
        let bus: EventBus<Event> = EventBus::new();
        let ready = bus.subscribe_with(
            |e: &Event| match e {
                Event::Waveform(WaveformEvent::Ready { source }) => Some(source.clone()),
                _ => None,
            },
            None,
        );
        let wanted = Arc::new(Mutex::new(Vec::<MediaSource>::new()));
        let list = Arc::clone(&wanted);
        let store = Arc::new(PeakStore::new(d.join(WAVEFORM_FOLDER), 1 << 20));
        let service = WaveformService::start(
            Arc::clone(&store),
            &bus,
            DecoderRegistry::with_default_backends(),
            Box::new(move || list.lock().unwrap().clone()),
        )
        .unwrap();
        // A scan is under way when the queue asks for peaks.
        bus.publish(&Event::Library(LibraryEvent::Watch(
            WatchEvent::RescanStarted {
                reason: RescanReason::Changes,
                roots: Vec::new(),
                full: true,
            },
        )));
        *wanted.lock().unwrap() = files.clone();
        bus.publish(&Event::Player(PlayerEvent::QueueChanged { version: 1 }));
        thread::sleep(Duration::from_millis(400));
        assert!(
            ready.try_recv().is_err(),
            "nothing is computed during a scan"
        );
        assert_eq!(store.usage(), 0);

        // The scan ends: both get their peaks, the current one first.
        bus.publish(&Event::Library(LibraryEvent::Watch(
            WatchEvent::RescanFinished {
                reason: RescanReason::Changes,
                summary: ScanSummary::default(),
            },
        )));
        let first = ready.recv_timeout(Duration::from_secs(20)).unwrap();
        assert_eq!(first, files[0]);
        let second = ready.recv_timeout(Duration::from_secs(20)).unwrap();
        assert_eq!(second, files[1]);
        let peaks = service.peaks(&files[0]).unwrap();
        assert!(!peaks.is_empty() && peaks.len() <= WAVEFORM_BINS);
        assert!(
            peaks.iter().all(|&p| p.abs_diff(64) <= 1),
            "a quarter-scale square"
        );

        // Peaks already there are not computed again.
        service.refresh();
        thread::sleep(Duration::from_millis(200));
        assert!(ready.try_recv().is_err());

        // A scan that starts while a file is decoded stops it.
        let long = d.join("long.wav");
        wav(&long, 600);
        let long = MediaSource::file(long);
        *wanted.lock().unwrap() = vec![long.clone()];
        bus.publish(&Event::Library(LibraryEvent::ScanEnded(Err(String::new()))));
        service.refresh();
        bus.publish(&Event::Library(LibraryEvent::Scan(ScanEvent::Walking {
            root: d.clone(),
        })));
        thread::sleep(Duration::from_millis(400));
        assert!(!store.contains(&long), "stopped by the scan");
        bus.publish(&Event::Library(LibraryEvent::ScanEnded(Err(String::new()))));
        wait_for("the long file", Duration::from_secs(60), || {
            store.contains(&long)
        });
        drop(service);
        let _ = fs::remove_dir_all(&d);
    }
}
