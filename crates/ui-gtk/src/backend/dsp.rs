// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the DSP and EQ page needs beyond the settings: the compressor's
//! activity, the impulse responses that convolution would use, and the
//! equalizers of single tracks.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use helikodu_model::settings::ConvolutionSettings;
use helikodu_service::dsp::impulse::{self, ImpulseError, MAX_SECONDS};

/// What the compressor did, for its editor.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CompressorReading {
    /// The gain reduction now, in dB (0 or positive).
    pub reduction_db: f64,
    /// The largest reduction in this track, in dB.
    pub largest_db: f64,
}

/// Why an impulse response cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpulseProblem {
    /// The file is not there.
    Missing,
    /// The file is not a WAV file the engine reads.
    NotWav,
    /// The file is longer than the engine takes.
    TooLong,
    /// The file cannot be read for another reason.
    Unreadable,
}

/// One impulse response, as the convolution editor lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpulseEntry {
    /// The sample rate it is for, in Hz.
    pub rate_hz: u32,
    /// The file.
    pub path: PathBuf,
    /// Whether it comes from the folder (else it was chosen by hand).
    pub from_folder: bool,
    /// Its length in seconds, when it can be read.
    pub seconds: Option<f64>,
    /// Its channels, when it can be read.
    pub channels: Option<usize>,
    /// Why it cannot be used, if it cannot.
    pub problem: Option<ImpulseProblem>,
}

/// The DSP page's needs beyond the settings store.
pub trait DspServices: fmt::Debug + Send + Sync {
    /// What the compressor did lately, when the player reports it; `None`
    /// while it is off or nothing plays.
    fn compressor_activity(&self) -> Option<CompressorReading> {
        None
    }

    /// The impulse responses `settings` name: the files chosen by hand and
    /// the WAV files of the folder, one for each rate, by rate. Blocking: it
    /// reads the files' headers.
    fn impulse_files(&self, settings: &ConvolutionSettings) -> Vec<ImpulseEntry> {
        read_impulse_files(settings)
    }

    /// The sample rate of the WAV file at `path`, from its header. Blocking.
    fn impulse_rate(&self, path: &Path) -> Option<u32> {
        impulse::read_info(path).ok().map(|info| info.rate)
    }

    /// How many tracks have an equalizer of their own. Blocking.
    fn tracks_with_equalizer(&self) -> usize {
        0
    }

    /// The name of the equalizer of the track that plays, when it has one
    /// and the store keeps a name for it.
    fn playing_equalizer_name(&self) -> Option<String> {
        None
    }
}

/// No DSP services: nothing is reported, the files are read from disk.
#[derive(Debug, Default)]
pub struct NoDsp;

impl DspServices for NoDsp {}

/// Why `error` keeps an impulse response from being used.
fn problem_of(error: &ImpulseError) -> ImpulseProblem {
    match error {
        ImpulseError::Read { source, .. } if source.kind() == io::ErrorKind::NotFound => {
            ImpulseProblem::Missing
        }
        ImpulseError::Format { reason, .. } if reason.contains("longer than") => {
            ImpulseProblem::TooLong
        }
        ImpulseError::Format { .. } => ImpulseProblem::NotWav,
        _ => ImpulseProblem::Unreadable,
    }
}

/// The entry for `path`, read from its header as the engine reads it.
fn entry(path: &Path, rate_hz: u32, from_folder: bool) -> ImpulseEntry {
    let mut entry = ImpulseEntry {
        rate_hz,
        path: path.to_owned(),
        from_folder,
        seconds: None,
        channels: None,
        problem: None,
    };
    match impulse::read_info(path) {
        Ok(info) => {
            let seconds = info.frames as f64 / f64::from(info.rate.max(1));
            entry.seconds = Some(seconds);
            entry.channels = Some(info.channels);
            if seconds > MAX_SECONDS {
                entry.problem = Some(ImpulseProblem::TooLong);
            } else if !(1..=2).contains(&info.channels) {
                entry.problem = Some(ImpulseProblem::NotWav);
            }
        }
        Err(error) => entry.problem = Some(problem_of(&error)),
    }
    entry
}

/// The impulse responses `settings` name, read from disk: each file chosen
/// by hand, then for every other rate the first WAV file of the folder (by
/// name) recorded at it, as the engine chooses. Sorted by rate.
pub fn read_impulse_files(settings: &ConvolutionSettings) -> Vec<ImpulseEntry> {
    let mut entries: Vec<ImpulseEntry> = settings
        .files
        .iter()
        .map(|file| entry(&file.path, file.rate_hz, false))
        .collect();
    if let Some(folder) = &settings.folder
        && let Ok(read) = std::fs::read_dir(folder)
    {
        let mut files: Vec<PathBuf> = read
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
            })
            .collect();
        files.sort();
        for path in files {
            // A file of the folder whose rate cannot be read is not used.
            let Ok(info) = impulse::read_info(&path) else {
                continue;
            };
            if entries.iter().any(|e| e.rate_hz == info.rate) {
                continue;
            }
            entries.push(entry(&path, info.rate, true));
        }
    }
    entries.sort_by_key(|e| e.rate_hz);
    entries
}

/// The entry convolution uses at `rate` Hz, as the engine chooses it: the
/// one for that rate, else the one with the nearest rate (resampled).
pub fn entry_for(entries: &[ImpulseEntry], rate: u32) -> Option<&ImpulseEntry> {
    entries
        .iter()
        .min_by_key(|e| (i64::from(e.rate_hz) - i64::from(rate)).unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use helikodu_model::settings::ImpulseFile;

    #[test]
    fn files_are_listed_by_rate_with_their_problems() {
        let dir = std::env::temp_dir().join(format!("helikodu-ui-ir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let response = vec![1.0, 0.5, 0.25, 0.0];
        impulse::write_wav(
            &dir.join("room-44k.wav"),
            44_100,
            std::slice::from_ref(&response),
        )
        .unwrap();
        impulse::write_wav(
            &dir.join("room-48k.wav"),
            48_000,
            &[response.clone(), response],
        )
        .unwrap();
        std::fs::write(dir.join("notes.wav"), b"not a wav").unwrap();
        let settings = ConvolutionSettings {
            folder: Some(dir.clone()),
            files: vec![ImpulseFile {
                rate_hz: 96_000,
                path: dir.join("missing.wav"),
            }],
            ..ConvolutionSettings::default()
        };
        let entries = read_impulse_files(&settings);
        let rates: Vec<u32> = entries.iter().map(|e| e.rate_hz).collect();
        assert_eq!(rates, [44_100, 48_000, 96_000]);
        assert_eq!(entries[0].channels, Some(1));
        assert_eq!(entries[1].channels, Some(2));
        assert!(entries[0].from_folder);
        assert_eq!(entries[2].problem, Some(ImpulseProblem::Missing));
        // 88.2 kHz has no file of its own: the nearest one is used.
        assert_eq!(entry_for(&entries, 88_200).unwrap().rate_hz, 96_000);
        assert_eq!(entry_for(&entries, 47_000).unwrap().rate_hz, 48_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
