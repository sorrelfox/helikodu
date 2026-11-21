// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Watching `settings.toml` for edits made outside the program.
//!
//! The folder of the file is watched with inotify (the file itself is replaced, not
//! written in place, by the program, the command line and most editors). Once the
//! events about the file have stopped for a moment, the store reads it again
//! ([`SettingsStore::reload`]): the store's own writes are recognised there and
//! cost one read.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use notify::{Config, EventKind, INotifyWatcher, RecursiveMode, Watcher as _};

use super::SettingsStore;

#[derive(Debug)]
enum Message {
    Changed,
    Stop,
}

/// Watches the settings file of a [`SettingsStore`] and reloads it when it changes
/// on disk, on a thread named `helikodu-settings`. Dropping it stops it.
#[derive(Debug)]
pub struct SettingsWatcher {
    messages: Sender<Message>,
    // Dropped before the thread is joined: it holds a sender of the channel.
    watch: Option<INotifyWatcher>,
    thread: Option<JoinHandle<()>>,
}

impl SettingsWatcher {
    /// How long the file must stay unchanged before it is read: an editor may
    /// write it in several steps.
    pub const DEBOUNCE: Duration = Duration::from_millis(300);

    /// Starts watching the file of `store`, reading it `debounce` after the last
    /// change. Its folder is created if needed.
    pub fn start(store: Arc<SettingsStore>, debounce: Duration) -> io::Result<Self> {
        let path = store.path().to_path_buf();
        let folder = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        std::fs::create_dir_all(&folder)?;
        let name: OsString = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
        let (messages, receiver) = mpsc::channel();
        let sender = messages.clone();
        let mut watch = INotifyWatcher::new(
            move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    // An overflow or another error: read the file to be sure.
                    let _ = sender.send(Message::Changed);
                    return;
                };
                let about_file = event
                    .paths
                    .iter()
                    .any(|p| p.file_name() == Some(name.as_os_str()));
                let changes = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
                );
                if about_file && changes {
                    let _ = sender.send(Message::Changed);
                }
            },
            Config::default(),
        )
        .map_err(io::Error::other)?;
        watch
            .watch(&folder, RecursiveMode::NonRecursive)
            .map_err(io::Error::other)?;
        let thread = thread::Builder::new()
            .name("helikodu-settings".into())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    if matches!(message, Message::Stop) {
                        return;
                    }
                    // Wait until the changes stop.
                    loop {
                        match receiver.recv_timeout(debounce) {
                            Ok(Message::Changed) => {}
                            Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                            Err(RecvTimeoutError::Timeout) => break,
                        }
                    }
                    store.reload();
                }
            })?;
        Ok(Self {
            messages,
            watch: Some(watch),
            thread: Some(thread),
        })
    }
}

impl Drop for SettingsWatcher {
    fn drop(&mut self) {
        self.watch.take();
        let _ = self.messages.send(Message::Stop);
        if let Some(thread) = self.thread.take()
            && !thread::panicking()
        {
            let _ = thread.join();
        }
    }
}
