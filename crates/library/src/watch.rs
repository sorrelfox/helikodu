// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The watcher: keeps the library in step with its folders while the program runs.
//!
//! - **Watch.** Each library folder is watched recursively with inotify (the `notify`
//!   crate's inotify backend, on a thread of its own; no async runtime).
//! - **Debounce.** Events are collected until none has come for
//!   [`WatchOptions::debounce`] (500 ms), or for [`WatchOptions::max_delay`] (1 s) at
//!   most, so changes appear within two seconds while a burst of events still
//!   becomes one rescan.
//! - **Rescan.** The events become rescans of the folders or subtrees they touched,
//!   run by the scanner, so a watched change is read exactly as a scan reads it.
//! - **Moves.** A rename seen from both sides moves the file or folder in the
//!   library, so its tracks keep their identity without being read again. A rename
//!   seen from one side only is a removal and an addition, and the scanner matches
//!   them by size and quick hash.
//! - **Overflow.** When the kernel's event queue overflows, the library folder is
//!   rescanned whole.
//! - **Watch limit.** When the kernel's limit on inotify watches is reached, the
//!   library folder is rescanned periodically instead, and
//!   [`WatchEvent::Watching`] says so with the limit.
//! - **File systems inotify cannot see.** Network file systems (NFS, SMB/CIFS),
//!   FUSE mounts and the Flatpak document portal are rescanned periodically,
//!   recognised by their type in the mount table.
//! - **Masking.** A library folder that disappears (an unmounted or unplugged
//!   device) is masked: nothing in it is marked missing, and it is rescanned when
//!   it comes back.
//! - **Grace period.** Files that disappeared are marked missing by the scanner and
//!   removed, with their tracks, once they have been missing for
//!   [`WatchOptions::grace_period`].

mod changes;
mod mounts;
mod thread;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::Library;
use crate::scan::{CancelToken, ScanEvent, ScanOptions, ScanSummary};

/// How a library folder is watched, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchOptions {
    /// The library folders and the scan rules. The watcher scans with these options.
    pub scan: ScanOptions,
    /// Whether every library folder is scanned when the watcher starts, to catch up
    /// with the changes made while nothing watched. On by default.
    pub initial_scan: bool,
    /// How long the events must quiet down before they are rescanned. 500 ms by
    /// default.
    pub debounce: Duration,
    /// How long events wait at most, however busy the folder. 1 s by default.
    pub max_delay: Duration,
    /// How often a library folder that cannot be watched with inotify is rescanned.
    /// 15 minutes by default.
    pub poll_interval: Duration,
    /// How often the watcher checks that every library folder is still there. 2 s
    /// by default.
    pub check_interval: Duration,
    /// How long a file stays in the library after it disappeared, in case it comes
    /// back (a device that is reconnected, a file that is replaced). 30 days by
    /// default. Missing times are kept in whole seconds.
    pub grace_period: Duration,
    /// The mount table, which tells the file system of each library folder.
    /// `/proc/self/mountinfo` by default.
    pub mountinfo: PathBuf,
}

impl WatchOptions {
    /// The defaults for watching the library folders of `scan`.
    pub fn new(scan: ScanOptions) -> Self {
        Self {
            scan,
            initial_scan: true,
            debounce: Duration::from_millis(500),
            max_delay: Duration::from_secs(1),
            poll_interval: Duration::from_secs(15 * 60),
            check_interval: Duration::from_secs(2),
            grace_period: Duration::from_secs(30 * 24 * 60 * 60),
            mountinfo: PathBuf::from(mounts::MOUNTINFO),
        }
    }
}

/// How a library folder is watched.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchMode {
    /// With inotify: changes appear within seconds.
    Inotify,
    /// By rescanning it every `interval`.
    Polling {
        /// Why inotify is not used.
        reason: PollReason,
        /// How often it is rescanned.
        interval: Duration,
    },
}

/// Why a library folder is rescanned periodically instead of watched.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PollReason {
    /// It is on a file system whose changes inotify does not see: a network file
    /// system, a FUSE mount or the document portal.
    FileSystem {
        /// The type in the mount table, such as `nfs4`, `cifs` or `fuse.portal`.
        fs_type: String,
    },
    /// The kernel's limit on inotify watches per user was reached. It can be raised
    /// with the `fs.inotify.max_user_watches` sysctl.
    WatchLimit {
        /// The limit, from `/proc/sys/fs/inotify/max_user_watches`.
        max_user_watches: Option<u64>,
    },
    /// inotify failed otherwise.
    WatchFailed {
        /// Why.
        message: String,
    },
}

/// Why a rescan runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RescanReason {
    /// The scan of every library folder when the watcher starts.
    Started,
    /// Watched changes.
    Changes,
    /// The kernel's event queue overflowed, so changes may have been lost.
    Overflow,
    /// The watch limit was reached; what happened meanwhile is caught up.
    WatchLimit,
    /// The periodic rescan of a library folder that is not watched with inotify.
    Periodic,
    /// A masked library folder is back.
    Remounted,
    /// Asked for with [`Watcher::rescan`].
    Requested,
}

/// Something the watcher did or noticed, sent on its event channel.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchEvent {
    /// A library folder is watched, or is watched another way from now on.
    Watching {
        /// The folder.
        root: PathBuf,
        /// How.
        mode: WatchMode,
    },
    /// A library folder is gone (not mounted, unplugged, removed). Its tracks stay
    /// in the library and nothing in it is marked missing until it is back.
    RootMasked {
        /// The folder.
        root: PathBuf,
        /// Why it is taken as gone.
        reason: String,
    },
    /// A masked library folder is back; it is rescanned.
    RootUnmasked {
        /// The folder.
        root: PathBuf,
    },
    /// The kernel's event queue overflowed for a library folder; it is rescanned
    /// whole.
    Overflow {
        /// The folder.
        root: PathBuf,
    },
    /// A watched rename moved files in the library, keeping their tracks'
    /// identities.
    Moved {
        /// The old path of the file or folder.
        from: PathBuf,
        /// The new path.
        to: PathBuf,
        /// The library's files that moved.
        files: usize,
    },
    /// A rescan begins.
    RescanStarted {
        /// Why.
        reason: RescanReason,
        /// The library folders it looks at.
        roots: Vec<PathBuf>,
        /// Whether it looks at all of them, rather than at what changed.
        full: bool,
    },
    /// An event of the running rescan.
    Scan(ScanEvent),
    /// A rescan ended.
    RescanFinished {
        /// Why it ran.
        reason: RescanReason,
        /// What it did.
        summary: ScanSummary,
    },
    /// Files whose grace period ended were removed with their tracks.
    MissingRemoved {
        /// The library folder.
        root: PathBuf,
        /// The number of files.
        files: usize,
    },
    /// Something failed; the watcher goes on.
    Error {
        /// The library folder concerned, if one is.
        root: Option<PathBuf>,
        /// What failed.
        message: String,
    },
}

/// A message to the watcher thread.
#[derive(Debug)]
enum Message {
    /// An event from the inotify watch of the library folder with this index.
    Notify {
        root: usize,
        event: notify::Result<notify::Event>,
    },
    /// A rescan of a folder, or of everything.
    Rescan(Option<PathBuf>),
    Stop,
}

/// The watcher, running on a thread named `helikodu-watch`. Dropping it stops it.
#[derive(Debug)]
pub struct Watcher {
    events: Receiver<WatchEvent>,
    messages: Sender<Message>,
    cancel: CancelToken,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Starts watching the library folders of `options` for `library`.
    ///
    /// Each folder is added to the library if needed. Folders that are not there
    /// are masked from the start. With [`WatchOptions::initial_scan`], every folder
    /// that is there is scanned first; the watches are set up before, so nothing
    /// that changes meanwhile is lost.
    pub fn spawn(library: Arc<Library>, options: WatchOptions) -> std::io::Result<Self> {
        let (event_sender, events) = mpsc::channel();
        let (messages, receiver) = mpsc::channel();
        let cancel = CancelToken::new();
        let state = thread::WatchThread::new(
            library,
            options,
            event_sender,
            messages.clone(),
            cancel.clone(),
        );
        let thread = std::thread::Builder::new()
            .name("helikodu-watch".to_owned())
            .spawn(move || state.run(&receiver))?;
        Ok(Self {
            events,
            messages,
            cancel,
            thread: Some(thread),
        })
    }

    /// The events of the watcher. The channel ends when the watcher stops.
    pub fn events(&self) -> &Receiver<WatchEvent> {
        &self.events
    }

    /// Rescans `path`, a library folder or a folder in one, or every library folder
    /// with `None`, as soon as possible.
    pub fn rescan(&self, path: Option<PathBuf>) {
        let _ = self.messages.send(Message::Rescan(path));
    }

    /// Stops the watcher, cancelling a running rescan, and waits for its thread.
    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        self.cancel.cancel();
        let _ = self.messages.send(Message::Stop);
        // While unwinding, the thread is told to stop but not waited for.
        if let Some(thread) = self.thread.take()
            && !std::thread::panicking()
            && let Err(panic) = thread.join()
        {
            std::panic::resume_unwind(panic);
        }
    }

    /// Feeds an inotify event or error to the watcher, as if from the watch of the
    /// library folder number `root`.
    #[cfg(test)]
    fn inject(&self, root: usize, event: notify::Result<notify::Event>) {
        let _ = self.messages.send(Message::Notify { root, event });
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.shut_down();
    }
}

#[cfg(test)]
mod tests;
