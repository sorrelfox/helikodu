// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSP page's needs over the running service: the compressor's activity
//! as the engine last reported it, and the tracks with an equalizer of their
//! own from the library.

use std::sync::Arc;

use helikodu_service::Service;

use super::super::dsp::{CompressorReading, DspServices};

/// The DSP page's needs over the running service.
#[derive(Debug, Clone)]
pub(in crate::backend) struct ServiceDsp {
    pub(in crate::backend) service: Arc<Service>,
}

impl DspServices for ServiceDsp {
    fn compressor_activity(&self) -> Option<CompressorReading> {
        self.service
            .player()
            .status()
            .compressor
            .map(|a| CompressorReading {
                reduction_db: a.reduction_db,
                largest_db: a.largest_db,
            })
    }

    fn tracks_with_equalizer(&self) -> usize {
        self.service
            .library()
            .and_then(|l| l.library().tracks_with_equalizer().ok())
            .map_or(0, |tracks| tracks.len())
    }
}
