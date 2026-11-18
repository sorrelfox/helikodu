// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Desktop integration through freedesktop standards and XDG portals.
//!
//! See the crate's README for its place in the workspace.
//!
//! - [`mpris`]: MPRIS2 for media widgets, lock screens and media keys.
//! - [`power`]: keeping the system from suspending while playing (the Inhibit
//!   portal, or logind outside a sandbox).
//! - [`portals`]: the appearance settings of the desktop (colour scheme, accent
//!   colour, contrast, reduced motion) through the Settings portal.
//! - [`open_folder`]: showing a file in the file manager.
//! - [`reservation`]: taking a sound card from the sound server for exclusive
//!   ALSA output (`org.freedesktop.ReserveDevice1`).
//! - [`tray`]: the tray icon and its menu (StatusNotifierItem and dbusmenu).
//! - [`global_shortcuts`]: shortcuts that work in other applications, through
//!   the GlobalShortcuts portal.
//! - [`session_watch`]: the screen lock and sleep, so that playback can pause.

#![forbid(unsafe_code)]

pub mod global_shortcuts;
pub mod mpris;
pub mod open_folder;
pub mod portals;
pub mod power;
pub mod reservation;
pub mod session_watch;
pub mod tray;

/// The application ID: the `.desktop` file, the AppStream metainfo, the icon and
/// the application's own bus name are named after it.
pub const APP_ID: &str = "io.github.sorrelfox.Helikodu";
