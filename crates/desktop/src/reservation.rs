// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Audio device reservation: `org.freedesktop.ReserveDevice1` on the session bus.
//!
//! A sound server (PipeWire through WirePlumber, PulseAudio, JACK) announces each
//! sound card it uses by owning the bus name
//! `org.freedesktop.ReserveDevice1.Audio<N>`, where `N` is the ALSA card index.
//! A program that wants the card for itself asks the owner to let go
//! (`RequestRelease` with its priority), takes the name with `REPLACE_EXISTING`,
//! and from then on serves the same interface at
//! `/org/freedesktop/ReserveDevice1/Audio<N>`, so that others can ask it in turn.
//! The previous owner closes the device; when the name is given back, it takes
//! the name and the device again.
//!
//! [`DeviceReserver::acquire`] does this for one card and returns a
//! [`ReservedDevice`]; dropping it gives the card back. While it is held, every
//! `RequestRelease` from another program is answered with `false` (WirePlumber
//! asks right away), and [`ReservedDevice::release_requested`] turns true.
//!
//! The calls block the calling thread for a few D-Bus round trips. zbus runs its
//! own small executor for the connection; no general async runtime is involved.

mod cards;
mod object;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::{Connection, OwnedVariant};

pub use self::cards::card_index;
use self::object::Reservation;

/// The application name other programs see as the holder of a card.
pub const APPLICATION_NAME: &str = "Helikodu";
/// The D-Bus interface of the protocol.
pub const INTERFACE: &str = "org.freedesktop.ReserveDevice1";

/// How long a call to the current owner may take before the card counts as not
/// released.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a card is held at least. WirePlumber asks for a card back within
/// milliseconds of losing it; a name given back while it is still tearing down
/// the card's nodes makes it create them again under new names (with a `.2`
/// suffix), which breaks every setting that refers to them by name.
const MIN_HOLD: Duration = Duration::from_millis(250);

/// The bus name of the reservation of ALSA card `card`.
pub fn bus_name(card: u32) -> String {
    format!("{INTERFACE}.Audio{card}")
}

/// The object path of the reservation of ALSA card `card`.
pub fn object_path(card: u32) -> String {
    format!("/org/freedesktop/ReserveDevice1/Audio{card}")
}

/// Why a card could not be reserved.
#[derive(Debug, Error)]
pub enum ReserveError {
    /// The program holding the card did not let it go.
    #[error("the card is reserved{}", holder.as_ref().map(|h| format!(" by {h}")).unwrap_or_default())]
    Refused {
        /// Its `ApplicationName`, if it has one, for example `JACK`.
        holder: Option<String>,
    },
    /// This program already holds the card.
    #[error("the card is already reserved by this program")]
    AlreadyHeld,
    /// The session bus could not be used.
    #[error("D-Bus: {0}")]
    Bus(#[from] zbus::Error),
}

/// Takes sound cards through `org.freedesktop.ReserveDevice1`.
///
/// Cheap to create: it connects to the session bus only when a card is acquired,
/// with one connection per reserved card, so that the bus gives the name back
/// even if this process ends abruptly.
#[derive(Debug, Clone)]
pub struct DeviceReserver {
    priority: i32,
    application_name: String,
}

impl DeviceReserver {
    /// The priority Helikodu claims cards with. Above PulseAudio (0) and
    /// WirePlumber (-20), below JACK (`i32::MAX`), so a running JACK server is
    /// not asked to give up its card (it would refuse anyway).
    pub const DEFAULT_PRIORITY: i32 = 10;

    /// A reserver with [`DEFAULT_PRIORITY`](Self::DEFAULT_PRIORITY) and the
    /// application name `Helikodu`.
    pub fn new() -> Self {
        Self {
            priority: Self::DEFAULT_PRIORITY,
            application_name: APPLICATION_NAME.to_owned(),
        }
    }

    /// The same reserver with another priority.
    #[must_use]
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// The priority sent with `RequestRelease` and shown as the `Priority`
    /// property.
    pub fn priority(&self) -> i32 {
        self.priority
    }

    /// Takes ALSA card `card` (its index, see [`card_index`]). `device_name` is
    /// shown to other programs as `ApplicationDeviceName`, for example
    /// `hw:CARD=Generic_1,DEV=1`.
    ///
    /// When the call returns, the previous owner has agreed to close the device;
    /// opening it can still fail with "busy" for a few milliseconds.
    pub fn acquire(&self, card: u32, device_name: &str) -> Result<ReservedDevice, ReserveError> {
        let name = bus_name(card);
        let path = object_path(card);
        let release_requested = Arc::new(AtomicBool::new(false));
        let object = Reservation {
            priority: self.priority,
            application_name: self.application_name.clone(),
            device_name: device_name.to_owned(),
            release_requested: release_requested.clone(),
        };
        let connection = zbus::block_on(async {
            let connection = zbus::connection::Builder::session()
                .method_timeout(CALL_TIMEOUT)
                .build()
                .await?;
            // Serve the object before taking the name, as the protocol expects.
            if connection
                .object_server()
                .at(path.as_str(), object)
                .await?
                .is_some()
            {
                return Err(ReserveError::AlreadyHeld);
            }
            take_name(&connection, &name, &path, self.priority).await?;
            Ok(connection)
        })?;
        Ok(ReservedDevice {
            card,
            name,
            path,
            connection: Some(connection),
            taken_at: Instant::now(),
            release_requested,
        })
    }
}

impl Default for DeviceReserver {
    fn default() -> Self {
        Self::new()
    }
}

/// Takes `name`: at once if nobody owns it, otherwise after its owner agreed.
async fn take_name(
    connection: &Connection,
    name: &str,
    path: &str,
    priority: i32,
) -> Result<(), ReserveError> {
    match connection
        .request_name_with_flags(name, RequestNameFlags::DO_NOT_QUEUE)
        .await
    {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => return Ok(()),
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(zbus::Error::NameTaken) => {}
        Err(error) => return Err(error.into()),
    }

    let granted = match connection
        .call_method(
            Some(name),
            path,
            Some(INTERFACE),
            "RequestRelease",
            &(priority,),
        )
        .await
    {
        Ok(reply) => reply.body().deserialize::<bool>().unwrap_or(false),
        // An owner that does not answer has not let go.
        Err(_) => false,
    };
    if !granted {
        return Err(ReserveError::Refused {
            holder: holder(connection, name, path).await,
        });
    }
    // Without ALLOW_REPLACEMENT: nobody can take the name back from us; they
    // have to ask.
    let reply = connection
        .request_name_with_flags(
            name,
            RequestNameFlags::DO_NOT_QUEUE | RequestNameFlags::REPLACE_EXISTING,
        )
        .await;
    match reply {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => Ok(()),
        Ok(_) | Err(zbus::Error::NameTaken) => Err(ReserveError::Refused {
            holder: holder(connection, name, path).await,
        }),
        Err(error) => Err(error.into()),
    }
}

/// The `ApplicationName` of the program that owns `name`.
async fn holder(connection: &Connection, name: &str, path: &str) -> Option<String> {
    let reply = connection
        .call_method(
            Some(name),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(INTERFACE, "ApplicationName"),
        )
        .await
        .ok()?;
    let value = reply.body().deserialize::<OwnedVariant>().ok()?;
    String::try_from(value).ok().filter(|name| !name.is_empty())
}

/// A sound card taken from its previous owner. Dropping it gives the card back.
///
/// A card is held for at least a quarter of a second: giving it back sooner
/// waits for the rest, because WirePlumber renames the card's nodes when it gets
/// a card back before it has finished letting go of it.
pub struct ReservedDevice {
    card: u32,
    name: String,
    path: String,
    connection: Option<Connection>,
    taken_at: Instant,
    release_requested: Arc<AtomicBool>,
}

impl ReservedDevice {
    /// The ALSA card index.
    pub fn card(&self) -> u32 {
        self.card
    }

    /// The bus name held, `org.freedesktop.ReserveDevice1.Audio<N>`.
    pub fn bus_name(&self) -> &str {
        &self.name
    }

    /// Whether another program has asked for the card since it was taken. Such
    /// requests are refused while the card is held.
    pub fn release_requested(&self) -> bool {
        self.release_requested.load(Ordering::Acquire)
    }

    /// The flag behind [`release_requested`](Self::release_requested), for
    /// whoever keeps the device on behalf of the engine.
    pub fn release_requested_flag(&self) -> Arc<AtomicBool> {
        self.release_requested.clone()
    }

    /// Gives the card back now, reporting errors that dropping would ignore.
    pub fn release(mut self) -> Result<(), ReserveError> {
        self.give_back()
    }

    fn give_back(&mut self) -> Result<(), ReserveError> {
        let Some(connection) = self.connection.take() else {
            return Ok(());
        };
        if let Some(rest) = MIN_HOLD.checked_sub(self.taken_at.elapsed()) {
            thread::sleep(rest);
        }
        zbus::block_on(async {
            let released = connection.release_name(self.name.as_str()).await;
            let removed = connection
                .object_server()
                .remove::<Reservation, _>(self.path.as_str())
                .await;
            // Closing the connection would give the name back as well.
            let _ = connection.close().await;
            released?;
            removed?;
            Ok(())
        })
    }
}

impl Drop for ReservedDevice {
    fn drop(&mut self) {
        let _ = self.give_back();
    }
}

impl fmt::Debug for ReservedDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReservedDevice")
            .field("name", &self.name)
            .field("held", &self.connection.is_some())
            .field("release_requested", &self.release_requested())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_card_index() {
        assert_eq!(bus_name(3), "org.freedesktop.ReserveDevice1.Audio3");
        assert_eq!(object_path(3), "/org/freedesktop/ReserveDevice1/Audio3");
    }

    #[test]
    fn refusal_names_the_holder() {
        let error = ReserveError::Refused {
            holder: Some("JACK".into()),
        };
        assert_eq!(error.to_string(), "the card is reserved by JACK");
        let error = ReserveError::Refused { holder: None };
        assert_eq!(error.to_string(), "the card is reserved");
    }

    #[test]
    fn creating_a_reserver_does_not_connect() {
        let reserver = DeviceReserver::new().with_priority(5);
        assert_eq!(reserver.priority(), 5);
        assert_eq!(
            DeviceReserver::default().priority(),
            DeviceReserver::DEFAULT_PRIORITY
        );
    }
}
