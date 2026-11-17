// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding one audio stream to PCM, gapless trimming and sample-accurate seeking.

mod anchor;
mod audio_decoder;
mod end_trim;
mod owned;
mod pcm;

pub use audio_decoder::AudioDecoder;
pub use pcm::{PcmBuffer, PcmSamples};
