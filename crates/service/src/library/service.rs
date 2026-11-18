// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The library for a window: queries on a worker thread with their results handed
//! to callbacks, cover thumbnails, scans and the folder watcher, all announced on the
//! event bus.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use helikodu_library::TagWriteReport;
use helikodu_library::tag_editing::{BatchItem, BatchProgress};
use helikodu_library::{
    ArtworkOptions, ArtworkPool, CoverHash, CoverLocator, ScanEvent, ScanHandle, ScanSummary,
    Thumbnail, ThumbnailCache, ThumbnailCacheOptions, ThumbnailSize, WatchEvent, WatchOptions,
    Watcher, read_file_details,
};
use helikodu_model::library::{TrackId, TrackRef};
use helikodu_model::settings::{LibrarySettings, Settings};

use super::duplicates::DuplicateApplier;
use super::{
    AlbumDetail, AlbumId, AlbumQuery, AlbumSummary, ArtistQuery, ArtistSummary, GenreQuery,
    GenreSummary, LibraryError, LibraryOverview, MusicLibrary, Paged, ScanRequest, SearchPage,
    TrackInfo, TrackQuery, TrackSummary,
};
use super::{CopyGroup, DuplicateCounts, DuplicateGroup, FileId, KeepRule, MatchBy, TrashOutcome};
use crate::Event;
use crate::bus::EventBus;
use crate::jobs::loudness::{LoudnessEvent, LoudnessJob, LoudnessRequest};

/// What the library service announces.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum LibraryEvent {
    /// Progress of a scan started with [`LibraryService::scan`].
    Scan(ScanEvent),
    /// That scan ended: its summary, or why it failed.
    ScanEnded(Result<ScanSummary, String>),
    /// The folder watcher did or noticed something (its rescans included).
    Watch(WatchEvent),
    /// A thumbnail asked for with [`LibraryService::thumbnail`] is ready, or cannot
    /// be made (`path` is `None`).
    ThumbnailReady {
        /// The cover.
        hash: CoverHash,
        /// The size.
        size: ThumbnailSize,
        /// The file.
        path: Option<PathBuf>,
    },
    /// A track was rated (whole stars), or its rating cleared.
    RatingChanged {
        /// The track.
        track: TrackId,
        /// The stars.
        stars: Option<u8>,
    },
    /// The contents of the library changed (a scan or rescan wrote to it): lists
    /// should be fetched again.
    Changed,
    /// Progress of a loudness measurement started with
    /// [`LibraryService::measure_loudness`].
    Loudness(LoudnessEvent),
    /// A change of the `[library.duplicates]` settings has been applied to the
    /// library: what had to be read (when copies get tracks again), or why it
    /// failed. [`LibraryEvent::Changed`] follows.
    DuplicatesApplied(Result<ScanSummary, String>),
}

/// How the library service starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryServiceOptions {
    /// The thumbnail cache folder (normally `$XDG_CACHE_HOME/helikodu/thumbnails`);
    /// `None` makes no thumbnails.
    pub thumbnails: Option<PathBuf>,
    /// The size limit of the thumbnail cache, in bytes.
    pub thumbnail_bytes: u64,
    /// The scan worker program, when it is not beside the running program.
    pub worker_program: Option<PathBuf>,
}

impl Default for LibraryServiceOptions {
    fn default() -> Self {
        Self {
            thumbnails: None,
            thumbnail_bytes: helikodu_library::DEFAULT_CACHE_BYTES,
            worker_program: None,
        }
    }
}

/// How the watcher keeps the library folders current, from the `[library]`
/// settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchSettings {
    /// The folders and their rules.
    pub library: LibrarySettings,
    /// Whether every folder is scanned when watching starts.
    pub initial_scan: bool,
}

/// One queue entry as the queue panel shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueItemInfo {
    /// What the entry plays.
    pub track: TrackRef,
    /// The title, or `None` when neither library nor tags know one (show the file
    /// name).
    pub title: Option<String>,
    /// The artists, as credited.
    pub artist: Option<String>,
    /// The album title.
    pub album: Option<String>,
    /// The track number.
    pub track_no: Option<u32>,
    /// The length.
    pub duration: Option<Duration>,
    /// The cover, for a library track that has one.
    pub cover: Option<CoverHash>,
    /// The file.
    pub path: Option<PathBuf>,
}

type Job = Box<dyn FnOnce(&MusicLibrary) + Send>;

/// A scan running in the background.
#[derive(Debug)]
struct RunningScan {
    cancel: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// Messages to the watcher thread.
#[derive(Debug)]
enum WatchControl {
    Rescan(Option<PathBuf>),
    Stop,
}

#[derive(Debug)]
struct RunningWatcher {
    control: Sender<WatchControl>,
    thread: JoinHandle<()>,
}

/// The library for front ends that must not wait for it. See the module
/// documentation.
///
/// Query methods return at once; the callback runs later on the service's worker
/// thread with the result. A GLib front end sends it on to its main context.
pub struct LibraryService {
    library: MusicLibrary,
    jobs: Option<Sender<Job>>,
    worker: Option<JoinHandle<()>>,
    artwork: Option<ArtworkPool>,
    scan: Mutex<Option<RunningScan>>,
    watcher: Mutex<Option<RunningWatcher>>,
    loudness: Mutex<Option<LoudnessJob>>,
    duplicates: DuplicateApplier,
    bus: EventBus<Event>,
    worker_program: Option<PathBuf>,
}

impl std::fmt::Debug for LibraryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryService")
            .field("library", &self.library)
            .field("artwork", &self.artwork)
            .finish_non_exhaustive()
    }
}

impl LibraryService {
    /// Starts the worker thread, and the artwork pool when thumbnails are wanted.
    pub fn start(
        library: MusicLibrary,
        bus: EventBus<Event>,
        options: LibraryServiceOptions,
    ) -> Result<Self, LibraryError> {
        let io_error = |context: &str| {
            let context = context.to_owned();
            move |source| LibraryError::Io { context, source }
        };
        let (jobs, receiver) = mpsc::channel::<Job>();
        let for_worker = library.clone();
        let worker = thread::Builder::new()
            .name("helikodu-library".into())
            .spawn(move || {
                for job in receiver {
                    job(&for_worker);
                }
            })
            .map_err(io_error("cannot start the library thread"))?;
        let artwork = match options.thumbnails {
            None => None,
            Some(dir) => {
                let mut cache_options = ThumbnailCacheOptions::new(dir);
                cache_options.max_bytes = options.thumbnail_bytes;
                let cache = Arc::new(
                    ThumbnailCache::open(&cache_options)
                        .map_err(io_error("cannot open the thumbnail cache"))?,
                );
                let locator: Arc<dyn CoverLocator> = library.inner().clone();
                let events = bus.clone();
                Some(
                    ArtworkPool::start(cache, locator, ArtworkOptions::default(), move |made| {
                        events.publish(&Event::Library(LibraryEvent::ThumbnailReady {
                            hash: made.hash,
                            size: made.size,
                            path: made.result.ok(),
                        }));
                    })
                    .map_err(io_error("cannot start the artwork threads"))?,
                )
            }
        };
        let duplicates =
            DuplicateApplier::start(library.clone(), bus.clone(), options.worker_program.clone());
        Ok(Self {
            library,
            jobs: Some(jobs),
            worker: Some(worker),
            artwork,
            scan: Mutex::new(None),
            watcher: Mutex::new(None),
            loudness: Mutex::new(None),
            duplicates,
            bus,
            worker_program: options.worker_program,
        })
    }

    /// The library itself, for work that may block (not on a window's thread).
    pub fn library(&self) -> &MusicLibrary {
        &self.library
    }

    /// Runs `query` on the worker thread and hands its result to `reply` there.
    pub fn run<T, Q, R>(&self, query: Q, reply: R) -> Result<(), LibraryError>
    where
        T: Send + 'static,
        Q: FnOnce(&MusicLibrary) -> Result<T, LibraryError> + Send + 'static,
        R: FnOnce(Result<T, LibraryError>) + Send + 'static,
    {
        let job: Job = Box::new(move |library| reply(query(library)));
        self.jobs
            .as_ref()
            .ok_or(LibraryError::Gone)?
            .send(job)
            .map_err(|_| LibraryError::Gone)
    }

    /// A page of albums.
    pub fn albums(
        &self,
        query: AlbumQuery,
        reply: impl FnOnce(Result<Paged<AlbumSummary>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.albums(&query), reply)
    }

    /// An album with its discs and tracks.
    pub fn album(
        &self,
        album: AlbumId,
        reply: impl FnOnce(Result<Option<AlbumDetail>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.album(album), reply)
    }

    /// A page of artists.
    pub fn artists(
        &self,
        query: ArtistQuery,
        reply: impl FnOnce(Result<Paged<ArtistSummary>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.artists(&query), reply)
    }

    /// A page of genres.
    pub fn genres(
        &self,
        query: GenreQuery,
        reply: impl FnOnce(Result<Paged<GenreSummary>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.genres(&query), reply)
    }

    /// A page of tracks.
    pub fn tracks(
        &self,
        query: TrackQuery,
        reply: impl FnOnce(Result<Paged<TrackSummary>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.tracks(&query), reply)
    }

    /// A page of search results.
    pub fn search(
        &self,
        text: String,
        offset: usize,
        limit: usize,
        reply: impl FnOnce(Result<SearchPage, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.search(&text, offset, limit), reply)
    }

    /// Everything known about one track.
    pub fn track_info(
        &self,
        track: TrackId,
        reply: impl FnOnce(Result<Option<TrackInfo>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.track_info(track), reply)
    }

    /// The folders, totals and most played tracks.
    pub fn overview(
        &self,
        top: usize,
        reply: impl FnOnce(Result<LibraryOverview, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.overview(top), reply)
    }

    /// What the queue panel shows for `tracks`, in the same order: library tracks
    /// from the library, other files from their tags.
    pub fn describe(
        &self,
        tracks: Vec<TrackRef>,
        reply: impl FnOnce(Result<Vec<QueueItemInfo>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| Ok(describe(l, tracks)), reply)
    }

    /// Rates `track` in whole stars, or clears the rating; announced with
    /// [`LibraryEvent::RatingChanged`].
    pub fn rate(
        &self,
        track: TrackId,
        stars: Option<u8>,
        reply: impl FnOnce(Result<(), LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        let bus = self.bus.clone();
        self.run(
            move |l| {
                l.rate(track, stars)?;
                bus.publish(&Event::Library(LibraryEvent::RatingChanged {
                    track,
                    stars,
                }));
                Ok(())
            },
            reply,
        )
    }

    /// Writes tags into files and reads them again, as [`MusicLibrary::write_tags`]
    /// does with this service's worker program and the `[library]` and `[advanced]`
    /// settings of `settings`; announces [`LibraryEvent::Changed`] when a file was
    /// read again. Blocks until all files are done: call it off a window's thread.
    pub fn write_tags(
        &self,
        items: &[BatchItem],
        settings: &Settings,
        progress: &mut dyn FnMut(&BatchProgress),
    ) -> Result<TagWriteReport, LibraryError> {
        let report = self.library.write_tags(
            items,
            &settings.library,
            &settings.advanced,
            self.worker_program.as_deref(),
            progress,
        )?;
        if report.refresh.is_some() {
            self.bus.publish(&Event::Library(LibraryEvent::Changed));
        }
        Ok(report)
    }

    /// Applies the `[library.duplicates]` settings of `settings` in the background
    /// (unless they are applied already): skipping copies removes their tracks,
    /// adding them reads the copies again. The service does this at start and
    /// whenever the settings change; progress is announced as
    /// [`LibraryEvent::Scan`], the end as [`LibraryEvent::DuplicatesApplied`] and
    /// [`LibraryEvent::Changed`].
    pub fn apply_duplicate_settings(&self, settings: &LibrarySettings) {
        self.duplicates.apply(settings);
    }

    /// The group of copies of the file of `track`, or `None`.
    pub fn copies_of(
        &self,
        track: TrackId,
        reply: impl FnOnce(Result<Option<CopyGroup>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.copies_of(track), reply)
    }

    /// Makes `file` the main copy of its group; announces [`LibraryEvent::Changed`].
    pub fn set_main_copy(
        &self,
        file: FileId,
        reply: impl FnOnce(Result<(), LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        let bus = self.bus.clone();
        self.run(
            move |l| {
                l.set_main_copy(file)?;
                bus.publish(&Event::Library(LibraryEvent::Changed));
                Ok(())
            },
            reply,
        )
    }

    /// Moves the copies `files`, which the owner confirmed, to the desktop trash
    /// (see [`MusicLibrary::trash_copies`]); each file's result says where it went or
    /// why it stayed. Announces [`LibraryEvent::Changed`].
    pub fn trash_copies(
        &self,
        files: Vec<FileId>,
        reply: impl FnOnce(Result<Vec<TrashOutcome>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        let bus = self.bus.clone();
        self.run(
            move |l| {
                let outcomes = l.trash_copies(&files);
                bus.publish(&Event::Library(LibraryEvent::Changed));
                outcomes
            },
            reply,
        )
    }

    /// Moves files found by the duplicate finder by another measure than identity
    /// to the desktop trash (see [`MusicLibrary::trash_duplicates`]). Announces
    /// [`LibraryEvent::Changed`].
    pub fn trash_duplicates(
        &self,
        files: Vec<FileId>,
        reply: impl FnOnce(Result<Vec<TrashOutcome>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        let bus = self.bus.clone();
        self.run(
            move |l| {
                let outcomes = l.trash_duplicates(&files);
                bus.publish(&Event::Library(LibraryEvent::Changed));
                outcomes
            },
            reply,
        )
    }

    /// The groups of duplicates by `match_by` with the marks `keep` suggests.
    pub fn find_duplicates(
        &self,
        match_by: MatchBy,
        keep: KeepRule,
        reply: impl FnOnce(Result<Vec<DuplicateGroup>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.find_duplicates(match_by, keep), reply)
    }

    /// How many groups each measure of the duplicate finder finds.
    pub fn duplicate_counts(
        &self,
        reply: impl FnOnce(Result<DuplicateCounts, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(MusicLibrary::duplicate_counts, reply)
    }

    /// The thumbnail of a cover: the file when it is cached, or else scheduled
    /// (newest first) and announced with [`LibraryEvent::ThumbnailReady`]. Does no
    /// file I/O on the calling thread. Without a thumbnail cache, `Unavailable`.
    pub fn thumbnail(&self, hash: &CoverHash, size: ThumbnailSize) -> Thumbnail {
        match &self.artwork {
            Some(pool) => pool.thumbnail(hash, size),
            None => Thumbnail::Unavailable,
        }
    }

    /// Drops the waiting thumbnail requests for which `keep` is false, such as covers
    /// scrolled out of view.
    pub fn retain_thumbnails(&self, keep: impl FnMut(&CoverHash, ThumbnailSize) -> bool) {
        if let Some(pool) = &self.artwork {
            pool.retain(keep);
        }
    }

    /// Starts a scan in the background, announcing its progress with
    /// [`LibraryEvent::Scan`], then [`LibraryEvent::ScanEnded`] and
    /// [`LibraryEvent::Changed`]. A scan already running is cancelled first.
    pub fn scan(&self, mut request: ScanRequest) -> Result<(), LibraryError> {
        self.cancel_scan();
        if request.worker_program.is_none() {
            request.worker_program = self.worker_program.clone();
        }
        let handle = self.library.scan(request)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
        let bus = self.bus.clone();
        let thread = thread::Builder::new()
            .name("helikodu-scan-events".into())
            .spawn(move || forward_scan(handle, &stop, &bus))
            .map_err(|source| LibraryError::Io {
                context: "cannot start the scan".into(),
                source,
            })?;
        *lock(&self.scan) = Some(RunningScan { cancel, thread });
        Ok(())
    }

    /// Starts measuring loudness in the background (stopping an earlier
    /// measurement first). Its progress is announced as [`LibraryEvent::Loudness`],
    /// and [`LibraryEvent::Changed`] follows its end.
    pub fn measure_loudness(&self, mut request: LoudnessRequest) -> Result<(), LibraryError> {
        self.stop_loudness();
        if request.worker_program.is_none() {
            request.worker_program = self.worker_program.clone();
        }
        let bus = self.bus.clone();
        let job = LoudnessJob::start(&self.library, request, move |event| {
            let finished = matches!(event, LoudnessEvent::Finished(_));
            bus.publish(&Event::Library(LibraryEvent::Loudness(event)));
            if finished {
                bus.publish(&Event::Library(LibraryEvent::Changed));
            }
        })?;
        *lock(&self.loudness) = Some(job);
        Ok(())
    }

    /// Whether a loudness measurement is running.
    pub fn is_measuring_loudness(&self) -> bool {
        lock(&self.loudness)
            .as_ref()
            .is_some_and(|job| !job.is_finished())
    }

    /// Stops the running loudness measurement, if any, and waits until it has
    /// stopped. What it finished stays stored.
    pub fn stop_loudness(&self) {
        if let Some(job) = lock(&self.loudness).take() {
            job.cancel();
            let _ = job.join();
        }
    }

    /// Cancels the running scan, if any, and waits until it has stopped.
    pub fn cancel_scan(&self) {
        if let Some(running) = lock(&self.scan).take() {
            running.cancel.store(true, Ordering::SeqCst);
            let _ = running.thread.join();
        }
    }

    /// Starts watching the library folders (stopping an earlier watcher first). Its
    /// events are announced as [`LibraryEvent::Watch`].
    pub fn watch(&self, settings: &WatchSettings) -> Result<(), LibraryError> {
        self.stop_watching();
        let options = self.watch_options(settings)?;
        let watcher =
            Watcher::spawn(Arc::clone(self.library.inner()), options).map_err(|source| {
                LibraryError::Io {
                    context: "cannot start watching the library folders".into(),
                    source,
                }
            })?;
        let (control, messages) = mpsc::channel();
        let bus = self.bus.clone();
        let thread = thread::Builder::new()
            .name("helikodu-watch-events".into())
            .spawn(move || forward_watch(watcher, &messages, &bus))
            .map_err(|source| LibraryError::Io {
                context: "cannot start watching the library folders".into(),
                source,
            })?;
        *lock(&self.watcher) = Some(RunningWatcher { control, thread });
        Ok(())
    }

    /// Rescans one folder, or every folder with `None`, through the watcher. Does
    /// nothing while nothing is watched.
    pub fn rescan(&self, folder: Option<PathBuf>) {
        if let Some(running) = &*lock(&self.watcher) {
            let _ = running.control.send(WatchControl::Rescan(folder));
        }
    }

    /// Stops the watcher, if one runs.
    pub fn stop_watching(&self) {
        if let Some(running) = lock(&self.watcher).take() {
            let _ = running.control.send(WatchControl::Stop);
            let _ = running.thread.join();
        }
    }

    fn watch_options(&self, settings: &WatchSettings) -> Result<WatchOptions, LibraryError> {
        let library = &settings.library;
        let scan = super::scan_options(library, self.worker_program.as_deref())?;
        let mut options = WatchOptions::new(scan);
        options.initial_scan = settings.initial_scan;
        options.poll_interval = Duration::from_secs(u64::from(library.rescan_interval_min) * 60);
        options.grace_period =
            Duration::from_secs(u64::from(library.missing_grace_days) * 24 * 60 * 60);
        Ok(options)
    }
}

impl Drop for LibraryService {
    fn drop(&mut self) {
        self.duplicates.stop();
        self.stop_loudness();
        self.cancel_scan();
        self.stop_watching();
        self.jobs = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn forward_scan(handle: ScanHandle, cancel: &AtomicBool, bus: &EventBus<Event>) {
    let mut cancelled = false;
    loop {
        if !cancelled && cancel.load(Ordering::SeqCst) {
            handle.cancel();
            cancelled = true;
        }
        match handle.events().recv_timeout(Duration::from_millis(50)) {
            Ok(event) => bus.publish(&Event::Library(LibraryEvent::Scan(event))),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let result = handle.join().map_err(|error| error.to_string());
    bus.publish(&Event::Library(LibraryEvent::ScanEnded(result)));
    bus.publish(&Event::Library(LibraryEvent::Changed));
}

fn forward_watch(watcher: Watcher, control: &mpsc::Receiver<WatchControl>, bus: &EventBus<Event>) {
    loop {
        match control.try_recv() {
            Ok(WatchControl::Rescan(folder)) => watcher.rescan(folder),
            Ok(WatchControl::Stop) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match watcher.events().recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                let changed = matches!(
                    event,
                    WatchEvent::RescanFinished { .. }
                        | WatchEvent::Moved { .. }
                        | WatchEvent::MissingRemoved { .. }
                        | WatchEvent::RootMasked { .. }
                        | WatchEvent::RootUnmasked { .. }
                );
                bus.publish(&Event::Library(LibraryEvent::Watch(event)));
                if changed {
                    bus.publish(&Event::Library(LibraryEvent::Changed));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    watcher.stop();
}

fn describe(library: &MusicLibrary, tracks: Vec<TrackRef>) -> Vec<QueueItemInfo> {
    tracks
        .into_iter()
        .map(|track| match &track {
            TrackRef::Library(id) => describe_library(library, *id, track.clone()),
            TrackRef::File(source) => describe_file(source.path(), track.clone()),
        })
        .collect()
}

fn describe_library(library: &MusicLibrary, id: TrackId, track: TrackRef) -> QueueItemInfo {
    let details = library.inner().track_details(id).ok().flatten();
    let cover = library
        .inner()
        .track_cover(id)
        .ok()
        .flatten()
        .map(|cover| cover.hash);
    match details {
        Some(details) => QueueItemInfo {
            track,
            title: details.tags.title,
            artist: join(&details.tags.artists),
            album: details.tags.album,
            track_no: details.tags.track_no,
            duration: details.audio.duration,
            cover,
            path: Some(details.path),
        },
        None => empty_item(track, None),
    }
}

fn describe_file(path: &Path, track: TrackRef) -> QueueItemInfo {
    match read_file_details(path) {
        Ok(details) => QueueItemInfo {
            track,
            title: details.tags.title,
            artist: join(&details.tags.artists),
            album: details.tags.album,
            track_no: details.tags.track_no,
            duration: details.audio.duration,
            cover: None,
            path: Some(path.to_path_buf()),
        },
        Err(_) => empty_item(track, Some(path.to_path_buf())),
    }
}

fn empty_item(track: TrackRef, path: Option<PathBuf>) -> QueueItemInfo {
    QueueItemInfo {
        track,
        title: None,
        artist: None,
        album: None,
        track_no: None,
        duration: None,
        cover: None,
        path,
    }
}

fn join(names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| names.join(", "))
}
