// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The writer: one thread that owns the only writing connection and runs every write.
//!
//! Writes arrive as jobs on a bounded channel. The thread takes the first job and then
//! every job already waiting, up to a batch limit, and runs them in one transaction.
//! Each job runs inside its own savepoint: a job that fails is rolled back alone and
//! the others still commit. A job's result is delivered only after its batch has
//! committed, so a successful write is on disk when the caller sees it. If the commit
//! fails, every job of the batch is rolled back and told so.
//!
//! If a job panics, the thread ends without committing, SQLite rolls back the open
//! transaction, and later writes fail with [`DbError::WriterStopped`].

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};

use rusqlite::Connection;

use super::DbError;
use super::error::SqliteError;

/// A write job with its result channel, type-erased.
trait WriteJob: Send {
    /// Runs the job and returns whether it succeeded and how to deliver its result once
    /// the batch's fate is known.
    fn run(self: Box<Self>, connection: &Connection) -> Outcome;

    /// Reports that the job could not run.
    fn fail(self: Box<Self>, error: DbError);
}

type Job = Box<dyn WriteJob>;

/// Delivers a job's result. Called with `Ok(())` when the batch committed, or with the
/// error that made the batch fail.
type Deliver = Box<dyn FnOnce(Result<(), DbError>) + Send>;

struct Outcome {
    succeeded: bool,
    deliver: Deliver,
}

struct TypedJob<T, F> {
    job: F,
    reply: SyncSender<Result<T, DbError>>,
}

impl<T, F> WriteJob for TypedJob<T, F>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> rusqlite::Result<T> + Send,
{
    fn run(self: Box<Self>, connection: &Connection) -> Outcome {
        let Self { job, reply } = *self;
        let value = job(connection);
        let succeeded = value.is_ok();
        let deliver: Deliver = Box::new(move |batch| {
            let result = match (value, batch) {
                (Ok(value), Ok(())) => Ok(value),
                (Err(error), _) => Err(DbError::from(error)),
                (Ok(_), Err(error)) => Err(error),
            };
            // The caller may have dropped the ticket; that is fine.
            let _ = reply.send(result);
        });
        Outcome { succeeded, deliver }
    }

    fn fail(self: Box<Self>, error: DbError) {
        let _ = self.reply.send(Err(error));
    }
}

/// Work for the writer thread.
enum Command {
    /// A write, batched with others in one transaction.
    Write(Job),
    /// Work that needs the connection outside any transaction, such as a rebuild of
    /// indexes or a checkpoint. Runs between batches.
    Exclusive(Box<dyn FnOnce(&mut Connection) + Send>),
    /// Stop after the commands already queued.
    Shutdown,
}

/// A handle that sends writes to the writer thread. Cloning is cheap.
#[derive(Debug, Clone)]
pub struct Writer {
    commands: SyncSender<Command>,
}

/// The result of a submitted write, available once its batch has committed.
#[derive(Debug)]
#[must_use = "a write's result tells whether it was committed"]
pub struct WriteTicket<T> {
    result: Receiver<Result<T, DbError>>,
}

impl<T> WriteTicket<T> {
    /// Waits until the write's batch has committed or failed, and returns the job's
    /// value or the error.
    pub fn wait(self) -> Result<T, DbError> {
        self.result.recv().unwrap_or(Err(DbError::WriterStopped))
    }

    /// The result if it is there already; `None` while the write is pending.
    pub fn try_wait(&self) -> Option<Result<T, DbError>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(DbError::WriterStopped)),
        }
    }
}

impl Writer {
    /// Queues `job` and returns a ticket for its result without waiting.
    ///
    /// The job receives the writer's connection inside an open transaction. It must not
    /// begin, commit or roll back transactions itself. If it returns an error, its own
    /// changes are rolled back and the rest of the batch is unaffected.
    ///
    /// Blocks while the queue is full, which slows a fast producer down to the speed of
    /// the disk.
    pub fn submit<T, F>(&self, job: F) -> Result<WriteTicket<T>, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let (reply, result) = mpsc::sync_channel(1);
        let job: Job = Box::new(TypedJob { job, reply });
        self.send(Command::Write(job))?;
        Ok(WriteTicket { result })
    }

    /// Runs `job` like [`submit`](Self::submit) and waits for the result.
    pub fn write<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        self.submit(job)?.wait()
    }

    /// Runs `job` on the writer's connection outside any transaction, between batches,
    /// and waits for the result.
    pub(crate) fn exclusive<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        let (reply, result) = mpsc::sync_channel(1);
        self.send(Command::Exclusive(Box::new(move |connection| {
            let _ = reply.send(job(connection));
        })))?;
        result.recv().unwrap_or(Err(DbError::WriterStopped))
    }

    fn send(&self, command: Command) -> Result<(), DbError> {
        self.commands
            .send(command)
            .map_err(|_| DbError::WriterStopped)
    }
}

/// The running writer thread.
#[derive(Debug)]
pub(crate) struct WriterThread {
    writer: Writer,
    thread: Option<JoinHandle<()>>,
}

impl WriterThread {
    /// Starts the thread with `connection`.
    pub(crate) fn start(
        connection: Connection,
        queue_capacity: usize,
        max_batch: usize,
    ) -> Result<Self, DbError> {
        let (commands, inbox) = mpsc::sync_channel(queue_capacity.max(1));
        let thread = thread::Builder::new()
            .name("helikodu-db-writer".to_owned())
            .spawn(move || run(connection, &inbox, max_batch.max(1)))
            .map_err(DbError::Thread)?;
        Ok(Self {
            writer: Writer { commands },
            thread: Some(thread),
        })
    }

    pub(crate) fn writer(&self) -> &Writer {
        &self.writer
    }
}

impl Drop for WriterThread {
    fn drop(&mut self) {
        // Other handles may still exist; the explicit command stops the thread anyway.
        // The writes queued before it are still committed.
        let _ = self.writer.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            // A panic in the thread has been reported by the panic hook already.
            let _ = thread.join();
        }
    }
}

fn run(mut connection: Connection, inbox: &Receiver<Command>, max_batch: usize) {
    let mut batch: Vec<Job> = Vec::with_capacity(max_batch);
    'commands: while let Ok(first) = inbox.recv() {
        let mut next = Some(first);
        while let Some(command) = next.take() {
            match command {
                Command::Write(job) => {
                    batch.push(job);
                    if batch.len() < max_batch {
                        match inbox.try_recv() {
                            Ok(command) => next = Some(command),
                            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
                        }
                    }
                }
                Command::Exclusive(job) => {
                    commit_batch(&connection, &mut batch);
                    job(&mut connection);
                }
                Command::Shutdown => {
                    commit_batch(&connection, &mut batch);
                    break 'commands;
                }
            }
        }
        commit_batch(&connection, &mut batch);
    }
    // Statistics for the query planner, cheap when nothing changed.
    let _ = connection.execute_batch("PRAGMA optimize");
}

/// Runs the jobs of `batch` in one transaction and delivers their results.
fn commit_batch(connection: &Connection, batch: &mut Vec<Job>) {
    if batch.is_empty() {
        return;
    }
    let jobs = std::mem::take(batch);
    if let Err(error) = connection.execute_batch("BEGIN IMMEDIATE") {
        // No job has run; each fails with the reason the batch could not start.
        let error = Arc::new(SqliteError(error));
        for job in jobs {
            job.fail(DbError::BatchFailed(Arc::clone(&error)));
        }
        return;
    }
    let deliveries: Vec<Outcome> = jobs
        .into_iter()
        .map(|job| run_job(connection, job))
        .collect();
    let committed = if connection.is_autocommit() {
        // A job ended the transaction itself, so the batch is no longer one unit.
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
            Some("a write job ended the batch's transaction".to_owned()),
        ))
    } else {
        connection.execute_batch("COMMIT")
    };
    let batch_result = committed.map_err(|error| {
        if !connection.is_autocommit() {
            let _ = connection.execute_batch("ROLLBACK");
        }
        Arc::new(SqliteError(error))
    });
    for Outcome { succeeded, deliver } in deliveries {
        let result = match &batch_result {
            Ok(()) => Ok(()),
            // A failed job reports its own error, not the batch's.
            Err(_) if !succeeded => Ok(()),
            Err(error) => Err(DbError::BatchFailed(Arc::clone(error))),
        };
        deliver(result);
    }
}

/// Runs one job inside its own savepoint, so that a failing job is rolled back alone.
fn run_job(connection: &Connection, job: Job) -> Outcome {
    if let Err(error) = connection.execute_batch("SAVEPOINT job") {
        let error = Arc::new(SqliteError(error));
        job.fail(DbError::BatchFailed(Arc::clone(&error)));
        return Outcome {
            succeeded: false,
            deliver: Box::new(|_| {}),
        };
    }
    let Outcome { succeeded, deliver } = job.run(connection);
    let ended = if succeeded {
        connection.execute_batch("RELEASE job")
    } else {
        connection.execute_batch("ROLLBACK TO job; RELEASE job")
    };
    match ended {
        Ok(()) => Outcome { succeeded, deliver },
        Err(error) => {
            let error = Arc::new(SqliteError(error));
            Outcome {
                succeeded: false,
                deliver: Box::new(move |_| deliver(Err(DbError::BatchFailed(error)))),
            }
        }
    }
}
