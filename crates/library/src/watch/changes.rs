// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning file-system events into rescans: what changed, coalesced until the events
//! quiet down.
//!
//! An event names a path; the rescan it asks for depends on what the path is:
//!
//! | Event | Rescan |
//! |---|---|
//! | a media file or CUE sheet created, written, changed, removed | the files of its folder |
//! | a folder created or removed | the folder and everything below it |
//! | the ignore marker created or removed | its folder and everything below it |
//! | something renamed | both sides as above; a pair with the same cookie is a move |
//! | the event queue overflowed | the whole library folder |
//! | opened, read, closed without writing | nothing |
//!
//! A folder holding a file that is still being written (changed, not yet closed) is
//! held back until the file is closed, or for [`STALE_WRITE`] at most, so a file
//! being copied in is read once, complete.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventKind};

use super::RescanReason;
use crate::scan::Scope;
use crate::scan::filter::classify;

/// How long a file that is being written holds back the rescan of its folder.
pub(crate) const STALE_WRITE: Duration = Duration::from_secs(30);

/// What an event means beyond the rescans it asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Notice {
    /// Nothing more.
    None,
    /// Events were lost; the whole library folder is rescanned.
    Overflow,
    /// The library folder itself was removed or renamed: is it still there?
    RootChanged,
}

/// The rescans one flush runs.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Batch {
    /// By the index of the library folder: the folder and the parts of it.
    pub(crate) scopes: BTreeMap<usize, (PathBuf, Vec<Scope>)>,
    /// Renames seen as pairs, `(from, to)`, in the order they happened.
    pub(crate) moves: Vec<(PathBuf, PathBuf)>,
    /// Why.
    pub(crate) reason: RescanReason,
}

impl Batch {
    pub(crate) fn is_empty(&self) -> bool {
        self.scopes.is_empty() && self.moves.is_empty()
    }
}

/// The changes waiting for the events to quiet down.
#[derive(Debug)]
pub(crate) struct Pending {
    ignore_marker: Option<OsString>,
    scopes: BTreeMap<usize, (PathBuf, Vec<Scope>)>,
    moves: Vec<(PathBuf, PathBuf)>,
    /// Paths renamed away, by the rename cookie, waiting for their other half.
    renames: HashMap<usize, PathBuf>,
    /// Files being written, since when.
    writing: HashMap<PathBuf, Instant>,
    reason: Option<RescanReason>,
    /// Flush at once rather than after the quiet period.
    immediate: bool,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Pending {
    pub(crate) fn new(ignore_marker: Option<OsString>) -> Self {
        Self {
            ignore_marker,
            scopes: BTreeMap::new(),
            moves: Vec::new(),
            renames: HashMap::new(),
            writing: HashMap::new(),
            reason: None,
            immediate: false,
            first: None,
            last: None,
        }
    }

    /// Whether nothing is waiting.
    pub(crate) fn is_empty(&self) -> bool {
        self.scopes.is_empty() && self.moves.is_empty()
    }

    /// Takes in `event` from the watch of the library folder `root` (number `index`).
    pub(crate) fn event(
        &mut self,
        index: usize,
        root: &Path,
        event: &Event,
        now: Instant,
    ) -> Notice {
        if event.need_rescan() {
            self.rescan(index, root, Scope::Root, RescanReason::Overflow, now);
            return Notice::Overflow;
        }
        let mut notice = Notice::None;
        for path in &event.paths {
            if path == root {
                if matches!(
                    event.kind,
                    EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
                ) {
                    notice = Notice::RootChanged;
                }
                continue;
            }
            if !path.starts_with(root) {
                continue;
            }
            self.path_event(index, root, path, event, now);
        }
        notice
    }

    fn path_event(&mut self, index: usize, root: &Path, path: &Path, event: &Event, now: Instant) {
        let kind = self.kind_of(path);
        match event.kind {
            EventKind::Access(AccessKind::Close(AccessMode::Write)) => {
                self.writing.remove(path);
                self.file_or_folder(index, root, path, kind, now);
            }
            EventKind::Create(CreateKind::Folder) => {
                self.add(index, root, Scope::Subtree(path.to_path_buf()), now);
            }
            EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any) => {
                if kind == PathKind::Media {
                    self.writing.insert(path.to_path_buf(), now);
                }
                self.file_or_folder(index, root, path, kind, now);
            }
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Metadata(_)) => {
                self.file_or_folder(index, root, path, kind, now);
            }
            EventKind::Remove(RemoveKind::File) => {
                self.writing.remove(path);
                self.file_or_folder(index, root, path, kind, now);
            }
            EventKind::Remove(_) => {
                self.writing.remove(path);
                self.gone(index, root, path, kind, now);
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                self.writing.remove(path);
                if let Some(cookie) = event.tracker() {
                    self.renames.insert(cookie, path.to_path_buf());
                }
                self.gone(index, root, path, kind, now);
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                if let Some(from) = event.tracker().and_then(|c| self.renames.remove(&c)) {
                    self.moves.push((from, path.to_path_buf()));
                }
                self.file_or_folder(index, root, path, kind, now);
            }
            // The pair is matched from its halves, also across library folders.
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {}
            EventKind::Modify(ModifyKind::Name(_)) => self.gone(index, root, path, kind, now),
            EventKind::Access(_)
            | EventKind::Modify(ModifyKind::Other)
            | EventKind::Any
            | EventKind::Other => {}
        }
    }

    /// The rescan for a path that exists, or was just written.
    fn file_or_folder(
        &mut self,
        index: usize,
        root: &Path,
        path: &Path,
        kind: PathKind,
        now: Instant,
    ) {
        match kind {
            PathKind::Media => self.add(index, root, Scope::Folder(parent(path)), now),
            PathKind::Marker => self.add(index, root, Scope::Subtree(parent(path)), now),
            PathKind::Other if path.is_dir() => {
                self.add(index, root, Scope::Subtree(path.to_path_buf()), now);
            }
            PathKind::Other => {}
        }
    }

    /// The rescan for a path that is gone, which may have been a folder.
    fn gone(&mut self, index: usize, root: &Path, path: &Path, kind: PathKind, now: Instant) {
        match kind {
            PathKind::Media => self.add(index, root, Scope::Folder(parent(path)), now),
            PathKind::Marker => self.add(index, root, Scope::Subtree(parent(path)), now),
            PathKind::Other => self.add(index, root, Scope::Subtree(path.to_path_buf()), now),
        }
    }

    fn kind_of(&self, path: &Path) -> PathKind {
        let Some(name) = path.file_name() else {
            return PathKind::Other;
        };
        if self.ignore_marker.as_deref() == Some(name) {
            PathKind::Marker
        } else if classify(name).is_some() {
            PathKind::Media
        } else {
            PathKind::Other
        }
    }

    fn add(&mut self, index: usize, root: &Path, scope: Scope, now: Instant) {
        let scope = match scope {
            Scope::Subtree(dir) if dir == root => Scope::Root,
            scope => scope,
        };
        self.scopes
            .entry(index)
            .or_insert_with(|| (root.to_path_buf(), Vec::new()))
            .1
            .push(scope);
        self.reason.get_or_insert(RescanReason::Changes);
        self.touch(now);
    }

    fn touch(&mut self, now: Instant) {
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    /// Asks for a rescan of `scope` of the library folder `root` (number `index`) for
    /// `reason`. Any reason but [`RescanReason::Changes`] runs it at once.
    pub(crate) fn rescan(
        &mut self,
        index: usize,
        root: &Path,
        scope: Scope,
        reason: RescanReason,
        now: Instant,
    ) {
        self.add(index, root, scope, now);
        if reason != RescanReason::Changes {
            if self.reason.is_none_or(|r| r == RescanReason::Changes) {
                self.reason = Some(reason);
            }
            self.immediate = true;
        }
    }

    /// Drops what is waiting for the library folder number `index`, which is no
    /// longer watched.
    pub(crate) fn forget_root(&mut self, index: usize) {
        if let Some((root, _)) = self.scopes.remove(&index) {
            self.writing.retain(|path, _| !path.starts_with(&root));
        }
        if self.is_empty() {
            self.first = None;
            self.last = None;
            self.immediate = false;
        }
    }

    /// When the waiting changes are due, if any are: after `debounce` without new
    /// events, or `max_delay` after the first.
    pub(crate) fn deadline(&self, debounce: Duration, max_delay: Duration) -> Option<Instant> {
        if self.is_empty() {
            return None;
        }
        let (first, last) = (self.first?, self.last?);
        if self.immediate {
            return Some(last);
        }
        Some((last + debounce).min(first + max_delay))
    }

    /// Takes the changes to rescan now. Parts holding a file that is still being
    /// written stay behind for later, unless it has been written for [`STALE_WRITE`].
    pub(crate) fn take(&mut self, now: Instant) -> Batch {
        self.writing
            .retain(|_, since| now.saturating_duration_since(*since) < STALE_WRITE);
        let mut batch = Batch {
            scopes: BTreeMap::new(),
            moves: std::mem::take(&mut self.moves),
            reason: self.reason.take().unwrap_or(RescanReason::Changes),
        };
        let mut held = BTreeMap::new();
        for (index, (root, scopes)) in std::mem::take(&mut self.scopes) {
            let (now_scopes, later): (Vec<Scope>, Vec<Scope>) = scopes
                .into_iter()
                .partition(|scope| !self.is_being_written(scope));
            if !now_scopes.is_empty() {
                batch.scopes.insert(index, (root.clone(), now_scopes));
            }
            if !later.is_empty() {
                held.insert(index, (root, later));
            }
        }
        self.scopes = held;
        self.renames.clear();
        self.immediate = false;
        if self.scopes.is_empty() {
            self.first = None;
            self.last = None;
        } else {
            // Look again after another quiet period.
            self.first = Some(now);
            self.last = Some(now);
            self.reason = Some(RescanReason::Changes);
        }
        batch
    }

    fn is_being_written(&self, scope: &Scope) -> bool {
        self.writing.keys().any(|path| match scope {
            Scope::Root => false,
            Scope::Subtree(dir) => path.starts_with(dir),
            Scope::Folder(dir) => path.parent() == Some(dir.as_path()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathKind {
    /// A media file or CUE sheet, by its name.
    Media,
    /// The ignore marker.
    Marker,
    /// Anything else: a folder, or a file the library does not take.
    Other,
}

fn parent(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use notify::event::{DataChange, Flag, MetadataKind};

    use super::*;

    const ROOT: &str = "/music";
    const DEBOUNCE: Duration = Duration::from_millis(500);
    const MAX_DELAY: Duration = Duration::from_secs(1);

    fn event(kind: EventKind, path: &str) -> Event {
        Event::new(kind).add_path(PathBuf::from(path))
    }

    fn pending() -> Pending {
        Pending::new(Some(".nomedia".into()))
    }

    fn feed(pending: &mut Pending, events: &[Event], now: Instant) -> Vec<Notice> {
        events
            .iter()
            .map(|e| pending.event(0, Path::new(ROOT), e, now))
            .collect()
    }

    fn scopes(batch: &Batch) -> Vec<Scope> {
        let (_, scopes) = &batch.scopes[&0];
        crate::scan::scope::normalize(Path::new(ROOT), scopes.iter().cloned())
    }

    fn folder(path: &str) -> Scope {
        Scope::Folder(path.into())
    }

    fn subtree(path: &str) -> Scope {
        Scope::Subtree(path.into())
    }

    #[test]
    fn file_events_rescan_their_folder() {
        let now = Instant::now();
        let mut p = pending();
        feed(
            &mut p,
            &[
                event(EventKind::Create(CreateKind::File), "/music/a/1.flac"),
                event(
                    EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                    "/music/a/1.flac",
                ),
                event(
                    EventKind::Access(AccessKind::Close(AccessMode::Write)),
                    "/music/a/1.flac",
                ),
                event(EventKind::Remove(RemoveKind::File), "/music/b/2.mp3"),
                event(
                    EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
                    "/music/c/3.cue",
                ),
                // Neither media nor an existing folder: nothing.
                event(EventKind::Create(CreateKind::File), "/music/a/cover.jpg"),
                event(EventKind::Remove(RemoveKind::File), "/music/a/x.part"),
                // Reads, by the scan itself among others: nothing.
                event(
                    EventKind::Access(AccessKind::Open(AccessMode::Any)),
                    "/music/d/4.flac",
                ),
                event(
                    EventKind::Access(AccessKind::Close(AccessMode::Read)),
                    "/music/d/4.flac",
                ),
            ],
            now,
        );
        let batch = p.take(now);
        assert_eq!(
            scopes(&batch),
            [folder("/music/a"), folder("/music/b"), folder("/music/c")]
        );
        assert_eq!(batch.reason, RescanReason::Changes);
        assert!(p.is_empty());
    }

    #[test]
    fn folder_and_marker_events_rescan_the_subtree() {
        let now = Instant::now();
        let mut p = pending();
        feed(
            &mut p,
            &[
                event(EventKind::Create(CreateKind::Folder), "/music/new"),
                event(EventKind::Create(CreateKind::File), "/music/new/1.flac"),
                event(EventKind::Remove(RemoveKind::Folder), "/music/old"),
                event(EventKind::Create(CreateKind::File), "/music/x/.nomedia"),
                event(EventKind::Remove(RemoveKind::Any), "/music/gone"),
            ],
            now,
        );
        assert_eq!(
            scopes(&p.take(now)),
            [
                subtree("/music/gone"),
                subtree("/music/new"),
                subtree("/music/old"),
                subtree("/music/x")
            ]
        );
    }

    #[test]
    fn rename_pairs_are_moves() {
        let now = Instant::now();
        let mut p = pending();
        let from = |path: &str, cookie| {
            event(EventKind::Modify(ModifyKind::Name(RenameMode::From)), path).set_tracker(cookie)
        };
        let to = |path: &str, cookie| {
            event(EventKind::Modify(ModifyKind::Name(RenameMode::To)), path).set_tracker(cookie)
        };
        feed(
            &mut p,
            &[
                from("/music/a/1.flac", 7),
                to("/music/b/1.flac", 7),
                event(
                    EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                    "/music/a/1.flac",
                )
                .add_path("/music/b/1.flac".into())
                .set_tracker(7),
                // Moved out of the library: no pair, the folder is rescanned.
                from("/music/c/2.flac", 8),
                // Moved in from outside.
                to("/music/d/3.flac", 9),
            ],
            now,
        );
        let batch = p.take(now);
        assert_eq!(
            batch.moves,
            [(
                PathBuf::from("/music/a/1.flac"),
                PathBuf::from("/music/b/1.flac")
            )]
        );
        assert_eq!(
            scopes(&batch),
            [
                folder("/music/a"),
                folder("/music/b"),
                folder("/music/c"),
                folder("/music/d")
            ]
        );
        // An unmatched half is not paired with a later one.
        feed(&mut p, &[to("/music/e/2.flac", 8)], now);
        assert!(p.take(now).moves.is_empty());
    }

    #[test]
    fn a_burst_waits_for_quiet_but_not_forever() {
        let start = Instant::now();
        let mut p = pending();
        assert_eq!(p.deadline(DEBOUNCE, MAX_DELAY), None);
        for i in 0..1000u32 {
            let now = start + Duration::from_millis(u64::from(i));
            let path = format!("/music/burst/{i}.flac");
            feed(
                &mut p,
                &[event(EventKind::Create(CreateKind::File), &path)],
                now,
            );
        }
        // Events every millisecond for a second: due one second after the first.
        assert_eq!(p.deadline(DEBOUNCE, MAX_DELAY), Some(start + MAX_DELAY));
        let batch = p.take(start + MAX_DELAY);
        assert_eq!(scopes(&batch), [folder("/music/burst")]);

        let now = start + Duration::from_secs(5);
        feed(
            &mut p,
            &[event(EventKind::Remove(RemoveKind::File), "/music/1.flac")],
            now,
        );
        assert_eq!(p.deadline(DEBOUNCE, MAX_DELAY), Some(now + DEBOUNCE));
    }

    #[test]
    fn overflow_rescans_the_whole_root_at_once() {
        let now = Instant::now();
        let mut p = pending();
        feed(
            &mut p,
            &[event(
                EventKind::Create(CreateKind::File),
                "/music/a/1.flac",
            )],
            now,
        );
        let overflow = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert_eq!(feed(&mut p, &[overflow], now), [Notice::Overflow]);
        assert_eq!(p.deadline(DEBOUNCE, MAX_DELAY), Some(now));
        let batch = p.take(now);
        assert_eq!(batch.reason, RescanReason::Overflow);
        assert_eq!(scopes(&batch), [Scope::Root]);
    }

    #[test]
    fn the_root_itself_going_away_is_noticed() {
        let now = Instant::now();
        let mut p = pending();
        let notices = feed(
            &mut p,
            &[
                event(EventKind::Modify(ModifyKind::Name(RenameMode::From)), ROOT),
                event(EventKind::Remove(RemoveKind::Folder), ROOT),
                event(EventKind::Create(CreateKind::File), "/elsewhere/1.flac"),
            ],
            now,
        );
        assert_eq!(
            notices,
            [Notice::RootChanged, Notice::RootChanged, Notice::None]
        );
        assert!(p.is_empty());
    }

    #[test]
    fn a_file_being_written_holds_back_its_folder() {
        let now = Instant::now();
        let mut p = pending();
        feed(
            &mut p,
            &[
                event(EventKind::Create(CreateKind::File), "/music/a/big.flac"),
                event(
                    EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                    "/music/a/big.flac",
                ),
                event(EventKind::Create(CreateKind::File), "/music/b/small.flac"),
            ],
            now,
        );
        let batch = p.take(now);
        assert_eq!(scopes(&batch), [folder("/music/b")]);
        assert!(!p.is_empty());
        assert_eq!(p.deadline(DEBOUNCE, MAX_DELAY), Some(now + DEBOUNCE));

        let later = now + Duration::from_secs(2);
        assert!(p.take(later).is_empty(), "still being written");
        feed(
            &mut p,
            &[event(
                EventKind::Access(AccessKind::Close(AccessMode::Write)),
                "/music/a/big.flac",
            )],
            later,
        );
        assert_eq!(scopes(&p.take(later)), [folder("/music/a")]);

        // A file that is never closed holds its folder back for a while only.
        feed(
            &mut p,
            &[event(
                EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                "/music/c/stuck.flac",
            )],
            later,
        );
        assert!(p.take(later).is_empty());
        assert_eq!(scopes(&p.take(later + STALE_WRITE)), [folder("/music/c")]);
    }

    #[test]
    fn forgetting_a_root_drops_its_changes() {
        let now = Instant::now();
        let mut p = pending();
        feed(
            &mut p,
            &[event(
                EventKind::Create(CreateKind::File),
                "/music/a/1.flac",
            )],
            now,
        );
        p.forget_root(0);
        assert!(p.is_empty());
        assert_eq!(p.deadline(DEBOUNCE, MAX_DELAY), None);
    }
}
