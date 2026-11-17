// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The object served while a card is held: answers other programs that ask for
//! the card.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use zbus::interface;

/// `org.freedesktop.ReserveDevice1` at `/org/freedesktop/ReserveDevice1/Audio<N>`.
pub(super) struct Reservation {
    pub(super) priority: i32,
    pub(super) application_name: String,
    pub(super) device_name: String,
    pub(super) release_requested: Arc<AtomicBool>,
}

#[interface(name = "org.freedesktop.ReserveDevice1")]
impl Reservation {
    /// Another program asks for the card. It stays ours while it is held, so
    /// the answer is always no; the request is noted.
    fn request_release(&self, priority: i32) -> bool {
        let _ = priority;
        self.release_requested.store(true, Ordering::Release);
        false
    }

    #[zbus(property)]
    fn priority(&self) -> i32 {
        self.priority
    }

    #[zbus(property)]
    fn application_name(&self) -> String {
        self.application_name.clone()
    }

    #[zbus(property)]
    fn application_device_name(&self) -> String {
        self.device_name.clone()
    }
}
