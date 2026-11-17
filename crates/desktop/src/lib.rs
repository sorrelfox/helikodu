// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Desktop integration through freedesktop standards and XDG portals.
//!
//! See the crate's README for its place in the workspace.
//!
//! - [`reservation`]: taking a sound card from the sound server for exclusive
//!   ALSA output (`org.freedesktop.ReserveDevice1`).

#![forbid(unsafe_code)]

pub mod reservation;
