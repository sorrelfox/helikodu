// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Impulse responses for convolution: reading WAV files and choosing the one for a
//! sample rate.
//!
//! An impulse response comes from the `files` entry for the rate, or else from the
//! WAV file in the folder recorded at that rate (the first by name). When neither
//! has the rate, the file with the nearest rate is resampled to it. WAV files may
//! hold 8 to 32-bit integers or 32 and 64-bit floats, with one or two channels.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use helikodu_model::audio::SampleRate;
use helikodu_model::settings::ConvolutionSettings;
use realfft::RealFftPlanner;
use thiserror::Error;

use crate::pipeline::{ResampleStage, ResamplerQuality};

/// The longest impulse response, in seconds.
pub const MAX_SECONDS: f64 = 20.0;

/// Why no impulse response could be had.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ImpulseError {
    /// Neither the files nor the folder name one.
    #[error("no impulse response is set")]
    NoneSet,
    /// The folder holds no WAV file.
    #[error("{} holds no WAV file", .0.display())]
    EmptyFolder(PathBuf),
    /// A file or folder could not be read.
    #[error("cannot read {}: {source}", .path.display())]
    Read {
        /// The file or folder.
        path: PathBuf,
        /// Why.
        #[source]
        source: io::Error,
    },
    /// A file is not a WAV file this reader understands.
    #[error("{} is not a usable WAV file: {reason}", .path.display())]
    Format {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        reason: String,
    },
    /// Resampling the impulse response failed.
    #[error("cannot resample {}: {reason}", .path.display())]
    Resample {
        /// The file.
        path: PathBuf,
        /// Why.
        reason: String,
    },
}

/// The format of a WAV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    /// Its sample rate, in Hz.
    pub rate: u32,
    /// Its channels.
    pub channels: usize,
    /// Its length, in frames.
    pub frames: usize,
}

/// An impulse response ready for one sample rate.
#[derive(Clone, PartialEq)]
pub struct Impulse {
    /// The file it was read from.
    pub path: PathBuf,
    /// The file's own rate, in Hz.
    pub file_rate: u32,
    /// The rate the samples are at now, in Hz.
    pub rate: u32,
    /// The samples, one vector per channel (one or two).
    pub channels: Vec<Vec<f64>>,
}

impl fmt::Debug for Impulse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Impulse")
            .field("path", &self.path)
            .field("file_rate", &self.file_rate)
            .field("rate", &self.rate)
            .field("channels", &self.channels.len())
            .field("taps", &self.taps())
            .finish()
    }
}

impl Impulse {
    /// The length, in frames.
    pub fn taps(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// Whether it was resampled from another rate.
    pub fn resampled(&self) -> bool {
        self.file_rate != self.rate
    }

    /// The largest magnitude of its frequency response over all channels, as a
    /// linear factor.
    pub fn peak_gain(&self) -> f64 {
        let size = self.taps().max(2).next_power_of_two().max(8192);
        let mut planner = RealFftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(size);
        let mut input = fft.make_input_vec();
        let mut spectrum = fft.make_output_vec();
        let mut peak: f64 = 0.0;
        for channel in &self.channels {
            input.fill(0.0);
            input[..channel.len()].copy_from_slice(channel);
            if fft.process(&mut input, &mut spectrum).is_ok() {
                peak = spectrum.iter().map(|c| c.norm()).fold(peak, f64::max);
            }
        }
        peak
    }
}

/// The impulse response `settings` name for `rate` Hz.
pub fn find(settings: &ConvolutionSettings, rate: u32) -> Result<Impulse, ImpulseError> {
    if let Some(path) = settings.file_for(rate) {
        return read(path)
            .map(|(info, channels)| Impulse {
                path: path.to_owned(),
                file_rate: info.rate,
                rate: info.rate,
                channels,
            })
            .and_then(|impulse| fit(impulse, rate));
    }
    let mut candidates: Vec<(PathBuf, u32)> = settings
        .files
        .iter()
        .map(|f| (f.path.clone(), f.rate_hz))
        .collect();
    if let Some(folder) = &settings.folder {
        let found = folder_files(folder)?;
        if found.is_empty() && settings.files.is_empty() {
            return Err(ImpulseError::EmptyFolder(folder.clone()));
        }
        for path in found {
            if let Ok(info) = read_info(&path) {
                if info.rate == rate {
                    return load(&path, rate);
                }
                candidates.push((path, info.rate));
            }
        }
    }
    let nearest = candidates
        .into_iter()
        .min_by_key(|(_, r)| (i64::from(*r) - i64::from(rate)).unsigned_abs())
        .ok_or(ImpulseError::NoneSet)?;
    load(&nearest.0, rate)
}

/// Reads `path` and brings it to `rate` Hz.
pub fn load(path: &Path, rate: u32) -> Result<Impulse, ImpulseError> {
    let (info, channels) = read(path)?;
    fit(
        Impulse {
            path: path.to_owned(),
            file_rate: info.rate,
            rate: info.rate,
            channels,
        },
        rate,
    )
}

/// The WAV files in `folder`, by name.
fn folder_files(folder: &Path) -> Result<Vec<PathBuf>, ImpulseError> {
    let entries = fs::read_dir(folder).map_err(|source| ImpulseError::Read {
        path: folder.to_owned(),
        source,
    })?;
    let mut files: Vec<PathBuf> = entries
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
    Ok(files)
}

/// Resamples `impulse` to `rate` Hz if it is at another rate.
fn fit(impulse: Impulse, rate: u32) -> Result<Impulse, ImpulseError> {
    if impulse.rate == rate {
        return Ok(impulse);
    }
    let failed = |reason: String| ImpulseError::Resample {
        path: impulse.path.clone(),
        reason,
    };
    let from = SampleRate::new(impulse.rate).ok_or_else(|| failed("no rate".into()))?;
    let to = SampleRate::new(rate).ok_or_else(|| failed("no rate".into()))?;
    let count = impulse.channels.len();
    let mut stage = ResampleStage::new(from, to, count, ResamplerQuality::High)
        .map_err(|e| failed(e.to_string()))?;
    let frames = impulse.taps();
    let mut interleaved = vec![0.0f32; frames * count];
    for (c, channel) in impulse.channels.iter().enumerate() {
        for (i, x) in channel.iter().enumerate() {
            interleaved[i * count + c] = *x as f32;
        }
    }
    let mut out = Vec::new();
    let mut buffer = vec![
        0.0f32;
        stage
            .max_output_frames(4096)
            .max(stage.max_output_frames(1))
            * count
    ];
    let mut at = 0;
    while at < frames {
        let end = (at + 4096).min(frames);
        let (taken, written) = stage.process(&interleaved[at * count..end * count], &mut buffer);
        out.extend_from_slice(&buffer[..written * count]);
        at += taken;
        if taken == 0 && written == 0 {
            break;
        }
    }
    loop {
        let written = stage.finish(&mut buffer);
        if written == 0 {
            break;
        }
        out.extend_from_slice(&buffer[..written * count]);
    }
    let mut channels = vec![Vec::with_capacity(out.len() / count); count];
    for frame in out.chunks_exact(count) {
        for (channel, x) in channels.iter_mut().zip(frame) {
            channel.push(f64::from(*x));
        }
    }
    Ok(Impulse {
        rate,
        channels,
        ..impulse
    })
}

/// The format of the WAV file at `path`, from its header.
pub fn read_info(path: &Path) -> Result<WavInfo, ImpulseError> {
    let bytes = fs::read(path).map_err(|source| ImpulseError::Read {
        path: path.to_owned(),
        source,
    })?;
    parse(&bytes)
        .map(|(info, _)| info)
        .map_err(|reason| ImpulseError::Format {
            path: path.to_owned(),
            reason,
        })
}

/// Reads the WAV file at `path`: its format and its samples per channel.
pub fn read(path: &Path) -> Result<(WavInfo, Vec<Vec<f64>>), ImpulseError> {
    let bytes = fs::read(path).map_err(|source| ImpulseError::Read {
        path: path.to_owned(),
        source,
    })?;
    let format = |reason: String| ImpulseError::Format {
        path: path.to_owned(),
        reason,
    };
    let (info, data) = parse(&bytes).map_err(format)?;
    decode(&info, data).map_err(format)
}

/// The sample encoding of a WAV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Int(u16),
    Float(u16),
}

/// What the header says, and where the samples are.
#[derive(Debug, Clone, Copy)]
struct Parsed<'a> {
    encoding: Encoding,
    data: &'a [u8],
}

fn parse(bytes: &[u8]) -> Result<(WavInfo, Parsed<'_>), String> {
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut at = 12;
    let mut fmt: Option<(u16, usize, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(bytes, at + 4) as usize;
        let body_start = at + 8;
        let body_end = body_start.saturating_add(size).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err("the format chunk is too short".into());
                }
                let mut tag = u16_at(body, 0);
                let channels = usize::from(u16_at(body, 2));
                let rate = u32_at(body, 4);
                let bits = u16_at(body, 14);
                if tag == 0xFFFE && body.len() >= 26 {
                    tag = u16_at(body, 24);
                }
                fmt = Some((tag, channels, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at = body_start.saturating_add(size + (size & 1));
    }
    let (tag, channels, rate, bits) = fmt.ok_or("there is no format chunk")?;
    let data = data.ok_or("there is no data chunk")?;
    let encoding = match (tag, bits) {
        (1, 8 | 16 | 24 | 32) => Encoding::Int(bits),
        (3, 32 | 64) => Encoding::Float(bits),
        _ => return Err(format!("format {tag} with {bits} bits is not supported")),
    };
    if !(1..=2).contains(&channels) {
        return Err(format!(
            "it has {channels} channels, but impulse responses may have one or two"
        ));
    }
    if rate == 0 {
        return Err("its sample rate is 0".into());
    }
    let bytes_per_frame = usize::from(bits / 8) * channels;
    let frames = data.len() / bytes_per_frame;
    if frames == 0 {
        return Err("it holds no samples".into());
    }
    if frames as f64 > MAX_SECONDS * f64::from(rate) {
        return Err(format!("it is longer than {MAX_SECONDS} seconds"));
    }
    let info = WavInfo {
        rate,
        channels,
        frames,
    };
    Ok((info, Parsed { encoding, data }))
}

fn decode(info: &WavInfo, parsed: Parsed<'_>) -> Result<(WavInfo, Vec<Vec<f64>>), String> {
    let count = info.channels;
    let mut channels = vec![Vec::with_capacity(info.frames); count];
    let width = match parsed.encoding {
        Encoding::Int(bits) | Encoding::Float(bits) => usize::from(bits / 8),
    };
    for (i, sample) in parsed
        .data
        .chunks_exact(width)
        .take(info.frames * count)
        .enumerate()
    {
        let value = match parsed.encoding {
            Encoding::Int(8) => (f64::from(sample[0]) - 128.0) / 128.0,
            Encoding::Int(16) => f64::from(i16::from_le_bytes([sample[0], sample[1]])) / 32_768.0,
            Encoding::Int(24) => {
                let v = i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8;
                f64::from(v) / 8_388_608.0
            }
            Encoding::Int(_) => {
                f64::from(i32::from_le_bytes([
                    sample[0], sample[1], sample[2], sample[3],
                ])) / 2_147_483_648.0
            }
            Encoding::Float(32) => f64::from(f32::from_le_bytes([
                sample[0], sample[1], sample[2], sample[3],
            ])),
            Encoding::Float(_) => f64::from_le_bytes([
                sample[0], sample[1], sample[2], sample[3], sample[4], sample[5], sample[6],
                sample[7],
            ]),
        };
        if !value.is_finite() {
            return Err("it holds samples that are not numbers".into());
        }
        channels[i % count].push(value);
    }
    Ok((*info, channels))
}

/// Writes `channels` (one vector per channel) as a 32-bit float WAV file, for
/// tests and tools.
pub fn write_wav(path: &Path, rate: u32, channels: &[Vec<f64>]) -> io::Result<()> {
    let count = channels.len().max(1);
    let frames = channels.first().map_or(0, Vec::len);
    let data_len = (frames * count * 4) as u32;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes());
    bytes.extend_from_slice(&(count as u16).to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * count as u32 * 4).to_le_bytes());
    bytes.extend_from_slice(&(count as u16 * 4).to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        for channel in channels {
            bytes.extend_from_slice(&(channel[i] as f32).to_le_bytes());
        }
    }
    fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helikodu_model::settings::ImpulseFile;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("helikodu-impulse-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn pcm16(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"LIST");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(b"abc\0");
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            b.extend_from_slice(&s.to_le_bytes());
        }
        b
    }

    #[test]
    fn integer_and_float_files_read_back() {
        let dir = scratch("read");
        let path = dir.join("a.wav");
        fs::write(&path, pcm16(44_100, &[16_384, -32_768, 0])).unwrap();
        let (info, channels) = read(&path).unwrap();
        assert_eq!((info.rate, info.channels, info.frames), (44_100, 1, 3));
        assert_eq!(channels[0], [0.5, -1.0, 0.0]);
        let float = dir.join("b.wav");
        write_wav(&float, 48_000, &[vec![1.0, 0.25], vec![-0.5, 0.0]]).unwrap();
        let (info, channels) = read(&float).unwrap();
        assert_eq!(info.channels, 2);
        assert_eq!(channels, [vec![1.0, 0.25], vec![-0.5, 0.0]]);
        fs::write(dir.join("c.wav"), b"RIFF0000WAVE").unwrap();
        assert!(matches!(
            read(&dir.join("c.wav")),
            Err(ImpulseError::Format { .. })
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_rate_chooses_the_file_and_others_are_resampled() {
        let dir = scratch("find");
        write_wav(&dir.join("room-44.wav"), 44_100, &[vec![1.0; 4]]).unwrap();
        write_wav(&dir.join("room-48.wav"), 48_000, &[vec![0.5; 4]]).unwrap();
        let mut settings = ConvolutionSettings {
            folder: Some(dir.clone()),
            ..ConvolutionSettings::default()
        };
        let found = find(&settings, 48_000).unwrap();
        assert!(found.path.ends_with("room-48.wav"));
        assert!(!found.resampled());
        assert_eq!(found.channels[0], [0.5; 4]);
        // 96 kHz has no file: the nearest (48 kHz) is resampled.
        let found = find(&settings, 96_000).unwrap();
        assert!(found.path.ends_with("room-48.wav"));
        assert!(found.resampled());
        assert_eq!(found.rate, 96_000);
        assert!((7..=9).contains(&found.taps()), "{}", found.taps());
        // An explicit file comes before the folder's.
        let explicit = dir.join("explicit.wav");
        write_wav(&explicit, 22_050, &[vec![0.25; 2]]).unwrap();
        settings.files = vec![ImpulseFile {
            rate_hz: 48_000,
            path: explicit.clone(),
        }];
        let found = find(&settings, 48_000).unwrap();
        assert_eq!(found.path, explicit);
        assert_eq!(found.rate, 48_000);
        assert!(matches!(
            find(&ConvolutionSettings::default(), 48_000),
            Err(ImpulseError::NoneSet)
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_peak_gain_is_the_largest_magnitude() {
        let impulse = Impulse {
            path: PathBuf::new(),
            file_rate: 48_000,
            rate: 48_000,
            channels: vec![vec![0.5, 0.5], vec![2.0]],
        };
        assert!((impulse.peak_gain() - 2.0).abs() < 1e-9);
    }
}
