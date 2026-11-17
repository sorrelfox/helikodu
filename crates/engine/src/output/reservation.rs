// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Device reservation: taking a sound card from the program that holds it.
//!
//! On a desktop, the sound server (PipeWire through WirePlumber, or PulseAudio)
//! holds every card, so a `hw:` device fails to open with "busy". The desktop's
//! protocol for handing a card over is `org.freedesktop.ReserveDevice1` on the
//! session bus. The engine does not speak D-Bus itself; the program that runs it
//! injects a [`DeviceReservation`] through
//! [`EngineConfig::device_reservation`](crate::transport::EngineConfig::device_reservation)
//! (or [`connect_backend_with`](super::connect_backend_with)). The default,
//! [`NoReservation`], takes nothing, and the backend then behaves as if
//! reservation did not exist.
//!
//! The ALSA backend asks for the card before it opens a `hw:` or `plughw:` device
//! in exclusive mode, and in shared mode when the device turns out to be busy. It
//! keeps the [`ReservationGuard`] for as long as the stream is open and drops it
//! after the device is closed, which gives the card back.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

/// A way of taking a sound card from the program that holds it.
pub trait DeviceReservation: Send + Sync + fmt::Debug {
    /// Takes `card` for this program.
    ///
    /// On success the card is ours until the returned guard is dropped; the
    /// device may still need a few milliseconds before it opens, while its
    /// previous holder closes it.
    fn acquire(&self, card: &ReservedCard) -> Result<ReservationGuard, ReservationError>;
}

/// The card to take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedCard {
    /// The ALSA card index: the `N` of `org.freedesktop.ReserveDevice1.Audio<N>`.
    pub index: u32,
    /// The card's ID string, for example `Generic_1`.
    pub id: String,
    /// The PCM that will be opened, for example `hw:CARD=Generic_1,DEV=1`.
    pub device: String,
}

/// Why a card could not be taken.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReservationError {
    /// This reservation does not take cards (the [`NoReservation`] default).
    #[error("device reservation is not available")]
    Unsupported,
    /// The program that holds the card did not give it up.
    #[error("the card is reserved{}", holder.as_ref().map(|h| format!(" by {h}")).unwrap_or_default())]
    Refused {
        /// The program that holds it, if known, for example `JACK`.
        holder: Option<String>,
    },
    /// Reservation failed for another reason, for example no session bus.
    #[error("device reservation failed: {0}")]
    Failed(String),
}

/// A taken card. Dropping the guard gives the card back.
pub struct ReservationGuard {
    holder: Box<dyn fmt::Debug + Send>,
    release_requested: Arc<AtomicBool>,
}

impl ReservationGuard {
    /// A guard that keeps `holder` alive and drops it to give the card back.
    ///
    /// `release_requested` is set by the reservation when another program asks
    /// for the card while it is ours; the reservation refuses such requests while
    /// the guard exists.
    pub fn new(
        holder: impl fmt::Debug + Send + 'static,
        release_requested: Arc<AtomicBool>,
    ) -> Self {
        Self {
            holder: Box::new(holder),
            release_requested,
        }
    }

    /// Whether another program has asked for the card since it was taken.
    ///
    /// WirePlumber asks for every card it lost right away, so this is usually
    /// true while PipeWire runs; it says only that someone would like the card.
    pub fn release_requested(&self) -> bool {
        self.release_requested.load(Ordering::Acquire)
    }
}

impl fmt::Debug for ReservationGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReservationGuard")
            .field("holder", &self.holder)
            .field("release_requested", &self.release_requested())
            .finish()
    }
}

/// The default reservation: takes nothing and always answers
/// [`ReservationError::Unsupported`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoReservation;

impl DeviceReservation for NoReservation {
    fn acquire(&self, _card: &ReservedCard) -> Result<ReservationGuard, ReservationError> {
        Err(ReservationError::Unsupported)
    }
}

/// A shared [`DeviceReservation`], cheap to clone. Two handles are equal when
/// they share the same reservation. The default is [`NoReservation`].
#[derive(Clone)]
pub struct SharedReservation(Arc<dyn DeviceReservation>);

impl SharedReservation {
    /// Shares `reservation`.
    pub fn new(reservation: impl DeviceReservation + 'static) -> Self {
        Self(Arc::new(reservation))
    }

    /// Shares a reservation that is already in an `Arc`.
    pub fn from_arc(reservation: Arc<dyn DeviceReservation>) -> Self {
        Self(reservation)
    }

    /// Takes `card`; see [`DeviceReservation::acquire`].
    pub fn acquire(&self, card: &ReservedCard) -> Result<ReservationGuard, ReservationError> {
        self.0.acquire(card)
    }
}

impl Default for SharedReservation {
    fn default() -> Self {
        Self::new(NoReservation)
    }
}

impl PartialEq for SharedReservation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for SharedReservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SharedReservation").field(&self.0).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Holder(Arc<AtomicBool>);

    impl Drop for Holder {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    fn dropping_the_guard_drops_the_holder() {
        let released = Arc::new(AtomicBool::new(false));
        let requested = Arc::new(AtomicBool::new(false));
        let guard = ReservationGuard::new(Holder(released.clone()), requested.clone());
        assert!(!guard.release_requested());
        requested.store(true, Ordering::Release);
        assert!(guard.release_requested());
        assert!(!released.load(Ordering::Acquire));
        drop(guard);
        assert!(released.load(Ordering::Acquire));
    }

    #[test]
    fn the_default_takes_nothing() {
        let card = ReservedCard {
            index: 0,
            id: "X".into(),
            device: "hw:CARD=X,DEV=0".into(),
        };
        assert_eq!(
            SharedReservation::default().acquire(&card).unwrap_err(),
            ReservationError::Unsupported
        );
        let a = SharedReservation::default();
        assert_eq!(a, a.clone());
        assert_ne!(a, SharedReservation::default());
    }

    #[test]
    fn refusal_names_the_holder() {
        let error = ReservationError::Refused {
            holder: Some("JACK".into()),
        };
        assert_eq!(error.to_string(), "the card is reserved by JACK");
    }
}
