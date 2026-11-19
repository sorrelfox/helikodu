// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Background jobs that work through many files: the loudness scanner, the check
//! of files ("Verify files") and the track analysis.
//!
//! A job runs on threads of its own at a lowered scheduling priority, reports its
//! progress as events and can be cancelled; what it finished before it was
//! cancelled is kept.

pub mod analysis;
pub mod loudness;
pub mod verify;
