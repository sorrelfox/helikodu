// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The D-Bus services against a private session bus started for each test
//! (`dbus-daemon --session`): MPRIS with a fake player, the appearance
//! reader with a fake Settings portal, the tray icon with a fake watcher, the
//! global shortcuts with a fake portal, and the lock and sleep watch with a
//! fake logind and Inhibit portal. Skipped, with a note, where `dbus-daemon`
//! cannot be run.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

mod bus {
    pub(crate) mod fake_portal;
    pub(crate) mod global_shortcuts;
    pub(crate) mod mpris;
    pub(crate) mod private_bus;
    pub(crate) mod session_watch;
    pub(crate) mod settings_portal;
    pub(crate) mod tray;
}
