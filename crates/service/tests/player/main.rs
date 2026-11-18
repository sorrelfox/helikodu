// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The player service, through its public API: queues of generated WAV files played
//! to the null and WAV sinks.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

mod controls;
mod crash;
mod media;
mod modes;
mod play_modes;
mod queue;
mod session;
mod stats;
