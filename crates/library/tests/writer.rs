// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The writer thread: batching, rollback of failed jobs and batches, and shutdown.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use helikodu_library::rusqlite::{self, Connection};
use helikodu_library::{Database, DatabaseConfig, DbError, WriteTicket, Writer};

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(test: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "library-writer-{test}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn db_path(&self) -> PathBuf {
        self.dir.join("library.db")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Inserts a `session` row named `key`.
fn insert(c: &Connection, key: &str) -> rusqlite::Result<usize> {
    c.execute("INSERT INTO session (key, value) VALUES (?1, x'00')", [key])
}

fn keys(path: &Path) -> Vec<String> {
    let c = Connection::open(path).unwrap();
    let mut statement = c.prepare("SELECT key FROM session ORDER BY key").unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<String>>>()
        .unwrap()
}

/// Submits a job that blocks the writer until the returned sender is used or dropped,
/// and waits until the writer is inside it. Jobs submitted meanwhile queue up and form
/// the next batches.
fn block_writer(writer: &Writer) -> (mpsc::Sender<()>, WriteTicket<usize>) {
    let (started_tx, started) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let ticket = writer
        .submit(move |c| {
            started_tx.send(()).unwrap();
            let _ = released.recv();
            insert(c, "blocker")
        })
        .unwrap();
    started.recv().unwrap();
    (release, ticket)
}

#[test]
fn queued_writes_are_committed_in_batches() {
    let scratch = Scratch::new("batches");
    let mut config = DatabaseConfig::file(scratch.db_path());
    config.max_batch = 8;
    let db = Database::open(&config).unwrap();
    let writer = db.writer().unwrap();
    // A connection outside the writer sees only committed rows.
    let observer = Arc::new(Mutex::new(Connection::open(scratch.db_path()).unwrap()));

    let (release, blocker) = block_writer(writer);
    let tickets: Vec<WriteTicket<i64>> = (0..20)
        .map(|n| {
            let observer = Arc::clone(&observer);
            writer
                .submit(move |c| {
                    let committed: i64 = observer.lock().unwrap().query_row(
                        "SELECT count(*) FROM session",
                        [],
                        |row| row.get(0),
                    )?;
                    insert(c, &format!("job {n:02}"))?;
                    Ok(committed)
                })
                .unwrap()
        })
        .collect();
    drop(release);
    blocker.wait().unwrap();
    let committed_before: Vec<i64> = tickets.into_iter().map(|t| t.wait().unwrap()).collect();
    // Jobs 0–7 form one batch, 8–15 the next, 16–19 the last. Every job of a batch sees
    // what the earlier batches committed and nothing of its own batch.
    let expected: Vec<i64> = (0..20).map(|n| 1 + 8 * (n / 8)).collect();
    assert_eq!(committed_before, expected);
    assert_eq!(keys(&scratch.db_path()).len(), 21);
}

#[test]
fn a_failed_job_is_rolled_back_alone() {
    let scratch = Scratch::new("failed-job");
    let db = Database::open(&DatabaseConfig::file(scratch.db_path())).unwrap();
    let writer = db.writer().unwrap();
    let (release, blocker) = block_writer(writer);
    let before = writer.submit(|c| insert(c, "before")).unwrap();
    let failing = writer
        .submit(|c| {
            insert(c, "failing")?;
            c.execute("INSERT INTO no_such_table VALUES (1)", [])
        })
        .unwrap();
    let after = writer.submit(|c| insert(c, "after")).unwrap();
    drop(release);
    blocker.wait().unwrap();
    assert_eq!(before.wait().unwrap(), 1);
    assert!(matches!(failing.wait(), Err(DbError::Sqlite(_))));
    assert_eq!(after.wait().unwrap(), 1);
    assert_eq!(keys(&scratch.db_path()), ["after", "before", "blocker"]);
}

#[test]
fn a_batch_that_cannot_commit_is_rolled_back_whole() {
    let scratch = Scratch::new("failed-commit");
    let db = Database::open(&DatabaseConfig::file(scratch.db_path())).unwrap();
    let writer = db.writer().unwrap();
    let (release, blocker) = block_writer(writer);
    let innocent = writer.submit(|c| insert(c, "innocent")).unwrap();
    // A deferred foreign key violation passes the statement but fails the commit.
    let culprit = writer
        .submit(|c| {
            c.execute_batch("PRAGMA defer_foreign_keys = ON")?;
            c.execute(
                "INSERT INTO dirs (root_id, path, path_text) VALUES (999, x'2f', '/')",
                [],
            )
        })
        .unwrap();
    drop(release);
    blocker.wait().unwrap();
    assert!(matches!(innocent.wait(), Err(DbError::BatchFailed(_))));
    assert!(matches!(culprit.wait(), Err(DbError::BatchFailed(_))));
    assert_eq!(keys(&scratch.db_path()), ["blocker"]);
    // The writer goes on.
    writer.write(|c| insert(c, "later")).unwrap();
    assert_eq!(keys(&scratch.db_path()), ["blocker", "later"]);
}

#[test]
fn a_panicking_job_stops_the_writer_and_commits_nothing_of_its_batch() {
    let scratch = Scratch::new("panic");
    let db = Database::open(&DatabaseConfig::file(scratch.db_path())).unwrap();
    let writer = db.writer().unwrap().clone();
    let (release, blocker) = block_writer(&writer);
    let innocent = writer.submit(|c| insert(c, "innocent")).unwrap();
    let panicking = writer
        .submit(|c| -> rusqlite::Result<usize> {
            insert(c, "panicking")?;
            panic!("a bug in a write job");
        })
        .unwrap();
    drop(release);
    blocker.wait().unwrap();
    assert!(matches!(innocent.wait(), Err(DbError::WriterStopped)));
    assert!(matches!(panicking.wait(), Err(DbError::WriterStopped)));
    assert!(matches!(
        writer.write(|c| insert(c, "later")),
        Err(DbError::WriterStopped)
    ));
    drop(db);
    assert_eq!(keys(&scratch.db_path()), ["blocker"]);
}

#[test]
fn closing_commits_the_queued_writes() {
    let scratch = Scratch::new("close");
    let db = Database::open(&DatabaseConfig::file(scratch.db_path())).unwrap();
    let writer = db.writer().unwrap().clone();
    for n in 0..100 {
        // Fire and forget: nobody waits for these tickets.
        drop(
            writer
                .submit(move |c| insert(c, &format!("{n:03}")))
                .unwrap(),
        );
    }
    drop(db);
    assert_eq!(keys(&scratch.db_path()).len(), 100);
    assert!(matches!(
        writer.write(|c| insert(c, "late")),
        Err(DbError::WriterStopped)
    ));
}

#[test]
fn writes_from_many_threads_all_arrive() {
    let db = Database::open(&DatabaseConfig::in_memory()).unwrap();
    let writer = db.writer().unwrap();
    std::thread::scope(|scope| {
        for thread in 0..8 {
            scope.spawn(move || {
                for n in 0..100 {
                    writer
                        .write(move |c| insert(c, &format!("{thread}-{n:03}")))
                        .unwrap();
                }
            });
        }
    });
    let count: i64 = db
        .read(|c| c.query_row("SELECT count(*) FROM session", [], |row| row.get(0)))
        .unwrap();
    assert_eq!(count, 800);
}
