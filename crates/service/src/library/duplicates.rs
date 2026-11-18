// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Copies (byte-identical files) and the duplicate finder for the front ends, and
//! the `[library.duplicates]` settings applied to the library as they change.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread::{self, JoinHandle};

use helikodu_library::{
    CancelToken, CopyGroup, DuplicateCounts, DuplicateGroup, FileId, KeepRule, MatchBy, ScanEvent,
    ScanSummary, Trash, TrashOutcome,
};
use helikodu_model::library::TrackId;
use helikodu_model::settings::{DuplicateSettings, LibrarySettings, SettingsGroup};

use super::service::LibraryEvent;
use super::{LibraryError, MusicLibrary, scan_options};
use crate::Event;
use crate::bus::{EventBus, Subscription};
use crate::settings::SettingsEvent;

impl MusicLibrary {
    /// How copies are treated, as the library database has it.
    pub fn duplicate_settings(&self) -> Result<DuplicateSettings, LibraryError> {
        Ok(self.inner().duplicate_settings()?)
    }

    /// Applies the `[library.duplicates]` settings of `settings` to the library: when
    /// the scan setting changed, the copies lose their tracks or are read to get them
    /// again (with the scan rules of `settings` and the worker program). Blocks until
    /// done; scan events go to `events`.
    pub fn apply_duplicate_settings(
        &self,
        settings: &LibrarySettings,
        worker_program: Option<&Path>,
        events: Option<&Sender<ScanEvent>>,
        cancel: &CancelToken,
    ) -> Result<ScanSummary, LibraryError> {
        let mut options = scan_options(settings, worker_program)?;
        if options.roots.is_empty() {
            options.roots = self.inner().roots()?;
        }
        Ok(self
            .inner()
            .apply_duplicate_settings(settings.duplicates, &options, events, cancel)?)
    }

    /// The group of copies of the file of `track`: the main copy and the copies,
    /// with their paths, formats, sizes, library folders and whether two of them are
    /// one file reached by two paths. `None` if the file has no copies.
    pub fn copies_of(&self, track: TrackId) -> Result<Option<CopyGroup>, LibraryError> {
        Ok(self.inner().copies_of(track)?)
    }

    /// Every group of copies.
    pub fn copy_groups(&self) -> Result<Vec<CopyGroup>, LibraryError> {
        Ok(self.inner().copy_groups()?)
    }

    /// Makes `file` the main copy of its group.
    pub fn set_main_copy(&self, file: FileId) -> Result<(), LibraryError> {
        Ok(self.inner().set_main_copy(file)?)
    }

    /// Moves the copies `files` (confirmed by the owner) to the desktop trash, each
    /// after checking that it is not the main copy, is unchanged and still identical
    /// to the main copy, and is not the same file as another path. Never deletes.
    pub fn trash_copies(&self, files: &[FileId]) -> Result<Vec<TrashOutcome>, LibraryError> {
        let trash = Trash::of_user().ok_or(LibraryError::NoDataFolder)?;
        Ok(self.inner().trash_copies(files, &trash)?)
    }

    /// Moves files the duplicate finder found by another measure than identity to
    /// the desktop trash, each after checking that it is unchanged and not the same
    /// file as another path.
    pub fn trash_duplicates(&self, files: &[FileId]) -> Result<Vec<TrashOutcome>, LibraryError> {
        let trash = Trash::of_user().ok_or(LibraryError::NoDataFolder)?;
        Ok(self.inner().trash_duplicates(files, &trash)?)
    }

    /// The groups of duplicates by `match_by`, with the files `keep` suggests moving
    /// to the trash.
    pub fn find_duplicates(
        &self,
        match_by: MatchBy,
        keep: KeepRule,
    ) -> Result<Vec<DuplicateGroup>, LibraryError> {
        Ok(self.inner().find_duplicates(match_by, keep)?)
    }

    /// How many groups each measure of the duplicate finder finds.
    pub fn duplicate_counts(&self) -> Result<DuplicateCounts, LibraryError> {
        Ok(self.inner().duplicate_counts()?)
    }
}

/// Applies `[library.duplicates]` as the settings change, in the background and one
/// change at a time, announcing [`LibraryEvent::Scan`] progress, then
/// [`LibraryEvent::DuplicatesApplied`] and [`LibraryEvent::Changed`].
pub(crate) struct DuplicateApplier {
    inner: Arc<Inner>,
    // Kept for as long as the applier lives; in a mutex, as a receiver is not `Sync`.
    _subscription: Mutex<Subscription<()>>,
}

struct Inner {
    library: MusicLibrary,
    bus: EventBus<Event>,
    worker_program: Option<PathBuf>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    applied: Option<DuplicateSettings>,
    running: Option<JoinHandle<()>>,
    cancel: CancelToken,
}

impl std::fmt::Debug for DuplicateApplier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuplicateApplier").finish_non_exhaustive()
    }
}

impl DuplicateApplier {
    /// Starts following the settings events on `bus`.
    pub(crate) fn start(
        library: MusicLibrary,
        bus: EventBus<Event>,
        worker_program: Option<PathBuf>,
    ) -> Self {
        let inner = Arc::new(Inner {
            library,
            bus: bus.clone(),
            worker_program,
            state: Mutex::new(State::default()),
        });
        let weak: Weak<Inner> = Arc::downgrade(&inner);
        let subscription = bus.subscribe_with(
            move |event: &Event| {
                if let Event::Settings(SettingsEvent::Changed { groups, settings }) = event
                    && groups.contains(&SettingsGroup::Library)
                    && let Some(inner) = weak.upgrade()
                {
                    Inner::apply(&inner, &settings.library);
                }
                None
            },
            None,
        );
        Self {
            inner,
            _subscription: Mutex::new(subscription),
        }
    }

    /// Applies `settings` unless they are applied already.
    pub(crate) fn apply(&self, settings: &LibrarySettings) {
        Inner::apply(&self.inner, settings);
    }

    /// Stops a change being applied and waits for it.
    pub(crate) fn stop(&self) {
        let running = {
            let mut state = lock(&self.inner.state);
            state.cancel.cancel();
            state.running.take()
        };
        if let Some(running) = running {
            let _ = running.join();
        }
    }
}

impl Inner {
    fn apply(this: &Arc<Self>, settings: &LibrarySettings) {
        let mut state = lock(&this.state);
        if state.applied == Some(settings.duplicates) {
            return;
        }
        state.applied = Some(settings.duplicates);
        let previous = state.running.take();
        let cancel = state.cancel.clone();
        let inner = Arc::clone(this);
        let settings = settings.clone();
        let spawned = thread::Builder::new()
            .name("helikodu-duplicates".into())
            .spawn(move || {
                if let Some(previous) = previous {
                    let _ = previous.join();
                }
                if cancel.is_cancelled() {
                    return;
                }
                inner.run(&settings, &cancel);
            });
        match spawned {
            Ok(handle) => state.running = Some(handle),
            Err(_) => state.applied = None,
        }
    }

    fn run(&self, settings: &LibrarySettings, cancel: &CancelToken) {
        let (sender, receiver) = mpsc::channel::<ScanEvent>();
        let bus = self.bus.clone();
        let forward = thread::Builder::new()
            .name("helikodu-duplicates-events".into())
            .spawn(move || {
                for event in receiver {
                    bus.publish(&Event::Library(LibraryEvent::Scan(event)));
                }
            });
        let result = self
            .library
            .apply_duplicate_settings(
                settings,
                self.worker_program.as_deref(),
                Some(&sender),
                cancel,
            )
            .map_err(|e| e.to_string());
        drop(sender);
        if let Ok(forward) = forward {
            let _ = forward.join();
        }
        self.bus
            .publish(&Event::Library(LibraryEvent::DuplicatesApplied(result)));
        self.bus.publish(&Event::Library(LibraryEvent::Changed));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
