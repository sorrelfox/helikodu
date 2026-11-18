// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Converting and copying files in the background.
//!
//! A [`ConvertJob`] works through its items with a few threads at a lowered
//! priority. Each file is written under a temporary name in its target folder
//! (`.name.part`), its tags and cover are copied into it, and it is renamed into
//! place only when complete: a cancelled or failed item leaves nothing behind.
//! Progress arrives as [`ConvertEvent`]s on the job's threads.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use helikodu_engine::convert::{TranscodeError, TranscodeSettings, transcode};
use helikodu_engine::decode::DecoderRegistry;
use helikodu_library::{CancelToken, TrackTags};
use helikodu_model::library::{MediaSource, TrackId};

use super::tags;
use crate::library::MusicLibrary;

/// The nice value of the converting threads.
const NICE: i32 = 10;

/// What to do with one file.
#[derive(Debug, Clone, PartialEq)]
pub enum JobAction {
    /// Convert it.
    Convert(TranscodeSettings),
    /// Copy it byte for byte.
    Copy,
}

/// One file of a job.
#[derive(Debug, Clone, PartialEq)]
pub struct JobItem {
    /// The library track, for its cover.
    pub track: Option<TrackId>,
    /// What to read.
    pub source: MediaSource,
    /// Whether the tags (and the cover, with [`JobOptions::copy_cover`]) are
    /// copied into a converted file.
    pub copy_tags: bool,
    /// The library's tags of the track, for a section of a file, whose file tags
    /// describe the whole file.
    pub tags: Option<TrackTags>,
    /// Where the file goes.
    pub target: PathBuf,
    /// What to do.
    pub action: JobAction,
}

/// How a job works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobOptions {
    /// The files worked on at a time; at least 1.
    pub workers: usize,
    /// Whether a file already at a target is replaced; otherwise the item is
    /// skipped.
    pub replace: bool,
    /// Whether the front cover is copied with the tags.
    pub copy_cover: bool,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            workers: 4,
            replace: false,
            copy_cover: true,
        }
    }
}

/// How one item ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemOutcome {
    /// The file was written; its size in bytes.
    Written(u64),
    /// A file was there already and was left alone.
    Skipped,
    /// It failed; the text says why.
    Failed(String),
}

/// What a job announces: `Started`, then `Progress` and `Done` for each item, from
/// several items at once, then `Finished`.
#[derive(Debug, Clone, PartialEq)]
pub enum ConvertEvent {
    /// The job started with this many items.
    Started {
        /// The items.
        items: usize,
    },
    /// An item is under way.
    Progress {
        /// Its index in the job.
        index: usize,
        /// The share of it done, from 0 to 1.
        fraction: f32,
    },
    /// An item ended.
    Done {
        /// Its index.
        index: usize,
        /// How.
        outcome: ItemOutcome,
    },
    /// The job ended.
    Finished(ConvertSummary),
}

/// How a job ended.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConvertSummary {
    /// The items.
    pub total: usize,
    /// Those written.
    pub written: usize,
    /// Those skipped.
    pub skipped: usize,
    /// Those that failed.
    pub failed: usize,
    /// The bytes written.
    pub bytes: u64,
    /// Whether it was stopped before the end.
    pub cancelled: bool,
    /// How long it took, in seconds.
    pub seconds: f64,
}

/// A conversion running in the background.
#[derive(Debug)]
pub struct ConvertJob {
    cancel: CancelToken,
    thread: JoinHandle<ConvertSummary>,
}

impl ConvertJob {
    /// Starts working through `items`. `events` is called on the job's threads,
    /// one call at a time. `library`, when given, supplies the covers.
    pub fn start(
        library: Option<MusicLibrary>,
        items: Vec<JobItem>,
        options: JobOptions,
        events: impl FnMut(ConvertEvent) + Send + 'static,
    ) -> io::Result<Self> {
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let thread = thread::Builder::new()
            .name("helikodu-convert".into())
            .spawn(move || run(library.as_ref(), &items, options, &token, events))?;
        Ok(Self { cancel, thread })
    }

    /// Asks the job to stop; items under way are abandoned and removed.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether the job has ended.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Waits for the job to end.
    pub fn join(self) -> ConvertSummary {
        self.thread.join().unwrap_or_default()
    }
}

/// Works through `items` on the calling thread with `options.workers` helpers:
/// what [`ConvertJob`] runs.
pub fn run(
    library: Option<&MusicLibrary>,
    items: &[JobItem],
    options: JobOptions,
    cancel: &CancelToken,
    events: impl FnMut(ConvertEvent) + Send,
) -> ConvertSummary {
    let started = Instant::now();
    let events = Mutex::new(events);
    let emit = |event: ConvertEvent| {
        if let Ok(mut events) = events.lock() {
            events(event);
        }
    };
    emit(ConvertEvent::Started { items: items.len() });
    let next = AtomicUsize::new(0);
    let (sender, outcomes) = mpsc::channel::<(usize, ItemOutcome)>();
    let registry = Arc::new(DecoderRegistry::with_default_backends());
    let workers = options.workers.max(1).min(items.len().max(1));
    let mut summary = ConvertSummary {
        total: items.len(),
        ..ConvertSummary::default()
    };
    thread::scope(|scope| {
        for index in 0..workers {
            let sender = sender.clone();
            let (registry, next, emit) = (&registry, &next, &emit);
            let spawned = thread::Builder::new()
                .name(format!("helikodu-convert-{index}"))
                .spawn_scoped(scope, move || {
                    lower_priority();
                    loop {
                        if cancel.is_cancelled() {
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        let outcome = work(library, registry, item, options, cancel, &|f| {
                            emit(ConvertEvent::Progress {
                                index: i,
                                fraction: f,
                            });
                        });
                        let Some(outcome) = outcome else { break };
                        emit(ConvertEvent::Done {
                            index: i,
                            outcome: outcome.clone(),
                        });
                        if sender.send((i, outcome)).is_err() {
                            break;
                        }
                    }
                });
            if spawned.is_err() && index == 0 {
                // Without any thread, nothing is done; the summary says so.
                break;
            }
        }
        drop(sender);
        for (_, outcome) in outcomes {
            match outcome {
                ItemOutcome::Written(bytes) => {
                    summary.written += 1;
                    summary.bytes += bytes;
                }
                ItemOutcome::Skipped => summary.skipped += 1,
                ItemOutcome::Failed(_) => summary.failed += 1,
            }
        }
    });
    summary.cancelled = cancel.is_cancelled();
    summary.seconds = started.elapsed().as_secs_f64();
    emit(ConvertEvent::Finished(summary.clone()));
    summary
}

/// Does one item; `None` when it was cancelled.
fn work(
    library: Option<&MusicLibrary>,
    registry: &DecoderRegistry,
    item: &JobItem,
    options: JobOptions,
    cancel: &CancelToken,
    progress: &dyn Fn(f32),
) -> Option<ItemOutcome> {
    if item.target.exists() && !options.replace {
        return Some(ItemOutcome::Skipped);
    }
    let partial = partial_path(&item.target);
    let result = write_item(library, registry, item, options, cancel, &partial, progress);
    match result {
        Ok(bytes) => Some(ItemOutcome::Written(bytes)),
        Err(error) => {
            let _ = fs::remove_file(&partial);
            match error {
                ItemError::Cancelled => None,
                ItemError::Failed(text) => Some(ItemOutcome::Failed(text)),
            }
        }
    }
}

enum ItemError {
    Cancelled,
    Failed(String),
}

impl From<io::Error> for ItemError {
    fn from(e: io::Error) -> Self {
        Self::Failed(e.to_string())
    }
}

fn write_item(
    library: Option<&MusicLibrary>,
    registry: &DecoderRegistry,
    item: &JobItem,
    options: JobOptions,
    cancel: &CancelToken,
    partial: &Path,
    progress: &dyn Fn(f32),
) -> Result<u64, ItemError> {
    if let Some(folder) = item.target.parent() {
        fs::create_dir_all(folder)?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(partial)?;
    match &item.action {
        JobAction::Copy => {
            drop(file);
            copy_file(&item.source.path, partial, cancel, progress)?;
        }
        JobAction::Convert(settings) => {
            let mut last = 0.0_f32;
            transcode(
                registry,
                &item.source,
                file,
                settings,
                &mut |done, total| {
                    if let Some(total) = total.filter(|t| *t > 0) {
                        let f = (done as f64 / total as f64).clamp(0.0, 1.0) as f32;
                        // About a hundred updates per file at most.
                        if f - last >= 0.01 {
                            last = f;
                            progress(f);
                        }
                    }
                },
                &|| cancel.is_cancelled(),
            )
            .map_err(|e| match e {
                TranscodeError::Cancelled => ItemError::Cancelled,
                other => ItemError::Failed(other.to_string()),
            })?;
            if item.copy_tags {
                let cover = if options.copy_cover {
                    library
                        .zip(item.track)
                        .and_then(|(l, t)| tags::track_cover(l, t))
                } else {
                    None
                };
                tags::write_into(
                    partial,
                    &item.source.path,
                    item.source.section.is_none(),
                    item.tags.as_ref(),
                    cover,
                )
                .map_err(ItemError::Failed)?;
            }
        }
    }
    if cancel.is_cancelled() {
        return Err(ItemError::Cancelled);
    }
    fs::rename(partial, &item.target)?;
    progress(1.0);
    Ok(fs::metadata(&item.target)?.len())
}

/// Copies `from` to `to`, a block at a time so that cancelling is quick.
fn copy_file(
    from: &Path,
    to: &Path,
    cancel: &CancelToken,
    progress: &dyn Fn(f32),
) -> Result<(), ItemError> {
    use std::io::{Read, Write};
    let mut input = File::open(from)?;
    let total = input.metadata()?.len().max(1);
    let mut output = File::create(to)?;
    let mut buffer = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(ItemError::Cancelled);
        }
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        output.write_all(&buffer[..n])?;
        done += n as u64;
        progress((done as f64 / total as f64) as f32);
    }
    output.sync_all()?;
    Ok(())
}

/// The temporary name of a file being written: `.name.part` beside it.
pub fn partial_path(target: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(".part");
    target.with_file_name(name)
}

fn lower_priority() {
    let thread = rustix::thread::gettid();
    let _ = rustix::process::setpriority_process(Some(thread), NICE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_names_are_hidden_beside_the_target() {
        assert_eq!(
            partial_path(Path::new("/m/a/01 x.opus")),
            PathBuf::from("/m/a/.01 x.opus.part")
        );
    }
}
