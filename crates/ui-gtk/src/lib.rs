// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The GTK 4 front end: windows, views, widgets and style.
//!
//! See the crate's README for its place in the workspace. [`run`] starts the
//! application with a [`backend::Backend`], the front end's adapter over the
//! `service` crate.

#![forbid(unsafe_code)]

mod application;
pub mod backend;
#[doc(hidden)]
pub mod chooser;
mod deck;
mod dialogs;
mod format;
pub mod i18n;
mod mini_player;
mod models;
mod play_modes;
mod playback;
mod popovers;
mod query_text;
mod remote_api;
mod shortcuts;
pub mod snapshot;
pub mod style;
mod tools;
mod views;
mod widgets;
mod window;

pub use application::{
    APP_ID, RunOptions, install_actions, prepare_display, register_resources, request_restart,
    restart_requested, run,
};
pub use views::generator::GeneratorState;
pub use views::similar::SimilarState;
pub use window::{MainWindow, WindowStyle};

/// The path of the resource bundle, from the application ID.
pub(crate) const RESOURCE_BASE: &str = "/io/github/sorrelfox/Helikodu";
