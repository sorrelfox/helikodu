// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Errors of the player service.

use std::io;

use helikodu_model::player::EngineError;
use thiserror::Error;

/// Something the player service could not do.
#[derive(Debug, Error)]
pub enum PlayerError {
    /// The configuration is not valid.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    /// A thread could not be started.
    #[error("cannot start the player: {0}")]
    Spawn(#[source] io::Error),
    /// The service thread has ended.
    #[error("the player has shut down")]
    Gone,
    /// An output backend could not be used.
    #[error(transparent)]
    Output(EngineError),
}

impl PartialEq for PlayerError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::InvalidConfig(a), Self::InvalidConfig(b)) => a == b,
            (Self::Spawn(a), Self::Spawn(b)) => a.kind() == b.kind(),
            (Self::Gone, Self::Gone) => true,
            (Self::Output(a), Self::Output(b)) => a == b,
            _ => false,
        }
    }
}
