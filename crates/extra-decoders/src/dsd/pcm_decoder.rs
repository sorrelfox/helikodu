// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A DSD file decoded to PCM.

use std::path::Path;

use helikodu_model::audio::{AudioSpec, Endianness, SampleFormat, SampleRate};

use super::decimator::Decimator;
use super::error::DsdError;
use super::reader::{DsdInfo, DsdReader, IDLE_BYTE};

/// The most PCM frames produced by one call to [`DsdPcmDecoder::read`].
const MAX_BLOCK_FRAMES: usize = 4096;

/// Decodes a DSF or DFF file to interleaved `f32` PCM.
///
/// The PCM rate is the DSD rate divided by a power of two (at least 16), the first
/// that is at most 192 kHz: 176.4 kHz for DSD64 to DSD512, 192 kHz for the 48 kHz
/// family. A stream of `n` DSD bits per channel gives `n / factor` frames (whole
/// frames; a shorter tail is dropped). The filter of frame `i` is centred on DSD bit
/// `i × factor`, the same instant, so the output has no filter delay; before the
/// start and after the end the stream is extended with the DSD idle pattern.
/// Samples carry the +6 dB of [`DSD_GAIN`](super::DSD_GAIN).
#[derive(Debug)]
pub struct DsdPcmDecoder {
    reader: DsdReader,
    decimator: Decimator,
    spec: AudioSpec,
    /// The stream length in PCM frames.
    frames: u64,
    /// The next frame [`read`](Self::read) returns.
    position: u64,
    /// Per channel, DSD bytes from `window_start` on.
    window: Vec<Vec<u8>>,
    /// The stream index of `window[_][0]`: the start of the filter window of
    /// `position`. Negative before the start of the stream.
    window_start: i64,
}

impl DsdPcmDecoder {
    /// Opens the DSF or DFF file at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DsdError> {
        Self::new(DsdReader::open(path)?)
    }

    /// Decodes the stream of `reader`, from its start.
    pub fn new(mut reader: DsdReader) -> Result<Self, DsdError> {
        let decimator = Decimator::new(reader.info().dsd_rate)?;
        let rate = SampleRate::new(decimator.output_rate())
            .ok_or_else(|| DsdError::malformed("no sample rate"))?;
        let spec = AudioSpec::new(
            SampleFormat::F32(Endianness::NATIVE),
            rate,
            reader.info().layout,
        );
        let frames = reader.info().samples / u64::from(decimator.factor());
        let channels = reader.info().channels();
        reader.seek(0);
        let window_start = decimator.window_start(0);
        Ok(Self {
            reader,
            decimator,
            spec,
            frames,
            position: 0,
            window: vec![Vec::new(); channels],
            window_start,
        })
    }

    /// What the file holds.
    pub fn info(&self) -> &DsdInfo {
        self.reader.info()
    }

    /// The format of the PCM output: native-endian `f32` at the PCM rate.
    pub fn spec(&self) -> AudioSpec {
        self.spec
    }

    /// The stream length in PCM frames.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The next frame [`read`](Self::read) returns.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Moves to PCM frame `frame` (clamped to the end).
    pub fn seek(&mut self, frame: u64) {
        self.position = frame.min(self.frames);
        self.window_start = self.decimator.window_start(self.position);
        for w in &mut self.window {
            w.clear();
        }
        self.reader
            .seek(u64::try_from(self.window_start).unwrap_or(0));
    }

    /// Replaces the contents of `out` with up to `max_frames` interleaved frames
    /// (at most 4096 at a time). Returns the number of frames: 0 at the end.
    pub fn read(&mut self, max_frames: usize, out: &mut Vec<f32>) -> Result<usize, DsdError> {
        out.clear();
        let left = usize::try_from(self.frames - self.position).unwrap_or(usize::MAX);
        let n = max_frames.min(MAX_BLOCK_FRAMES).min(left);
        if n == 0 {
            return Ok(0);
        }
        let step = self.decimator.step();
        let needed = (n - 1) * step + self.decimator.window();
        self.fill(needed)?;
        let channels = self.window.len();
        out.reserve(n * channels);
        for i in 0..n {
            for w in &self.window {
                #[allow(clippy::cast_possible_truncation)]
                out.push(self.decimator.sample(&w[i * step..]) as f32);
            }
        }
        for w in &mut self.window {
            w.drain(..n * step);
        }
        self.window_start += (n * step) as i64;
        self.position += n as u64;
        Ok(n)
    }

    /// Makes the window at least `len` bytes long.
    fn fill(&mut self, len: usize) -> Result<(), DsdError> {
        let total = i64::try_from(self.reader.bytes_per_channel()).unwrap_or(i64::MAX);
        loop {
            let have = self.window[0].len();
            if have >= len {
                return Ok(());
            }
            let missing = len - have;
            let end = self.window_start + have as i64;
            if end < 0 || end >= total {
                // Before the start or after the end: the idle pattern.
                let count = if end < 0 {
                    missing.min(usize::try_from(-end).unwrap_or(usize::MAX))
                } else {
                    missing
                };
                for w in &mut self.window {
                    w.resize(w.len() + count, IDLE_BYTE);
                }
            } else if self.reader.read(missing, &mut self.window)? == 0 {
                // The reader ended early; never expected, but do not spin.
                for w in &mut self.window {
                    w.resize(len, IDLE_BYTE);
                }
            }
        }
    }
}
