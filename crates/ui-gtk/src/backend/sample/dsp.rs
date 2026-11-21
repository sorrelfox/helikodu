// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSP page's needs over invented data: a compressor at work, a folder
//! of room corrections for 44.1 and 48 kHz, and three tracks with an
//! equalizer of their own, as the design shows them.

use std::path::Path;

use helikodu_model::settings::ConvolutionSettings;

use super::super::dsp::{CompressorReading, DspServices, ImpulseEntry, ImpulseProblem};

/// The DSP page's needs over invented data.
#[derive(Debug, Default)]
pub(in crate::backend) struct SampleDsp;

/// An invented file: a name with "broken" in it is not a WAV file, one with
/// "missing" is not there; any other is a mono response of 0.68 s.
fn invented(path: &Path, rate_hz: u32, from_folder: bool) -> ImpulseEntry {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let problem = if name.contains("broken") {
        Some(ImpulseProblem::NotWav)
    } else if name.contains("missing") {
        Some(ImpulseProblem::Missing)
    } else {
        None
    };
    ImpulseEntry {
        rate_hz,
        path: path.to_owned(),
        from_folder,
        seconds: problem
            .is_none()
            .then_some(if from_folder { 0.74 } else { 0.68 }),
        channels: problem.is_none().then_some(if from_folder { 2 } else { 1 }),
        problem,
    }
}

impl DspServices for SampleDsp {
    fn compressor_activity(&self) -> Option<CompressorReading> {
        Some(CompressorReading {
            reduction_db: 6.5,
            largest_db: 14.2,
        })
    }

    fn impulse_files(&self, settings: &ConvolutionSettings) -> Vec<ImpulseEntry> {
        let mut entries: Vec<ImpulseEntry> = settings
            .files
            .iter()
            .map(|f| invented(&f.path, f.rate_hz, false))
            .collect();
        if let Some(folder) = &settings.folder {
            for (rate, name) in [(44_100, "room-44k.wav"), (48_000, "room-48k.wav")] {
                if !entries.iter().any(|e| e.rate_hz == rate) {
                    entries.push(invented(&folder.join(name), rate, true));
                }
            }
        }
        entries.sort_by_key(|e| e.rate_hz);
        entries
    }

    fn tracks_with_equalizer(&self) -> usize {
        3
    }

    fn playing_equalizer_name(&self) -> Option<String> {
        Some("Bright master fix".into())
    }
}
