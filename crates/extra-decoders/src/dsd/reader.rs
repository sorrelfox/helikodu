// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The raw DSD stream of a DSF or DFF file, in one canonical form.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

use helikodu_model::audio::ChannelLayout;

use super::error::DsdError;
use super::{dff, dsf};

/// The DSD idle pattern: four ones and four zeros, which averages to silence.
/// The reader's users pad with it before the start and after the end of a stream.
pub const IDLE_BYTE: u8 = 0x69;

/// The most bytes per channel read from the file at once.
const MAX_READ: usize = 64 * 1024;

/// The container of a DSD file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DsdContainer {
    /// Sony's DSD Stream File (`.dsf`).
    Dsf,
    /// Philips' DSD Interchange File Format, DSDIFF (`.dff`).
    Dff,
}

impl DsdContainer {
    /// The usual short name: `dsf` or `dff`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dsf => "dsf",
            Self::Dff => "dff",
        }
    }
}

/// What a DSD file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DsdInfo {
    /// The container.
    pub container: DsdContainer,
    /// The DSD sample rate in Hz (bits per second per channel): 2 822 400 for DSD64.
    pub dsd_rate: u32,
    /// The channels, in WAVE order. The reader reorders DFF channels to it.
    pub layout: ChannelLayout,
    /// The length of the stream in DSD samples (bits) per channel.
    pub samples: u64,
}

impl DsdInfo {
    /// The number of channels.
    pub fn channels(&self) -> usize {
        usize::from(self.layout.channel_count())
    }
}

/// How the sound data is laid out in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Storage {
    /// DSF: per-channel blocks of `block_size` bytes, one block of each channel in
    /// turn. Bytes hold their earliest bit in the least significant position when
    /// `lsb_first`.
    Blocks { block_size: u64, lsb_first: bool },
    /// DFF: one byte of each channel in turn, earliest bit most significant.
    Interleaved,
}

/// The result of parsing a container.
#[derive(Debug)]
pub(super) struct Parsed {
    pub(super) info: DsdInfo,
    /// Where the sound data starts in the file.
    pub(super) data_start: u64,
    /// How many bytes of sound data the file holds (after `data_start`).
    pub(super) data_len: u64,
    pub(super) storage: Storage,
    /// For each output channel (WAVE order), the channel's index in the file.
    pub(super) order: Vec<usize>,
}

/// Reads the raw DSD stream of a DSF or DFF file.
///
/// [`read`](Self::read) yields, per channel, bytes in time order with the earliest
/// bit most significant, channels in WAVE order.
#[derive(Debug)]
pub struct DsdReader {
    file: File,
    parsed: Parsed,
    /// The stream length in bytes per channel.
    bytes_per_channel: u64,
    /// The next byte to read, per channel.
    position: u64,
    scratch: Vec<u8>,
    /// For DSF: the block group held in `scratch`.
    cached_group: Option<u64>,
}

impl DsdReader {
    /// Opens the file at `path` and parses its header.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DsdError> {
        Self::from_file(File::open(path)?)
    }

    /// Parses the header of an already open file.
    pub fn from_file(file: File) -> Result<Self, DsdError> {
        let len = file.metadata()?.len();
        let mut magic = [0u8; 16];
        let got = read_up_to(&file, &mut magic, 0)?;
        let parsed = match super::sniff(&magic[..got]) {
            Some(DsdContainer::Dsf) => dsf::parse(&file, len)?,
            Some(DsdContainer::Dff) => dff::parse(&file, len)?,
            None => return Err(DsdError::NotDsd),
        };
        let channels = parsed.info.channels() as u64;
        let declared = parsed.info.samples.div_ceil(8);
        let available = match parsed.storage {
            Storage::Interleaved => parsed.data_len / channels,
            Storage::Blocks { block_size, .. } => {
                let group = block_size * channels;
                let rest = parsed.data_len % group;
                let partial = rest
                    .saturating_sub((channels - 1) * block_size)
                    .min(block_size);
                parsed.data_len / group * block_size + partial
            }
        };
        let bytes_per_channel = declared.min(available);
        let mut parsed = parsed;
        parsed.info.samples = parsed.info.samples.min(bytes_per_channel * 8);
        Ok(Self {
            file,
            parsed,
            bytes_per_channel,
            position: 0,
            scratch: Vec::new(),
            cached_group: None,
        })
    }

    /// What the file holds. When the file is shorter than its header says,
    /// [`samples`](DsdInfo::samples) is the length actually present.
    pub fn info(&self) -> &DsdInfo {
        &self.parsed.info
    }

    /// The stream length in bytes per channel.
    pub fn bytes_per_channel(&self) -> u64 {
        self.bytes_per_channel
    }

    /// The next byte [`read`](Self::read) returns, per channel.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Moves to byte `position` of each channel (clamped to the end).
    pub fn seek(&mut self, position: u64) {
        self.position = position.min(self.bytes_per_channel);
    }

    /// Appends up to `max` bytes of each channel to `out`, one vector per channel.
    /// Returns how many bytes per channel were appended: 0 at the end.
    ///
    /// # Panics
    /// When `out` does not have one vector per channel.
    pub fn read(&mut self, max: usize, out: &mut [Vec<u8>]) -> Result<usize, DsdError> {
        assert_eq!(
            out.len(),
            self.parsed.info.channels(),
            "one vector per channel"
        );
        let left = self.bytes_per_channel - self.position;
        let n = usize::try_from(left).unwrap_or(usize::MAX).min(max);
        let mut done = 0;
        while done < n {
            let step = match self.parsed.storage {
                Storage::Interleaved => self.read_interleaved(n - done, out)?,
                Storage::Blocks {
                    block_size,
                    lsb_first,
                } => self.read_blocks(block_size, lsb_first, n - done, out)?,
            };
            done += step;
            self.position += step as u64;
        }
        Ok(n)
    }

    fn read_interleaved(&mut self, max: usize, out: &mut [Vec<u8>]) -> Result<usize, DsdError> {
        let channels = self.parsed.info.channels();
        let n = max.min(MAX_READ);
        self.scratch.resize(n * channels, 0);
        let at = self.parsed.data_start + self.position * channels as u64;
        self.file.read_exact_at(&mut self.scratch, at)?;
        for (out, &from) in out.iter_mut().zip(&self.parsed.order) {
            out.extend(self.scratch.iter().skip(from).step_by(channels));
        }
        Ok(n)
    }

    fn read_blocks(
        &mut self,
        block_size: u64,
        lsb_first: bool,
        max: usize,
        out: &mut [Vec<u8>],
    ) -> Result<usize, DsdError> {
        let group = self.position / block_size;
        let within = self.position % block_size;
        if self.cached_group != Some(group) {
            let group_len = block_size * self.parsed.info.channels() as u64;
            let start = group * group_len;
            let present = self.parsed.data_len.saturating_sub(start).min(group_len);
            let present =
                usize::try_from(present).map_err(|_| DsdError::malformed("block size"))?;
            self.scratch.clear();
            self.scratch.resize(present, 0);
            self.file
                .read_exact_at(&mut self.scratch, self.parsed.data_start + start)?;
            // Bytes missing from a truncated last group never get read: the stream
            // length was cut to what is present.
            self.scratch
                .resize(usize::try_from(group_len).unwrap_or(0), 0);
            self.cached_group = Some(group);
        }
        // Both are below the block size, which came from a u32.
        let within = usize::try_from(within).unwrap_or(0);
        let block = usize::try_from(block_size).unwrap_or(0);
        let n = max.min(block - within);
        for (out, &from) in out.iter_mut().zip(&self.parsed.order) {
            let bytes = &self.scratch[from * block + within..from * block + within + n];
            if lsb_first {
                out.extend(bytes.iter().map(|b| b.reverse_bits()));
            } else {
                out.extend_from_slice(bytes);
            }
        }
        Ok(n)
    }
}

/// Reads as many bytes as the file has at `at`, up to the length of `buf`.
pub(super) fn read_up_to(file: &File, buf: &mut [u8], at: u64) -> Result<usize, DsdError> {
    let mut got = 0;
    while got < buf.len() {
        match file.read_at(&mut buf[got..], at + got as u64) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(got)
}

/// Reads exactly `N` bytes at `at`; a short file is a damaged one.
pub(super) fn read_array<const N: usize>(
    file: &File,
    at: u64,
    what: &str,
) -> Result<[u8; N], DsdError> {
    let mut buf = [0u8; N];
    if read_up_to(file, &mut buf, at)? < N {
        return Err(DsdError::malformed(format!(
            "the file ends inside the {what}"
        )));
    }
    Ok(buf)
}
