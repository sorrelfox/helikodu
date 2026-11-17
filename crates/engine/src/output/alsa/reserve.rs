// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening a device with device reservation: take the card from the sound
//! server first (exclusive mode) or when the device turns out to be busy (shared
//! mode), then open it.

use std::thread;
use std::time::{Duration, Instant};

use ::alsa as alsa_lib;
use alsa_lib::ctl::Ctl;
use helikodu_model::audio::DeviceSelection;

use super::super::{
    OutputError, ReservationError, ReservationGuard, ReservedCard, SharedReservation,
};
use super::{KIND, devices, libc_errno};

/// How long a device may stay busy after its card was taken: the previous holder
/// needs a moment to close it (about 5 ms for PipeWire).
const REOPEN_TIMEOUT: Duration = Duration::from_secs(1);
/// How often a busy device is tried again meanwhile.
const REOPEN_POLL: Duration = Duration::from_millis(5);

/// The card a `hw:` or `plughw:` device belongs to; `None` for other PCMs.
pub(super) fn card_of(name: &str) -> Option<ReservedCard> {
    let (card, _) = devices::parse_hw_name(name)?;
    let index = devices::card_index(&card)?;
    let id = Ctl::new(&format!("hw:{index}"), false)
        .ok()
        .and_then(|ctl| ctl.card_info().ok()?.get_id().ok().map(str::to_owned))
        .unwrap_or(card);
    Some(ReservedCard {
        index: u32::try_from(index).ok()?,
        id,
        device: name.to_owned(),
    })
}

/// What [`open`] needs to know besides how to open.
pub(super) struct Target<'a> {
    pub(super) reservation: &'a SharedReservation,
    /// The card to take, if the device belongs to one.
    pub(super) card: Option<&'a ReservedCard>,
    /// Exclusive mode: take the card before opening.
    pub(super) exclusive: bool,
    pub(super) device: &'a DeviceSelection,
    /// The PCM name, to find who holds a busy device.
    pub(super) name: &'a str,
}

/// Opens the device with `open`, taking its card when needed. The guard, if any,
/// must be kept for as long as the device is open.
///
/// A card the reservation could not take because its holder refused gives
/// [`OutputError::DeviceBusy`] naming the holder. When reservation is not
/// available or fails otherwise, the device is opened anyway; a busy device is
/// then reported busy, naming the program that holds it.
pub(super) fn open<T>(
    target: &Target<'_>,
    mut open: impl FnMut() -> Result<T, alsa_lib::Error>,
    error: impl Fn(&alsa_lib::Error) -> OutputError,
) -> Result<(T, Option<ReservationGuard>), OutputError> {
    let Some(card) = target.card else {
        return open().map(|t| (t, None)).map_err(|e| error(&e));
    };
    let mut asked = target.exclusive;
    let mut guard = if asked { take(target, card)? } else { None };
    let mut taken_at = Instant::now();
    loop {
        match open() {
            Ok(t) => return Ok((t, guard)),
            Err(e) if e.errno() == libc_errno::EBUSY => {
                if !asked {
                    asked = true;
                    guard = take(target, card)?;
                    taken_at = Instant::now();
                    if guard.is_some() {
                        continue;
                    }
                } else if guard.is_some() && taken_at.elapsed() < REOPEN_TIMEOUT {
                    thread::sleep(REOPEN_POLL);
                    continue;
                }
                return Err(error(&e));
            }
            Err(e) => return Err(error(&e)),
        }
    }
}

/// Takes the card. `Ok(None)` when reservation is not available or failed for a
/// reason other than a refusal.
fn take(target: &Target<'_>, card: &ReservedCard) -> Result<Option<ReservationGuard>, OutputError> {
    match target.reservation.acquire(card) {
        Ok(guard) => Ok(Some(guard)),
        Err(ReservationError::Refused { holder }) => Err(OutputError::DeviceBusy {
            backend: KIND,
            device: target.device.clone(),
            owner: holder.or_else(|| devices::owner(target.name)),
        }),
        Err(ReservationError::Unsupported | ReservationError::Failed(_)) => Ok(None),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::Cell;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    use super::super::open_error;
    use super::*;
    use crate::output::DeviceReservation;

    /// A reservation that logs what happens, for the order of events.
    #[derive(Debug, Clone)]
    pub(in super::super) struct FakeReservation {
        pub(in super::super) log: Arc<Mutex<Vec<String>>>,
        answer: Option<ReservationError>,
    }

    impl FakeReservation {
        pub(in super::super) fn granting() -> Self {
            Self {
                log: Arc::default(),
                answer: None,
            }
        }

        pub(in super::super) fn refusing(error: ReservationError) -> Self {
            Self {
                log: Arc::default(),
                answer: Some(error),
            }
        }

        pub(in super::super) fn note(&self, entry: &str) {
            self.log.lock().unwrap().push(entry.to_owned());
        }

        pub(in super::super) fn entries(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    #[derive(Debug)]
    struct Holder(FakeReservation, u32);

    impl Drop for Holder {
        fn drop(&mut self) {
            self.0.note(&format!("release {}", self.1));
        }
    }

    impl DeviceReservation for FakeReservation {
        fn acquire(&self, card: &ReservedCard) -> Result<ReservationGuard, ReservationError> {
            self.note(&format!("acquire {}", card.index));
            match &self.answer {
                Some(error) => Err(error.clone()),
                None => Ok(ReservationGuard::new(
                    Holder(self.clone(), card.index),
                    Arc::new(AtomicBool::new(false)),
                )),
            }
        }
    }

    pub(in super::super) fn card() -> ReservedCard {
        ReservedCard {
            index: 3,
            id: "Generic_1".into(),
            device: "hw:CARD=Generic_1,DEV=1".into(),
        }
    }

    fn busy() -> alsa_lib::Error {
        alsa_lib::Error::new("snd_pcm_open", libc_errno::EBUSY)
    }

    struct Case {
        reservation: SharedReservation,
        card: Option<ReservedCard>,
        exclusive: bool,
        device: DeviceSelection,
    }

    impl Case {
        fn new(fake: &FakeReservation, exclusive: bool) -> Self {
            Self {
                reservation: SharedReservation::new(fake.clone()),
                card: Some(card()),
                exclusive,
                device: "hw:CARD=Generic_1,DEV=1".parse().unwrap(),
            }
        }

        /// Opens with a device that is busy for the first `busy_opens` attempts.
        fn open(
            &self,
            fake: &FakeReservation,
            busy_opens: u32,
        ) -> Result<((), Option<ReservationGuard>), OutputError> {
            let attempts = Cell::new(0);
            let target = Target {
                reservation: &self.reservation,
                card: self.card.as_ref(),
                exclusive: self.exclusive,
                device: &self.device,
                name: "hw:CARD=Generic_1,DEV=1",
            };
            open(
                &target,
                || {
                    attempts.set(attempts.get() + 1);
                    if attempts.get() <= busy_opens {
                        fake.note("open busy");
                        Err(busy())
                    } else {
                        fake.note("open");
                        Ok(())
                    }
                },
                |e| open_error(&self.device, "hw:CARD=Generic_1,DEV=1", e),
            )
        }
    }

    #[test]
    fn exclusive_takes_the_card_before_opening_and_keeps_it() {
        let fake = FakeReservation::granting();
        let ((), guard) = Case::new(&fake, true).open(&fake, 0).unwrap();
        assert!(guard.is_some());
        assert_eq!(fake.entries(), ["acquire 3", "open"]);
        drop(guard);
        assert_eq!(fake.entries(), ["acquire 3", "open", "release 3"]);
    }

    #[test]
    fn exclusive_waits_while_the_old_holder_closes_the_device() {
        let fake = FakeReservation::granting();
        let ((), guard) = Case::new(&fake, true).open(&fake, 2).unwrap();
        assert!(guard.is_some());
        assert_eq!(
            fake.entries(),
            ["acquire 3", "open busy", "open busy", "open"]
        );
    }

    #[test]
    fn shared_takes_the_card_only_when_busy() {
        let fake = FakeReservation::granting();
        let ((), guard) = Case::new(&fake, false).open(&fake, 0).unwrap();
        assert!(guard.is_none());
        assert_eq!(fake.entries(), ["open"]);

        let fake = FakeReservation::granting();
        let ((), guard) = Case::new(&fake, false).open(&fake, 1).unwrap();
        assert!(guard.is_some());
        assert_eq!(fake.entries(), ["open busy", "acquire 3", "open"]);
    }

    #[test]
    fn a_refusal_is_device_busy_naming_the_holder() {
        for exclusive in [true, false] {
            let fake = FakeReservation::refusing(ReservationError::Refused {
                holder: Some("JACK".into()),
            });
            let error = Case::new(&fake, exclusive).open(&fake, 1).unwrap_err();
            assert!(
                matches!(&error, OutputError::DeviceBusy { owner: Some(o), .. } if o == "JACK"),
                "{error}"
            );
            // In exclusive mode the device is not even tried.
            let expected: &[&str] = if exclusive {
                &["acquire 3"]
            } else {
                &["open busy", "acquire 3"]
            };
            assert_eq!(fake.entries(), expected);
        }
    }

    #[test]
    fn without_reservation_a_busy_device_is_busy() {
        for answer in [
            ReservationError::Unsupported,
            ReservationError::Failed("no session bus".into()),
        ] {
            let fake = FakeReservation::refusing(answer);
            let error = Case::new(&fake, true).open(&fake, 5).unwrap_err();
            assert!(matches!(error, OutputError::DeviceBusy { .. }), "{error}");
            // Tried once, not again for a second.
            assert_eq!(fake.entries(), ["acquire 3", "open busy"]);
        }
    }

    #[test]
    fn a_device_that_stays_busy_gives_up_and_releases() {
        let fake = FakeReservation::granting();
        let start = Instant::now();
        let error = Case::new(&fake, true).open(&fake, u32::MAX).unwrap_err();
        assert!(matches!(error, OutputError::DeviceBusy { .. }), "{error}");
        assert!(start.elapsed() >= REOPEN_TIMEOUT);
        let entries = fake.entries();
        assert_eq!(entries.first().map(String::as_str), Some("acquire 3"));
        assert_eq!(entries.last().map(String::as_str), Some("release 3"));
    }

    #[test]
    fn other_pcms_are_not_reserved() {
        let fake = FakeReservation::granting();
        let mut case = Case::new(&fake, true);
        case.card = None;
        let ((), guard) = case.open(&fake, 0).unwrap();
        assert!(guard.is_none());
        assert_eq!(fake.entries(), ["open"]);
        assert!(card_of("default").is_none());
        assert!(card_of("pipewire").is_none());
    }
}
