// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A small pool of read-only connections for views, search and smart playlists.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use rusqlite::Connection;

use super::DbError;
use super::collation::LocaleCollation;
use super::connection::{self, Location, Role};

/// A fixed number of read-only connections, lent out one at a time.
#[derive(Debug)]
pub(crate) struct ReadPool {
    state: Mutex<PoolState>,
    returned: Condvar,
}

#[derive(Debug)]
struct PoolState {
    idle: Vec<PooledConnection>,
    /// The collation every connection should have, and a number that changes with it.
    collation: LocaleCollation,
    generation: u64,
}

#[derive(Debug)]
struct PooledConnection {
    connection: Connection,
    /// The collation generation registered on this connection.
    generation: u64,
}

impl ReadPool {
    /// Opens `size` read-only connections (at least one).
    pub(crate) fn open(
        location: &Location,
        size: usize,
        collation: &LocaleCollation,
    ) -> Result<Self, DbError> {
        let idle = (0..size.max(1))
            .map(|_| {
                Ok(PooledConnection {
                    connection: connection::open(location, Role::Reader, collation)?,
                    generation: 0,
                })
            })
            .collect::<Result<Vec<_>, DbError>>()?;
        Ok(Self {
            state: Mutex::new(PoolState {
                idle,
                collation: collation.clone(),
                generation: 0,
            }),
            returned: Condvar::new(),
        })
    }

    /// Runs `read` on a pooled connection, inside a read transaction so that all its
    /// queries see the same snapshot. Waits while every connection is in use.
    pub(crate) fn read<T>(
        &self,
        read: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, DbError> {
        let mut lease = self.lease()?;
        let transaction = lease.connection_mut().transaction()?;
        let result = read(&transaction);
        // Nothing was written; ending the transaction releases the snapshot.
        transaction.rollback()?;
        Ok(result?)
    }

    /// From now on, connections sort with `collation`. Each connection takes it up the
    /// next time it is lent out.
    pub(crate) fn set_collation(&self, collation: &LocaleCollation) {
        let mut state = self.lock();
        state.collation = collation.clone();
        state.generation += 1;
    }

    fn lease(&self) -> Result<Lease<'_>, DbError> {
        let mut state = self.lock();
        let mut pooled = loop {
            match state.idle.pop() {
                Some(pooled) => break pooled,
                None => {
                    state = self
                        .returned
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        };
        if pooled.generation != state.generation {
            let collation = state.collation.clone();
            let generation = state.generation;
            drop(state);
            let registered = collation.register(&pooled.connection);
            if let Err(error) = registered {
                self.lock().idle.push(pooled);
                self.returned.notify_one();
                return Err(error.into());
            }
            pooled.generation = generation;
        }
        Ok(Lease {
            pool: self,
            pooled: Some(pooled),
        })
    }

    fn lock(&self) -> MutexGuard<'_, PoolState> {
        // The state is a list and a counter; a panic elsewhere cannot leave it torn.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A connection lent out of the pool; it goes back when the lease is dropped, also when
/// the reader panics.
struct Lease<'a> {
    pool: &'a ReadPool,
    pooled: Option<PooledConnection>,
}

impl Lease<'_> {
    fn connection_mut(&mut self) -> &mut Connection {
        match &mut self.pooled {
            Some(pooled) => &mut pooled.connection,
            None => unreachable!("a lease holds its connection until it is dropped"),
        }
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(pooled) = self.pooled.take() {
            self.pool.lock().idle.push(pooled);
            self.pool.returned.notify_one();
        }
    }
}
