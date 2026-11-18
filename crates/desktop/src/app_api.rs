// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The application's own D-Bus interface, `io.github.sorrelfox.Helikodu.Control1`:
//! what MPRIS cannot express (enqueue next or last, play a query, rate, export a
//! playlist, the status and the queue as JSON). `helikodu-cli` uses it.
//!
//! The interface is served on the bus name the application already owns as a
//! single instance, at the application's object path, so the window registers it
//! on that connection; this module holds its definition and names. Its
//! introspection data is [`INTERFACE_XML`], which `docs/dev/dbus-api.md`
//! describes.

/// The bus name: the application ID, owned by the running application.
pub const BUS_NAME: &str = crate::APP_ID;

/// The object that serves the interface.
pub const OBJECT_PATH: &str = "/io/github/sorrelfox/Helikodu";

/// The interface name. A new version gets a new name; within a version methods
/// are only added.
pub const INTERFACE: &str = "io.github.sorrelfox.Helikodu.Control1";

/// The version the `Version` property reports.
pub const VERSION: u32 = 1;

/// The D-Bus error a call that cannot be done returns, with a message that says
/// why.
pub const ERROR_FAILED: &str = "io.github.sorrelfox.Helikodu.Error.Failed";

/// The D-Bus error of a call with an argument out of range or not understood.
pub const ERROR_INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";

/// The introspection data of the interface.
pub const INTERFACE_XML: &str = include_str!("app_api/interface.xml");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_match_the_introspection_data() {
        assert!(INTERFACE_XML.contains(&format!("<interface name=\"{INTERFACE}\">")));
        assert!(INTERFACE.starts_with(BUS_NAME));
        assert_eq!(OBJECT_PATH, format!("/{}", BUS_NAME.replace('.', "/")));
    }
}
