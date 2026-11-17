// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The watcher thread: inotify watches, timers, masking, and the rescans.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use notify::event::{AccessKind, AccessMode};
use notify::{Config, EventKind, INotifyWatcher, RecursiveMode, Watcher as _};

use super::changes::{Notice, Pending};
use super::mounts::{Mount, max_user_watches, mount_of, needs_polling, read_mounts};
use super::{Message, PollReason, RescanReason, WatchEvent, WatchMode, WatchOptions};
use crate::catalog::RootId;
use crate::scan::filter::classify;
use crate::scan::{self, CancelToken, ScanTarget, Scope, SymlinkPolicy};
use crate::{DbError, Library};

/// The longest wait between two looks at the timers.
const MAX_WAIT: Duration = Duration::from_secs(60);

/// A library folder as the watcher sees it.
#[derive(Debug)]
struct Root {
    path: PathBuf,
    id: Option<RootId>,
    /// Whether it is gone and masked.
    masked: bool,
    /// The mount point of its file system when it was last seen.
    mount: Option<PathBuf>,
    /// Its inotify watch, when it is watched.
    watch: Option<INotifyWatcher>,
    /// When it is next rescanned, when it is polled.
    next_poll: Option<Instant>,
}

/// Whether a library folder is there.
#[derive(Debug, PartialEq, Eq)]
enum Availability {
    /// It is, on the file system mounted at this point (when known).
    There { mount: Option<PathBuf> },
    /// It is not, for this reason.
    Gone(String),
}

/// The state of the watcher thread.
#[derive(Debug)]
pub(super) struct WatchThread {
    library: Arc<Library>,
    options: WatchOptions,
    events: Sender<WatchEvent>,
    messages: Sender<Message>,
    cancel: CancelToken,
    roots: Vec<Root>,
    pending: Pending,
}

impl WatchThread {
    pub(super) fn new(
        library: Arc<Library>,
        options: WatchOptions,
        events: Sender<WatchEvent>,
        messages: Sender<Message>,
        cancel: CancelToken,
    ) -> Self {
        let pending = Pending::new(options.scan.ignore_marker.clone());
        Self {
            library,
            options,
            events,
            messages,
            cancel,
            roots: Vec::new(),
            pending,
        }
    }

    fn emit(&self, event: WatchEvent) {
        let _ = self.events.send(event);
    }

    fn error(&self, root: Option<&Path>, message: impl Into<String>) {
        self.emit(WatchEvent::Error {
            root: root.map(Path::to_path_buf),
            message: message.into(),
        });
    }

    pub(super) fn run(mut self, receiver: &Receiver<Message>) {
        self.start();
        let purge_interval = (self.options.grace_period / 4)
            .clamp(Duration::from_secs(1), Duration::from_secs(60 * 60));
        let started = Instant::now();
        let mut next_check = started + self.options.check_interval;
        let mut next_purge = started + purge_interval;
        while !self.cancel.is_cancelled() {
            let now = Instant::now();
            if now >= next_check {
                self.check_roots();
                next_check = now + self.options.check_interval;
            }
            self.due_polls(now);
            if self.pending_deadline().is_some_and(|due| due <= now) {
                self.flush();
                continue;
            }
            if now >= next_purge {
                self.purge();
                next_purge = Instant::now() + purge_interval;
            }

            let mut deadline = next_check.min(next_purge).min(now + MAX_WAIT);
            if let Some(due) = self.pending_deadline() {
                deadline = deadline.min(due);
            }
            for root in &self.roots {
                if let Some(poll) = root.next_poll {
                    deadline = deadline.min(poll);
                }
            }
            match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(message) => self.handle(message),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn pending_deadline(&self) -> Option<Instant> {
        self.pending
            .deadline(self.options.debounce, self.options.max_delay)
    }

    /// Adds the library folders, masks those that are gone, watches the others and
    /// asks for the first scan.
    fn start(&mut self) {
        for path in &self.options.scan.roots {
            let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            let id = match self.library.add_root(path.clone()) {
                Ok(id) => Some(id),
                Err(e) => {
                    self.error(Some(&path), e.to_string());
                    None
                }
            };
            self.roots.push(Root {
                path,
                id,
                masked: false,
                mount: None,
                watch: None,
                next_poll: None,
            });
        }
        let mounts = self.mounts();
        for index in 0..self.roots.len() {
            match self.availability(index, mounts.as_deref()) {
                Availability::There { .. } => {
                    self.start_root(index, mounts.as_deref());
                    if self.options.initial_scan {
                        self.rescan(index, Scope::Root, RescanReason::Started);
                    }
                }
                Availability::Gone(reason) => self.mask(index, reason),
            }
        }
    }

    fn mounts(&self) -> Option<Vec<Mount>> {
        read_mounts(&self.options.mountinfo).ok()
    }

    fn rescan(&mut self, index: usize, scope: Scope, reason: RescanReason) {
        let root = self.roots[index].path.clone();
        self.pending
            .rescan(index, &root, scope, reason, Instant::now());
    }

    /// Watches the library folder `index`, which is there, with inotify or by
    /// polling, as its file system allows.
    fn start_root(&mut self, index: usize, mounts: Option<&[Mount]>) {
        let path = self.roots[index].path.clone();
        let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let mount = mounts.and_then(|m| mount_of(m, &canonical)).cloned();
        self.roots[index].mount = mount.as_ref().map(|m| m.point.clone());
        self.roots[index].watch = None;
        let mode = match mount {
            Some(mount) if needs_polling(&mount.fs_type) => self.poll(
                index,
                PollReason::FileSystem {
                    fs_type: mount.fs_type,
                },
            ),
            _ => match self.watch(index, &path) {
                Ok(watch) => {
                    let root = &mut self.roots[index];
                    root.watch = Some(watch);
                    root.next_poll = None;
                    WatchMode::Inotify
                }
                Err(e) => self.poll(index, poll_reason(&e)),
            },
        };
        self.emit(WatchEvent::Watching { root: path, mode });
    }

    fn watch(&self, index: usize, path: &Path) -> notify::Result<INotifyWatcher> {
        let messages = self.messages.clone();
        let follow = self.options.scan.symlinks == SymlinkPolicy::Follow;
        let mut watch = INotifyWatcher::new(
            move |event: notify::Result<notify::Event>| {
                // Reads, among them every file a scan opens, are dropped here, so a
                // large rescan does not fill the channel.
                if let Ok(e) = &event
                    && let EventKind::Access(access) = e.kind
                    && access != AccessKind::Close(AccessMode::Write)
                {
                    return;
                }
                let _ = messages.send(Message::Notify { root: index, event });
            },
            Config::default().with_follow_symlinks(follow),
        )?;
        watch.watch(path, RecursiveMode::Recursive)?;
        Ok(watch)
    }

    /// Rescans the library folder `index` periodically from now on.
    fn poll(&mut self, index: usize, reason: PollReason) -> WatchMode {
        let interval = self.options.poll_interval;
        let root = &mut self.roots[index];
        root.watch = None;
        root.next_poll = Some(Instant::now() + interval);
        WatchMode::Polling { reason, interval }
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Notify { root, event } => self.notified(root, event),
            Message::Rescan(path) => self.requested(path),
            Message::Stop => {}
        }
    }

    fn notified(&mut self, index: usize, event: notify::Result<notify::Event>) {
        let Some(root) = self.roots.get(index) else {
            return;
        };
        if root.masked || root.watch.is_none() {
            return; // A late event from a watch that has been dropped.
        }
        let path = root.path.clone();
        match event {
            Ok(event) => match self.pending.event(index, &path, &event, Instant::now()) {
                Notice::None => {}
                Notice::Overflow => self.emit(WatchEvent::Overflow { root: path }),
                Notice::RootChanged => {
                    let mounts = self.mounts();
                    self.check_root(index, mounts.as_deref());
                }
            },
            Err(e) if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) => {
                // Folders created from now on would not be watched.
                let mode = self.poll(index, poll_reason(&e));
                self.emit(WatchEvent::Watching { root: path, mode });
                self.rescan(index, Scope::Root, RescanReason::WatchLimit);
            }
            Err(e) => self.error(Some(&path), e.to_string()),
        }
    }

    fn requested(&mut self, path: Option<PathBuf>) {
        let Some(path) = path else {
            for index in 0..self.roots.len() {
                if !self.roots[index].masked {
                    self.rescan(index, Scope::Root, RescanReason::Requested);
                }
            }
            return;
        };
        let path = std::path::absolute(&path).unwrap_or(path);
        match self.root_of(&path) {
            Some(index) => self.rescan(index, Scope::Subtree(path), RescanReason::Requested),
            None => self.error(
                None,
                format!(
                    "{} is not in a library folder that is there",
                    path.display()
                ),
            ),
        }
    }

    /// The library folder, not masked, that holds `path`.
    fn root_of(&self, path: &Path) -> Option<usize> {
        self.roots
            .iter()
            .enumerate()
            .filter(|(_, root)| !root.masked && path.starts_with(&root.path))
            .max_by_key(|(_, root)| root.path.as_os_str().len())
            .map(|(index, _)| index)
    }

    fn due_polls(&mut self, now: Instant) {
        for index in 0..self.roots.len() {
            let root = &mut self.roots[index];
            if !root.masked && root.next_poll.is_some_and(|poll| poll <= now) {
                root.next_poll = Some(now + self.options.poll_interval);
                self.rescan(index, Scope::Root, RescanReason::Periodic);
            }
        }
    }

    // --- Masking -------------------------------------------------------------------

    fn check_roots(&mut self) {
        let mounts = self.mounts();
        for index in 0..self.roots.len() {
            self.check_root(index, mounts.as_deref());
        }
    }

    /// Masks the library folder `index` if it is gone, unmasks it if it is back, and
    /// watches it again if another file system was mounted there.
    fn check_root(&mut self, index: usize, mounts: Option<&[Mount]>) {
        let availability = self.availability(index, mounts);
        let (masked, recorded) = (self.roots[index].masked, self.roots[index].mount.clone());
        match availability {
            Availability::Gone(reason) if !masked => self.mask(index, reason),
            Availability::There { .. } if masked => {
                self.roots[index].masked = false;
                let path = self.roots[index].path.clone();
                self.emit(WatchEvent::RootUnmasked { root: path });
                self.start_root(index, mounts);
                self.rescan(index, Scope::Root, RescanReason::Remounted);
            }
            Availability::There { mount } if mounts.is_some() && mount != recorded => {
                self.start_root(index, mounts);
                self.rescan(index, Scope::Root, RescanReason::Remounted);
            }
            Availability::There { .. } | Availability::Gone(_) => {}
        }
    }

    /// Whether the library folder `index` is there.
    ///
    /// It is gone when it cannot be read, and when it is empty while its file system
    /// is no longer the one it was on (a mount point left behind by a device). When
    /// the mount table is unknown, an empty folder in which the library has files
    /// is taken as gone too.
    fn availability(&self, index: usize, mounts: Option<&[Mount]>) -> Availability {
        let root = &self.roots[index];
        let mut entries = match fs::metadata(&root.path) {
            Ok(metadata) if !metadata.is_dir() => {
                return Availability::Gone("it is not a folder".to_owned());
            }
            Ok(_) => match fs::read_dir(&root.path) {
                Ok(entries) => entries,
                Err(e) => return Availability::Gone(e.to_string()),
            },
            Err(e) => return Availability::Gone(e.to_string()),
        };
        let canonical = fs::canonicalize(&root.path).unwrap_or_else(|_| root.path.clone());
        let mount = mounts
            .and_then(|m| mount_of(m, &canonical))
            .map(|m| m.point.clone());
        if entries.next().is_some() {
            return Availability::There { mount };
        }
        match (&root.mount, mounts) {
            (Some(before), Some(_)) if mount.as_ref() != Some(before) => {
                Availability::Gone("its file system is no longer mounted".to_owned())
            }
            (Some(_), Some(_)) => Availability::There { mount },
            _ if self.has_present_files(root) => Availability::Gone(
                "it is empty, but the library has files in it: its device is probably gone"
                    .to_owned(),
            ),
            _ => Availability::There { mount },
        }
    }

    fn has_present_files(&self, root: &Root) -> bool {
        let Some(id) = root.id else {
            return false;
        };
        self.library
            .database()
            .read(|c| scan::store::has_present_files(c, id))
            .unwrap_or(false)
    }

    fn mask(&mut self, index: usize, reason: String) {
        let root = &mut self.roots[index];
        root.masked = true;
        root.watch = None;
        root.next_poll = None;
        let path = root.path.clone();
        self.pending.forget_root(index);
        self.emit(WatchEvent::RootMasked { root: path, reason });
    }

    // --- Rescans -------------------------------------------------------------------

    /// Applies the moves and runs the rescans that are due.
    fn flush(&mut self) {
        let batch = self.pending.take(Instant::now());
        if batch.is_empty() {
            return;
        }
        // A folder that went away since its events came must not be scanned: it
        // would look as if everything in it had been removed.
        let mounts = self.mounts();
        for &index in batch.scopes.keys() {
            self.check_root(index, mounts.as_deref());
        }
        for (from, to) in &batch.moves {
            self.apply_move(from, to);
        }
        let targets: Vec<ScanTarget> = batch
            .scopes
            .into_iter()
            .filter(|(index, _)| !self.roots[*index].masked)
            .map(|(_, (root, scopes))| ScanTarget { root, scopes })
            .collect();
        if targets.is_empty() {
            return;
        }
        let reason = batch.reason;
        self.emit(WatchEvent::RescanStarted {
            reason,
            roots: targets.iter().map(|t| t.root.clone()).collect(),
            full: targets.iter().all(|t| t.scopes.contains(&Scope::Root)),
        });
        let result = std::thread::scope(|s| {
            let (sender, receiver) = mpsc::channel();
            let events = self.events.clone();
            s.spawn(move || {
                for event in receiver {
                    let _ = events.send(WatchEvent::Scan(event));
                }
            });
            scan::run_targets(
                self.library.database(),
                &self.options.scan,
                &targets,
                Some(&sender),
                &self.cancel,
            )
        });
        match result {
            Ok(summary) => self.emit(WatchEvent::RescanFinished { reason, summary }),
            Err(e) => self.error(None, e.to_string()),
        }
    }

    /// Moves what the library has at `from` to `to`, keeping the identities of the
    /// tracks. The rescans that follow check both sides.
    fn apply_move(&self, from: &Path, to: &Path) {
        let Some(index) = self.root_of(to) else {
            return; // Moved out of the library: missing, by the rescan of `from`.
        };
        let Some(root) = self.roots[index].id else {
            return;
        };
        let moved = if to.is_dir() {
            self.library.move_folder(root, from, to)
        } else {
            self.move_file(root, from, to)
        };
        match moved {
            Ok(0) => {}
            Ok(files) => self.emit(WatchEvent::Moved {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
                files,
            }),
            Err(e) => self.error(Some(&self.roots[index].path), e.to_string()),
        }
    }

    /// Moves the library's file at `from` to `to`, replacing what the library had
    /// there. Returns 1 if it moved, 0 if there was nothing to move.
    fn move_file(&self, root: RootId, from: &Path, to: &Path) -> Result<usize, DbError> {
        if to.file_name().and_then(classify).is_none() {
            return Ok(0); // Renamed to a name the library does not take.
        }
        let known = |path: &Path| {
            let path = path.to_path_buf();
            self.library
                .database()
                .read(move |c| crate::catalog::write::file_at(c, &path))
                .map(|file| file.is_some())
        };
        if !known(from)? {
            return Ok(0);
        }
        if known(to)? {
            self.library.remove_file(to)?;
        }
        self.library.move_file(root, from, to).map(|()| 1)
    }

    /// Removes the files whose grace period has ended, in the library folders that
    /// are there.
    fn purge(&self) {
        let Some(cutoff) = SystemTime::now().checked_sub(self.options.grace_period) else {
            return;
        };
        for root in self.roots.iter().filter(|r| !r.masked) {
            let Some(id) = root.id else {
                continue;
            };
            match self.library.remove_missing_files(id, cutoff) {
                Ok(0) => {}
                Ok(files) => self.emit(WatchEvent::MissingRemoved {
                    root: root.path.clone(),
                    files,
                }),
                Err(e) => self.error(Some(&root.path), e.to_string()),
            }
        }
    }
}

/// Why a library folder whose watch failed with `error` is polled.
fn poll_reason(error: &notify::Error) -> PollReason {
    match error.kind {
        notify::ErrorKind::MaxFilesWatch => PollReason::WatchLimit {
            max_user_watches: max_user_watches(),
        },
        _ => PollReason::WatchFailed {
            message: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watch_limit_is_reported_with_the_limit() {
        let reason = poll_reason(&notify::Error::new(notify::ErrorKind::MaxFilesWatch));
        let PollReason::WatchLimit { max_user_watches } = reason else {
            panic!("{reason:?}");
        };
        // Linux has had the file since 2.6.13; the limit is at least 8192 since 5.11.
        assert!(
            max_user_watches.is_some_and(|n| n > 0),
            "{max_user_watches:?}"
        );

        let other = poll_reason(&notify::Error::generic("broken"));
        assert!(matches!(other, PollReason::WatchFailed { .. }), "{other:?}");
    }
}
