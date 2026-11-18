// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The D-Bus services against a private session bus started for each test
//! (`dbus-daemon --session`): MPRIS with a fake player, and the appearance
//! reader with a fake Settings portal. Skipped, with a note, where `dbus-daemon`
//! cannot be run.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

mod bus {
    pub(crate) mod mpris;
    pub(crate) mod private_bus;
    pub(crate) mod settings_portal;
}
