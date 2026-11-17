// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The WAV backend: writes the audio, exactly as a device would receive it, to a
//! WAVE file. For tests and for checking the output path bit for bit.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use helikodu_model::audio::{
    AudioSpec, DeviceCapabilities, DeviceId, DeviceInfo, DeviceSelection, Endianness,
    OutputBackendKind, SampleFormat,
};

use super::software_sink::{Pacing, SinkTarget, SoftwareStream};
use super::{OpenedStream, OutputBackend, OutputError, StreamConfig};

/// The size of the header [`WavFile`] writes, up to the start of the samples.
const HEADER_LEN: u64 = 68;
/// Where the data chunk's size is stored.
const DATA_SIZE_OFFSET: u64 = 64;
/// `WAVE_FORMAT_EXTENSIBLE`.
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// The tail shared by the sub-format GUIDs of `KSDATAFORMAT_SUBTYPE_PCM` and
/// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`, after the two-byte format code.
const GUID_TAIL: [u8; 14] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

/// A backend with one device: a WAVE file.
///
/// The device's name is the file name. Opening [`DeviceSelection::SystemDefault`]
/// writes to the file the backend was created with; opening a named device writes
/// to the file of that name. The file is replaced when a stream opens, and its
/// header is completed when the stream stops.
///
/// Formats: unsigned 8-bit, signed 16-, 24- (packed or in 32-bit words) and
/// 32-bit, and 32- and 64-bit float, all little-endian as WAVE requires. Other
/// requests are answered with the nearest of these (DSD with 32-bit float).
/// Files are written as `WAVE_FORMAT_EXTENSIBLE` with the layout's channel mask.
/// Sizes in the header saturate above 4 GiB.
#[derive(Debug, Clone)]
pub struct WavBackend {
    path: PathBuf,
    pacing: Pacing,
}

impl WavBackend {
    /// A WAV backend writing to `path`, taking samples in real time like a device.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            pacing: Pacing::RealTime,
        }
    }

    /// The same backend with the given pacing. [`Pacing::Freewheel`] writes as fast
    /// as samples arrive and never inserts silence.
    #[must_use]
    pub fn with_pacing(self, pacing: Pacing) -> Self {
        Self { pacing, ..self }
    }

    /// The file written for [`DeviceSelection::SystemDefault`].
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The format a WAVE file can hold that is closest to `format`.
    fn negotiate(format: SampleFormat) -> SampleFormat {
        use Endianness::Little;
        match format {
            SampleFormat::U8 => SampleFormat::U8,
            SampleFormat::S16(_) => SampleFormat::S16(Little),
            SampleFormat::S24(_) => SampleFormat::S24(Little),
            SampleFormat::S24Packed(_) => SampleFormat::S24Packed(Little),
            SampleFormat::S32(_) => SampleFormat::S32(Little),
            SampleFormat::F64(_) => SampleFormat::F64(Little),
            SampleFormat::F32(_)
            | SampleFormat::DsdU8
            | SampleFormat::DsdU16(_)
            | SampleFormat::DsdU32(_) => SampleFormat::F32(Little),
        }
    }
}

impl OutputBackend for WavBackend {
    fn kind(&self) -> OutputBackendKind {
        OutputBackendKind::Wav
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        let name = self.path.to_string_lossy();
        let id = DeviceId::new(name.as_ref()).map_err(|_| OutputError::BackendUnavailable {
            backend: OutputBackendKind::Wav,
            reason: "no file name".into(),
        })?;
        let mut formats: Vec<SampleFormat> =
            SampleFormat::ALL.into_iter().map(Self::negotiate).collect();
        formats.dedup();
        Ok(vec![DeviceInfo {
            backend: OutputBackendKind::Wav,
            id,
            description: format!("WAV file {name}"),
            is_default: true,
            capabilities: DeviceCapabilities {
                sample_rates: Vec::new(),
                formats,
                channel_counts: Vec::new(),
            },
        }])
    }

    fn open(&self, config: &StreamConfig) -> Result<OpenedStream, OutputError> {
        let path = match &config.device {
            DeviceSelection::SystemDefault => self.path.clone(),
            DeviceSelection::Device(id) => PathBuf::from(id.as_str()),
        };
        let spec = AudioSpec {
            format: Self::negotiate(config.spec.format),
            ..config.spec
        };
        let file = WavFile::create(&path, spec)?;
        SoftwareStream::open(OutputBackendKind::Wav, config, spec, self.pacing, file)
    }
}

/// A WAVE file being written.
struct WavFile {
    out: BufWriter<File>,
    data_len: u64,
}

impl WavFile {
    fn create(path: &Path, spec: AudioSpec) -> io::Result<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&header(spec, 0))?;
        Ok(Self { out, data_len: 0 })
    }
}

impl SinkTarget for WavFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.data_len += bytes.len() as u64;
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        // Chunks have an even length.
        if self.data_len % 2 == 1 {
            self.out.write_all(&[0])?;
        }
        let data_len = saturate(self.data_len);
        let riff_len = saturate(HEADER_LEN - 8 + self.data_len.next_multiple_of(2));
        self.out.seek(SeekFrom::Start(4))?;
        self.out.write_all(&riff_len.to_le_bytes())?;
        self.out.seek(SeekFrom::Start(DATA_SIZE_OFFSET))?;
        self.out.write_all(&data_len.to_le_bytes())?;
        self.out.flush()?;
        self.out.get_ref().sync_all()
    }
}

fn saturate(len: u64) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// The 68-byte header of a `WAVE_FORMAT_EXTENSIBLE` file with `data_len` bytes of
/// samples.
fn header(spec: AudioSpec, data_len: u32) -> Vec<u8> {
    let channels = spec.channel_count();
    let container_bits = (spec.format.bytes() * 8) as u16;
    let valid_bits = spec.format.bits() as u16;
    let block_align = spec.bytes_per_frame() as u16;
    let byte_rate = spec.rate.hz().saturating_mul(u32::from(block_align));
    let format_code: u16 = if spec.format.is_float() { 3 } else { 1 };

    let mut h = Vec::with_capacity(HEADER_LEN as usize);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&((HEADER_LEN - 8) as u32 + data_len).to_le_bytes());
    h.extend_from_slice(b"WAVE");
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&40u32.to_le_bytes());
    h.extend_from_slice(&FORMAT_EXTENSIBLE.to_le_bytes());
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&spec.rate.hz().to_le_bytes());
    h.extend_from_slice(&byte_rate.to_le_bytes());
    h.extend_from_slice(&block_align.to_le_bytes());
    h.extend_from_slice(&container_bits.to_le_bytes());
    h.extend_from_slice(&22u16.to_le_bytes());
    h.extend_from_slice(&valid_bits.to_le_bytes());
    h.extend_from_slice(&spec.layout.mask().unwrap_or(0).to_le_bytes());
    h.extend_from_slice(&format_code.to_le_bytes());
    h.extend_from_slice(&GUID_TAIL);
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len.to_le_bytes());
    debug_assert_eq!(h.len() as u64, HEADER_LEN);
    h
}

#[cfg(test)]
mod tests {
    use helikodu_model::audio::{ChannelLayout, SampleRate};

    use super::*;

    #[test]
    fn header_layout() {
        let spec = AudioSpec::new(
            SampleFormat::S24(Endianness::Little),
            SampleRate::HZ_44100,
            ChannelLayout::SURROUND_5_1,
        );
        let h = header(spec, 1000);
        assert_eq!(h.len(), 68);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 1060);
        assert_eq!(u16::from_le_bytes([h[20], h[21]]), 0xFFFE);
        assert_eq!(u16::from_le_bytes([h[22], h[23]]), 6);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 44_100);
        assert_eq!(
            u32::from_le_bytes(h[28..32].try_into().unwrap()),
            44_100 * 24
        );
        assert_eq!(u16::from_le_bytes([h[32], h[33]]), 24);
        // A 32-bit container with 24 valid bits.
        assert_eq!(u16::from_le_bytes([h[34], h[35]]), 32);
        assert_eq!(u16::from_le_bytes([h[38], h[39]]), 24);
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 0x3F);
        assert_eq!(&h[44..46], &[1, 0]);
        assert_eq!(&h[60..64], b"data");
        assert_eq!(u32::from_le_bytes(h[64..68].try_into().unwrap()), 1000);
    }

    #[test]
    fn formats_become_little_endian_pcm() {
        use Endianness::{Big, Little};
        assert_eq!(
            WavBackend::negotiate(SampleFormat::S16(Big)),
            SampleFormat::S16(Little)
        );
        assert_eq!(
            WavBackend::negotiate(SampleFormat::DsdU32(Big)),
            SampleFormat::F32(Little)
        );
        assert_eq!(WavBackend::negotiate(SampleFormat::U8), SampleFormat::U8);
    }
}
