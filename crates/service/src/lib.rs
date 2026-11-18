// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The application core shared by every front end: the player, the library, the
//! settings and the event bus that announces what they do.
//!
//! See the crate's README for its place in the workspace.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bus;
pub mod clock;
mod event;
pub mod files;
pub mod formats;
pub mod jobs;
pub mod library;
pub mod player;
mod service;
pub mod settings;

pub use event::{Event, SubscribeOptions};
pub use service::{LibraryChoice, Service, ServiceConfig, ServiceError};

/// Exclusive access to a sound card that another program, such as the session
/// manager, holds. The desktop integration implements [`DeviceReservation`]
/// and the application hands it to the engine.
pub use helikodu_engine::output::{
    DeviceReservation, NoReservation, ReservationError, ReservationGuard, ReservedCard,
    SharedReservation,
};
