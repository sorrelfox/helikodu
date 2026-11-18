// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Commands for the running application, through its D-Bus interface
//! (`io.github.sorrelfox.Helikodu.Control1`, described in `docs/dev/dbus-api.md`).
//!
//! The application is never started for them: when it is not running, they say
//! so and end with exit code 4.

pub(crate) mod commands;
mod status;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use serde_json::Value;
use zbus::Connection;

/// The bus name the running application owns.
pub(crate) const BUS_NAME: &str = "io.github.sorrelfox.Helikodu";
/// The object that serves the interface.
pub(crate) const OBJECT_PATH: &str = "/io/github/sorrelfox/Helikodu";
/// The interface.
pub(crate) const INTERFACE: &str = "io.github.sorrelfox.Helikodu.Control1";

/// The application is not running, or there is no session bus.
#[derive(Debug)]
pub(crate) struct NotRunning;

impl std::fmt::Display for NotRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Helikodu is not running")
    }
}

impl std::error::Error for NotRunning {}

/// A connection to the running application.
pub(crate) struct Remote {
    connection: Connection,
}

impl Remote {
    /// Connects to the session bus and checks that the application is running.
    pub(crate) fn connect() -> Result<Self> {
        zbus::block_on(async {
            let connection = Connection::session().await.map_err(|_| NotRunning)?;
            let reply = connection
                .call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "NameHasOwner",
                    &(BUS_NAME,),
                )
                .await
                .map_err(|_| NotRunning)?;
            let running: bool = reply.body().deserialize().map_err(|_| NotRunning)?;
            if !running {
                return Err(NotRunning.into());
            }
            Ok(Self { connection })
        })
    }

    /// Calls `method` with `args` and reads its reply as `T`.
    pub(crate) fn call<A, T>(&self, method: &str, args: &A) -> Result<T>
    where
        A: Serialize + zbus::zvariant::DynamicType,
        T: for<'de> serde::Deserialize<'de> + zbus::zvariant::Type,
    {
        zbus::block_on(async {
            let reply = self
                .connection
                .call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, args)
                .await
                .map_err(|error| match error {
                    zbus::Error::MethodError(_, Some(message), _) => anyhow!(message),
                    zbus::Error::FDO(fdo) => match *fdo {
                        zbus::fdo::Error::UnknownMethod(_)
                        | zbus::fdo::Error::UnknownInterface(_)
                        | zbus::fdo::Error::UnknownObject(_) => anyhow!(
                            "the running Helikodu does not offer {method}; it may be an older version"
                        ),
                        other => anyhow!(other.to_string()),
                    },
                    other => anyhow!(other),
                })?;
            reply
                .body()
                .deserialize::<T>()
                .with_context(|| format!("the reply to {method} cannot be read"))
        })
    }

    /// Calls `method`, which returns nothing.
    pub(crate) fn run<A>(&self, method: &str, args: &A) -> Result<()>
    where
        A: Serialize + zbus::zvariant::DynamicType,
    {
        self.call::<A, ()>(method, args)
    }

    /// Calls `method`, which returns JSON text, and reads it.
    pub(crate) fn json(&self, method: &str) -> Result<Value> {
        let text: String = self.call(method, &())?;
        serde_json::from_str(&text).with_context(|| format!("the reply to {method} is not JSON"))
    }
}
