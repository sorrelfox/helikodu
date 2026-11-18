// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoders for the long tail of formats: tracker modules, game music, SID, MIDI and DSD files.
//!
//! Each decoder sits behind a Cargo feature of its own. The synthesizing decoders
//! (tracker modules, game music, SID tunes, game audio streams and MIDI) share the
//! [`song`] interface: subsongs, lengths, loops and fades. Every `unsafe` block
//! is in the module of the library it calls, and no panic crosses into C: the
//! libraries' callbacks are their own, and the SID shim catches every C++ exception.
//!
//! See the crate's README for its place in the workspace.

#![warn(missing_docs)]

#[cfg(feature = "dsd")]
pub mod dsd;
#[cfg(feature = "gme")]
pub mod gme;
#[cfg(feature = "midi")]
pub mod midi;
#[cfg(feature = "openmpt")]
pub mod openmpt;
#[cfg(feature = "sid")]
pub mod sid;
#[cfg(feature = "song")]
pub mod song;
#[cfg(feature = "vgmstream")]
pub mod vgmstream;
