// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the registry and its backends open files with: the decoder settings,
//! whether DSD files go out as DoP, and the whole-file buffering limit.

use std::sync::{Arc, PoisonError, RwLock};

use helikodu_model::settings::{DecoderSettings, MEMORY_BUFFER_LIMIT};

/// How files are opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeOptions {
    /// The settings of the long-tail decoders and of whole-file buffering.
    pub settings: DecoderSettings,
    /// Whether DSD files are opened as DSD over PCM (DoP) rather than converted to
    /// PCM. The transport sets it when the output path would leave the samples
    /// untouched; a DoP source on a path that would change them falls back to PCM
    /// ([`Decoder::pcm_fallback`](super::Decoder::pcm_fallback)).
    pub dop: bool,
    /// Files larger than this, in bytes, are never read into memory whole:
    /// [`MEMORY_BUFFER_LIMIT`] (512 MiB) unless lowered.
    pub memory_limit: u64,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            settings: DecoderSettings::default(),
            dop: false,
            memory_limit: MEMORY_BUFFER_LIMIT,
        }
    }
}

/// Options shared between the registry and its backends; the registry changes them
/// and the backends read them when they open a file.
#[derive(Debug, Clone, Default)]
pub struct SharedOptions(Arc<RwLock<DecodeOptions>>);

impl SharedOptions {
    /// The current options.
    pub fn get(&self) -> DecodeOptions {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Changes the options.
    pub fn update(&self, change: impl FnOnce(&mut DecodeOptions)) {
        change(&mut self.0.write().unwrap_or_else(PoisonError::into_inner));
    }
}
