// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pool of worker processes that read files for the scanner.
//!
//! Each slot is a thread that owns one `helikodu-scan-worker` process at a time and
//! gives it one file at a time. A worker that does not answer within the time
//! limit is killed; a worker that dies (a crash, or its memory limit) is noticed
//! when its output ends. Either way the file is reported as failed and the slot
//! starts a new worker for its next file. A file is given to a worker once: one that
//! crashes the reader is not tried again.

use std::ffi::OsString;
use std::io::{BufWriter, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helikodu_meta::worker::{
    self, FileReport, Message, PROTOCOL_VERSION, Request, read_frame, write_frame,
};

use super::progress::{CancelToken, FailureKind, FileFailure, ScanError};

/// How often a waiting slot looks at the stop flags.
const POLL: Duration = Duration::from_millis(50);
/// How long a new worker may take to greet.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a worker whose input was closed may take to exit.
const EXIT_TIMEOUT: Duration = Duration::from_secs(1);
/// After this many workers in a row fail to start, the scan stops.
const MAX_FAILED_STARTS: usize = 3;

/// How to start a worker.
#[derive(Debug, Clone)]
pub(crate) struct WorkerCommand {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
    pub(crate) env: Vec<(OsString, OsString)>,
    pub(crate) file_timeout: Duration,
}

/// A file for a worker. `index` is the caller's.
#[derive(Debug)]
pub(crate) struct Job {
    pub(crate) index: usize,
    pub(crate) request: Request,
}

/// What became of a [`Job`], or why the pool cannot go on.
#[derive(Debug)]
pub(crate) enum Outcome {
    Read {
        index: usize,
        result: Result<Box<FileReport>, FileFailure>,
    },
    Fatal(ScanError),
}

/// The running pool.
#[derive(Debug)]
pub(crate) struct Pool {
    jobs: Option<SyncSender<Job>>,
    results: Receiver<Outcome>,
    slots: Vec<JoinHandle<()>>,
    restarts: Arc<AtomicUsize>,
}

impl Pool {
    /// Starts `count` slots. Each starts its worker when it gets its first file.
    /// They stop when the job sender is dropped and the queue is empty, or at once
    /// when `cancel` or `stop` is set.
    pub(crate) fn start(
        command: WorkerCommand,
        count: usize,
        cancel: CancelToken,
        stop: CancelToken,
    ) -> Self {
        let (jobs, queue) = mpsc::sync_channel::<Job>(count * 4);
        let queue = Arc::new(Mutex::new(queue));
        // Bounded too: when the database writes fall behind the workers, the
        // reports would otherwise pile up here, a few kilobytes each, until the
        // scan ends (over a gigabyte for half a million files).
        let (results_out, results) = mpsc::sync_channel(count * 4);
        let restarts = Arc::new(AtomicUsize::new(0));
        let command = Arc::new(command);
        let slots = (0..count)
            .map(|n| {
                let mut slot = Slot {
                    command: Arc::clone(&command),
                    queue: Arc::clone(&queue),
                    results: results_out.clone(),
                    cancel: cancel.clone(),
                    stop: stop.clone(),
                    restarts: Arc::clone(&restarts),
                    worker: None,
                    failed_starts: 0,
                };
                thread::Builder::new()
                    .name(format!("helikodu-scan-slot-{n}"))
                    .spawn(move || slot.run())
            })
            .filter_map(Result::ok)
            .collect();
        Self {
            jobs: Some(jobs),
            results,
            slots,
            restarts,
        }
    }

    /// A sender for jobs. It blocks while every slot is busy and the queue is full.
    pub(crate) fn jobs(&self) -> Option<SyncSender<Job>> {
        self.jobs.clone()
    }

    /// Drops the pool's own job sender, so that the slots stop once the other
    /// senders are gone and the queue is empty.
    pub(crate) fn close(&mut self) {
        self.jobs = None;
    }

    /// The outcomes. The channel ends when every slot has stopped.
    pub(crate) fn results(&self) -> &Receiver<Outcome> {
        &self.results
    }

    /// Waits for the slots and returns how many workers were restarted.
    pub(crate) fn join(mut self) -> usize {
        self.close();
        for slot in self.slots.drain(..) {
            let _ = slot.join();
        }
        self.restarts.load(Ordering::Relaxed)
    }
}

/// One slot of the pool: a thread and its current worker.
struct Slot {
    command: Arc<WorkerCommand>,
    queue: Arc<Mutex<Receiver<Job>>>,
    results: SyncSender<Outcome>,
    cancel: CancelToken,
    stop: CancelToken,
    restarts: Arc<AtomicUsize>,
    worker: Option<Worker>,
    failed_starts: usize,
}

impl Slot {
    fn stopping(&self) -> bool {
        self.cancel.is_cancelled() || self.stop.is_cancelled()
    }

    fn run(&mut self) {
        while !self.stopping() {
            let next = match self.queue.lock() {
                Ok(queue) => queue.recv_timeout(POLL),
                Err(_) => break,
            };
            let job = match next {
                Ok(job) => job,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let Some(result) = self.read(&job.request) else {
                break;
            };
            let outcome = Outcome::Read {
                index: job.index,
                result,
            };
            if self.results.send(outcome).is_err() {
                break;
            }
        }
        if let Some(worker) = self.worker.take() {
            if self.stopping() {
                worker.kill();
            } else {
                worker.finish();
            }
        }
    }

    /// Reads one file. `None` when the pool is stopping, or cannot start workers.
    fn read(&mut self, request: &Request) -> Option<Result<Box<FileReport>, FileFailure>> {
        let mut tries = 0;
        loop {
            if self.worker.is_none() {
                match Worker::start(&self.command) {
                    Ok(worker) => {
                        self.failed_starts = 0;
                        self.worker = Some(worker);
                    }
                    Err(e @ ScanError::WorkerVersion { .. }) => {
                        let _ = self.results.send(Outcome::Fatal(e));
                        return None;
                    }
                    Err(e) => {
                        self.failed_starts += 1;
                        if self.failed_starts >= MAX_FAILED_STARTS {
                            let _ = self.results.send(Outcome::Fatal(e));
                            return None;
                        }
                        continue;
                    }
                }
            }
            let worker = self.worker.as_mut()?;
            if worker.send(request).is_err() {
                // The worker died before it took the file (after the previous one);
                // the file is not to blame, so it goes to a new worker, once.
                self.discard(false);
                tries += 1;
                if tries > 1 {
                    return Some(Err(FileFailure {
                        kind: FailureKind::WorkerCrashed,
                        message: "the scan worker could not take the file".to_owned(),
                    }));
                }
                continue;
            }
            return self.wait(request.id);
        }
    }

    /// Waits for the answer to request `id`.
    fn wait(&mut self, id: u64) -> Option<Result<Box<FileReport>, FileFailure>> {
        let timeout = self.command.file_timeout;
        let deadline = Instant::now() + timeout;
        loop {
            if self.stopping() {
                self.discard(false);
                return None;
            }
            let worker = self.worker.as_mut()?;
            let now = Instant::now();
            let wait = deadline.saturating_duration_since(now).min(POLL);
            let failure = match worker.messages.recv_timeout(wait) {
                Ok(Ok(Message::Response(response))) if response.id == id => {
                    return Some(response.result.map(Box::new).map_err(from_worker_failure));
                }
                Ok(Ok(other)) => FileFailure {
                    kind: FailureKind::WorkerCrashed,
                    message: format!("the scan worker answered out of turn: {other:?}"),
                },
                Ok(Err(e)) => FileFailure {
                    kind: FailureKind::WorkerCrashed,
                    message: format!("the scan worker sent a malformed answer: {e}"),
                },
                Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
                Err(RecvTimeoutError::Timeout) => FileFailure {
                    kind: FailureKind::TimedOut,
                    message: format!(
                        "reading the file took longer than {} s; the scan worker was stopped",
                        timeout.as_secs_f64()
                    ),
                },
                Err(RecvTimeoutError::Disconnected) => {
                    let status = self.worker.take().map(Worker::exit_status);
                    self.restarts.fetch_add(1, Ordering::Relaxed);
                    return Some(Err(FileFailure {
                        kind: FailureKind::WorkerCrashed,
                        message: describe_exit(status.flatten()),
                    }));
                }
            };
            self.discard(true);
            return Some(Err(failure));
        }
    }

    /// Kills the current worker; the next file starts a new one.
    fn discard(&mut self, count_restart: bool) {
        if let Some(worker) = self.worker.take() {
            worker.kill();
            if count_restart {
                self.restarts.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// A running worker process.
struct Worker {
    child: Child,
    input: Option<BufWriter<ChildStdin>>,
    messages: Receiver<Result<Message, String>>,
    reader: Option<JoinHandle<()>>,
}

impl Worker {
    fn start(command: &WorkerCommand) -> Result<Self, ScanError> {
        let start_error = |reason: String| ScanError::WorkerStart {
            program: command.program.clone(),
            reason,
        };
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .envs(command.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| start_error(e.to_string()))?;
        let (Some(input), Some(mut output)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(start_error("no pipes".to_owned()));
        };
        let (sender, messages) = mpsc::channel();
        let reader = thread::Builder::new()
            .name("helikodu-scan-reader".to_owned())
            .spawn(move || {
                while let Ok(Some(frame)) = read_frame(&mut output) {
                    let message = Message::decode(&frame).map_err(|e| e.to_string());
                    if sender.send(message).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| start_error(e.to_string()));
        let reader = match reader {
            Ok(reader) => reader,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        let worker = Self {
            child,
            input: Some(BufWriter::new(input)),
            messages,
            reader: Some(reader),
        };
        match worker.messages.recv_timeout(START_TIMEOUT) {
            Ok(Ok(Message::Hello(hello))) if hello.version == PROTOCOL_VERSION => Ok(worker),
            Ok(Ok(Message::Hello(hello))) => {
                worker.kill();
                Err(ScanError::WorkerVersion {
                    found: hello.version,
                    expected: PROTOCOL_VERSION,
                })
            }
            other => {
                let reason = match other {
                    Ok(Ok(message)) => format!("it said {message:?} instead of hello"),
                    Ok(Err(e)) => format!("it sent a malformed greeting: {e}"),
                    Err(RecvTimeoutError::Timeout) => "it did not greet in time".to_owned(),
                    Err(RecvTimeoutError::Disconnected) => "it exited at once".to_owned(),
                };
                worker.kill();
                Err(start_error(reason))
            }
        }
    }

    fn send(&mut self, request: &Request) -> std::io::Result<()> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        write_frame(input, &Message::Request(request.clone()).encode())?;
        input.flush()
    }

    /// Kills the process and waits for it.
    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.join_reader();
    }

    /// Closes the input, which tells the worker to exit; kills it if it does not.
    fn finish(mut self) {
        self.input = None;
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        self.join_reader();
    }

    /// Waits for a process whose output has ended and returns how it ended.
    fn exit_status(mut self) -> Option<ExitStatus> {
        self.input = None;
        let deadline = Instant::now() + EXIT_TIMEOUT;
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                _ => {
                    let _ = self.child.kill();
                    break self.child.wait().ok();
                }
            }
        };
        self.join_reader();
        status
    }

    fn join_reader(&mut self) {
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn from_worker_failure(failure: worker::Failure) -> FileFailure {
    FileFailure {
        kind: match failure.kind {
            worker::FailureKind::Io => FailureKind::Io,
            worker::FailureKind::Unreadable => FailureKind::Unreadable,
            worker::FailureKind::Internal => FailureKind::ReaderFailed,
        },
        message: failure.message,
    }
}

/// How a worker that stopped answering ended, for the user.
fn describe_exit(status: Option<ExitStatus>) -> String {
    let Some(status) = status else {
        return "the scan worker stopped while reading the file".to_owned();
    };
    if let Some(signal) = status.signal() {
        let name = match signal {
            4 => " (SIGILL)",
            6 => " (SIGABRT: a crash, or the memory limit)",
            7 => " (SIGBUS)",
            8 => " (SIGFPE)",
            9 => " (SIGKILL)",
            11 => " (SIGSEGV)",
            _ => "",
        };
        format!("the scan worker crashed while reading the file: signal {signal}{name}")
    } else {
        format!(
            "the scan worker exited while reading the file, with status {}",
            status.code().unwrap_or(-1)
        )
    }
}
