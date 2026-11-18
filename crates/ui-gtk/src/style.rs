// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The theme system: design tokens, colour schemes, the accent and the
//! stylesheet providers.

pub(crate) mod color;
mod css;
pub(crate) mod theme;
pub(crate) mod tokens;

pub use theme::{AccentChoice, SchemeChoice, set_choice};
pub(crate) use theme::{connect_changed, current, install, register_window};
