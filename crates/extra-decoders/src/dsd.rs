// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! DSD files: the DSF and DSDIFF (DFF) reader and the conversion of DSD to PCM.
//!
//! [`DsdReader`] parses the container and yields the raw 1-bit stream in one
//! canonical form whatever the file stores: per channel, bytes in time order with
//! the earliest bit in the most significant position, channels in WAVE order. DSF
//! stores each byte least-significant bit first in per-channel blocks; DFF stores
//! bytes most-significant bit first, interleaved per channel. That form is what DoP
//! and native DSD output will need.
//!
//! [`DsdPcmDecoder`] turns the stream into PCM: a linear-phase FIR low-pass filter
//! decimates it to 176.4 kHz (DSD64, DSD128, DSD256, DSD512) or 192 kHz (the 48 kHz
//! family), as interleaved `f32`. The output is aligned with the DSD stream (the
//! filter delay is compensated), has exactly `bits / factor` frames, and is raised
//! by 6 dB, because the DSD 0 dB reference (50 % modulation) corresponds to −6 dBFS
//! in PCM.
//!
//! DST-compressed DFF files are refused with [`DsdError::Unsupported`]; FFmpeg
//! decodes them.

#![forbid(unsafe_code)]

mod decimator;
mod dff;
mod dsf;
mod error;
mod pcm_decoder;
mod reader;

pub use decimator::DSD_GAIN;
pub use error::DsdError;
pub use pcm_decoder::DsdPcmDecoder;
pub use reader::{DsdContainer, DsdInfo, DsdReader, IDLE_BYTE};

/// Whether `header`, the first bytes of a file, starts like a DSF or DFF file.
/// Twelve or more bytes are needed to recognise a DFF file, four for DSF.
pub fn sniff(header: &[u8]) -> Option<DsdContainer> {
    if header.starts_with(b"DSD ") {
        Some(DsdContainer::Dsf)
    } else if header.len() >= 16 && header.starts_with(b"FRM8") && &header[12..16] == b"DSD " {
        Some(DsdContainer::Dff)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_both_containers() {
        assert_eq!(sniff(b"DSD \x1c\0\0\0"), Some(DsdContainer::Dsf));
        assert_eq!(
            sniff(b"FRM8\0\0\0\0\0\0\0\x10DSD FVER"),
            Some(DsdContainer::Dff)
        );
        assert_eq!(sniff(b"FRM8\0\0\0\0\0\0\0\x10AIFF"), None);
        assert_eq!(sniff(b"fLaC"), None);
        assert_eq!(sniff(b""), None);
    }
}
