// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The null and WAV sinks, end to end through the public output API.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use helikodu_engine::output::{
    NullBackend, OpenedStream, OutputBackend, OutputError, Pacing, SampleWriter, StreamConfig,
    StreamState, WavBackend, connect_backend,
};
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, DeviceId, DeviceSelection, Endianness, OutputBackendKind,
    SampleFormat, SampleRate,
};

const LITTLE: Endianness = Endianness::Little;

/// A file in the system's temporary directory, removed when dropped.
struct TempFile(PathBuf);

impl TempFile {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "helikodu-engine-test-{}-{name}.wav",
            std::process::id()
        ));
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A small deterministic generator, so the signal covers many values.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }
}

/// `frames` frames of integer samples of `bits` bits, including both extremes.
fn integer_signal(bits: u32, frames: usize, channels: usize) -> Vec<i32> {
    let max = (1i64 << (bits - 1)) - 1;
    let min = -(1i64 << (bits - 1));
    let mut lcg = Lcg(bits.into());
    let mut signal: Vec<i32> = (0..frames * channels)
        .map(|_| (i64::from(lcg.next()) % (max - min + 1) + min) as i32)
        .collect();
    signal[0] = min as i32;
    signal[1] = max as i32;
    signal[2] = 0;
    signal
}

fn to_float(signal: &[i32], bits: u32) -> Vec<f32> {
    let scale = (1u64 << (bits - 1)) as f32;
    signal.iter().map(|&v| v as f32 / scale).collect()
}

/// Writes every sample, waiting while the ring buffer is full.
fn feed(writer: &mut SampleWriter, samples: &[f32]) {
    let channels = writer.channels();
    let mut offset = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while offset < samples.len() {
        let end = (offset + 4096 * channels).min(samples.len());
        let frames = writer.write(&samples[offset..end]);
        offset += frames * channels;
        if frames == 0 {
            assert!(Instant::now() < deadline, "the sink stopped taking samples");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Plays `samples` through a freewheeling WAV sink and returns the file's header
/// and data.
fn play_to_wav(spec: AudioSpec, samples: &[f32]) -> (Wav, AudioSpec) {
    let file = TempFile::new(&format!("{}-{}", spec.format, spec.channel_count()));
    let backend = WavBackend::new(&file.0).with_pacing(Pacing::Freewheel);
    let OpenedStream {
        mut stream,
        mut writer,
    } = backend.open(&StreamConfig::new(spec)).unwrap();
    let negotiated = stream.spec();
    stream.start().unwrap();
    feed(&mut writer, samples);
    stream.drain(Duration::from_secs(10)).unwrap();
    let timing = stream.timing();
    assert_eq!(
        timing.frames_delivered as usize,
        samples.len() / usize::from(spec.channel_count())
    );
    assert_eq!(timing.underruns, 0);
    stream.stop().unwrap();
    assert_eq!(stream.state(), StreamState::Closed);
    (Wav::read(&file.0), negotiated)
}

/// The fields of a WAVE file this test checks.
#[derive(Debug)]
struct Wav {
    format_tag: u16,
    channels: u16,
    rate: u32,
    block_align: u16,
    container_bits: u16,
    valid_bits: u16,
    channel_mask: u32,
    sub_format: u16,
    riff_len: u32,
    data: Vec<u8>,
}

impl Wav {
    fn read(path: &Path) -> Self {
        let bytes = std::fs::read(path).unwrap();
        let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(u32_at(16), 40);
        assert_eq!(&bytes[60..64], b"data");
        let data_len = u32_at(64) as usize;
        assert_eq!(bytes.len(), 68 + data_len.next_multiple_of(2));
        Self {
            format_tag: u16_at(20),
            channels: u16_at(22),
            rate: u32_at(24),
            block_align: u16_at(32),
            container_bits: u16_at(34),
            valid_bits: u16_at(38),
            channel_mask: u32_at(40),
            sub_format: u16_at(44),
            riff_len: u32_at(4),
            data: bytes[68..68 + data_len].to_vec(),
        }
    }
}

#[test]
fn wav_s16_is_bit_exact() {
    let spec = AudioSpec::new(
        SampleFormat::S16(LITTLE),
        SampleRate::HZ_44100,
        ChannelLayout::STEREO,
    );
    let signal = integer_signal(16, 30_000, 2);
    let (wav, negotiated) = play_to_wav(spec, &to_float(&signal, 16));
    assert_eq!(negotiated, spec);
    let expected: Vec<u8> = signal
        .iter()
        .flat_map(|&v| (v as i16).to_le_bytes())
        .collect();
    assert_eq!(wav.data, expected);
    assert_eq!(
        (wav.format_tag, wav.channels, wav.rate, wav.block_align),
        (0xFFFE, 2, 44_100, 4)
    );
    assert_eq!(
        (wav.container_bits, wav.valid_bits, wav.sub_format),
        (16, 16, 1)
    );
    assert_eq!(wav.channel_mask, 0b11);
    assert_eq!(wav.riff_len as usize, 60 + expected.len());
}

#[test]
fn wav_s24_packed_is_bit_exact() {
    let spec = AudioSpec::new(
        SampleFormat::S24Packed(LITTLE),
        SampleRate::new(96_000).unwrap(),
        ChannelLayout::SURROUND_5_1,
    );
    // An odd number of 3-byte samples: the data chunk needs a pad byte.
    let signal = integer_signal(24, 10_001, 6);
    let (wav, _) = play_to_wav(spec, &to_float(&signal, 24));
    let expected: Vec<u8> = signal
        .iter()
        .flat_map(|&v| {
            let b = v.to_le_bytes();
            [b[0], b[1], b[2]]
        })
        .collect();
    assert_eq!(wav.data.len() % 2, 0);
    assert_eq!(wav.data, expected);
    assert_eq!((wav.channels, wav.block_align), (6, 18));
    assert_eq!((wav.container_bits, wav.valid_bits), (24, 24));
    assert_eq!(wav.channel_mask, 0x3F);
}

#[test]
fn wav_s24_in_32_bit_words_is_bit_exact() {
    let spec = AudioSpec::new(
        SampleFormat::S24(Endianness::Big),
        SampleRate::HZ_48000,
        ChannelLayout::MONO,
    );
    let signal = integer_signal(24, 5_000, 1);
    let (wav, negotiated) = play_to_wav(spec, &to_float(&signal, 24));
    // WAVE is little-endian.
    assert_eq!(negotiated.format, SampleFormat::S24(LITTLE));
    let expected: Vec<u8> = signal.iter().flat_map(|&v| v.to_le_bytes()).collect();
    assert_eq!(wav.data, expected);
    assert_eq!((wav.container_bits, wav.valid_bits), (32, 24));
    assert_eq!(wav.channel_mask, 0x4);
}

#[test]
fn wav_s32_keeps_24_bits_exactly() {
    // The ring buffer carries f32, which holds 24 significant bits; 32-bit
    // samples with 8 zero low bits come through unchanged.
    let spec = AudioSpec::new(
        SampleFormat::S32(LITTLE),
        SampleRate::HZ_48000,
        ChannelLayout::STEREO,
    );
    let signal: Vec<i32> = integer_signal(24, 4_000, 2)
        .iter()
        .map(|v| v << 8)
        .collect();
    let (wav, _) = play_to_wav(spec, &to_float(&signal, 32));
    let expected: Vec<u8> = signal.iter().flat_map(|&v| v.to_le_bytes()).collect();
    assert_eq!(wav.data, expected);
}

#[test]
fn wav_f32_is_bit_exact() {
    let spec = AudioSpec::new(
        SampleFormat::F32(LITTLE),
        SampleRate::HZ_48000,
        ChannelLayout::STEREO,
    );
    let mut lcg = Lcg(7);
    let samples: Vec<f32> = (0..20_000)
        .map(|_| (lcg.next() as f32 / u32::MAX as f32) * 2.0 - 1.0)
        .collect();
    let (wav, _) = play_to_wav(spec, &samples);
    let expected: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
    assert_eq!(wav.data, expected);
    assert_eq!(
        (wav.format_tag, wav.sub_format, wav.valid_bits),
        (0xFFFE, 3, 32)
    );
}

#[test]
fn wav_u8_is_bit_exact() {
    let spec = AudioSpec::new(
        SampleFormat::U8,
        SampleRate::new(8000).unwrap(),
        ChannelLayout::MONO,
    );
    let signal = integer_signal(8, 999, 1);
    let (wav, _) = play_to_wav(spec, &to_float(&signal, 8));
    let expected: Vec<u8> = signal.iter().map(|&v| (v + 128) as u8).collect();
    assert_eq!(wav.data, expected);
}

#[test]
fn wav_real_time_pacing_takes_real_time() {
    let file = TempFile::new("paced");
    let spec = AudioSpec::new(
        SampleFormat::S16(LITTLE),
        SampleRate::HZ_48000,
        ChannelLayout::STEREO,
    );
    let backend = WavBackend::new(&file.0);
    let OpenedStream {
        mut stream,
        mut writer,
    } = backend.open(&StreamConfig::new(spec)).unwrap();
    // 300 ms of audio, more than the 200 ms ring buffer holds.
    let samples = vec![0.25f32; 14_400 * 2];
    let filled = writer.write(&samples);
    assert_eq!(filled, writer.capacity_frames());
    let started = Instant::now();
    stream.start().unwrap();
    feed(&mut writer, &samples[filled * 2..]);
    stream.drain(Duration::from_secs(5)).unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(280), "{elapsed:?}");
    assert_eq!(stream.timing().underruns, 0);
    stream.stop().unwrap();
    let wav = Wav::read(&file.0);
    // A slow machine may let the paced sink write a little silence while it
    // waits for the feeder; every frame of the signal must still be there,
    // unchanged and in one piece, and anything else must be silence.
    let frames: Vec<&[u8]> = wav.data.chunks(4).collect();
    let signal = 8192i16.to_le_bytes();
    let is_signal = |f: &[u8]| f[..2] == signal && f[2..] == signal;
    assert_eq!(frames.iter().filter(|f| is_signal(f)).count(), 14_400);
    assert!(
        frames
            .iter()
            .all(|f| is_signal(f) || f.iter().all(|&b| b == 0))
    );
}

#[test]
fn wav_writes_to_a_named_device_file() {
    let default = TempFile::new("default");
    let named = TempFile::new("named");
    let backend = WavBackend::new(&default.0).with_pacing(Pacing::Freewheel);
    let devices = backend.devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id.as_str(), default.0.to_string_lossy());
    let spec = AudioSpec::new(
        SampleFormat::S16(LITTLE),
        SampleRate::HZ_44100,
        ChannelLayout::MONO,
    );
    let config = StreamConfig {
        device: DeviceSelection::Device(DeviceId::new(named.0.to_string_lossy()).unwrap()),
        ..StreamConfig::new(spec)
    };
    let mut opened = backend.open(&config).unwrap();
    opened.stream.start().unwrap();
    feed(&mut opened.writer, &[0.5; 100]);
    opened.stream.drain(Duration::from_secs(5)).unwrap();
    opened.stream.stop().unwrap();
    assert!(!default.0.exists());
    assert_eq!(Wav::read(&named.0).data.len(), 200);
}

#[test]
fn wav_to_an_unwritable_path_fails_to_open() {
    let backend = WavBackend::new("/nonexistent-directory/helikodu.wav");
    let spec = AudioSpec::new(
        SampleFormat::S16(LITTLE),
        SampleRate::HZ_44100,
        ChannelLayout::MONO,
    );
    assert!(matches!(
        backend.open(&StreamConfig::new(spec)),
        Err(OutputError::Io(_))
    ));
}

#[test]
fn null_sink_runs_in_real_time_and_pauses() {
    let backend = NullBackend::new();
    let devices = backend.devices().unwrap();
    assert_eq!(devices[0].id.as_str(), "null");
    assert!(devices[0].is_default);
    let spec = AudioSpec::new(
        SampleFormat::S24Packed(LITTLE),
        SampleRate::HZ_48000,
        ChannelLayout::STEREO,
    );
    let OpenedStream {
        mut stream,
        mut writer,
    } = backend.open(&StreamConfig::new(spec)).unwrap();
    assert_eq!(stream.spec(), spec);
    assert_eq!(stream.backend(), OutputBackendKind::Null);
    assert_eq!(stream.state(), StreamState::Ready);
    assert!(!stream.has_volume());
    assert!(matches!(
        stream.set_volume(0.5),
        Err(OutputError::Unsupported(_))
    ));

    writer.write(&vec![0.1; writer.capacity_frames() * 2]);
    stream.start().unwrap();
    assert_eq!(stream.state(), StreamState::Running);
    std::thread::sleep(Duration::from_millis(100));
    stream.pause().unwrap();
    assert_eq!(stream.state(), StreamState::Paused);
    std::thread::sleep(Duration::from_millis(30));
    let paused = stream.timing();
    // About 100 ms at 48 kHz, in 10 ms periods.
    assert!(
        (2_400..=9_600).contains(&paused.frames_delivered),
        "{paused:?}"
    );
    assert_eq!(paused.underruns, 0);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(stream.timing().frames_delivered, paused.frames_delivered);
    assert_eq!(
        paused.played(),
        helikodu_model::audio::MediaTime::from_frames(
            paused.frames_delivered,
            SampleRate::HZ_48000
        )
    );

    stream.start().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let timing = stream.timing();
    // The ring buffer (200 ms) ran dry: the sink played silence and counted it.
    assert!(timing.underruns > 0, "{timing:?}");
    assert!(timing.silent_frames > 0);
    stream.stop().unwrap();
    assert!(writer.is_closed());
    assert!(matches!(stream.start(), Err(OutputError::Closed)));
}

#[test]
fn null_sink_refuses_other_devices_and_answers_dsd_with_float() {
    let backend = NullBackend::with_pacing(Pacing::Freewheel);
    let spec = AudioSpec::new(
        SampleFormat::DsdU32(Endianness::Big),
        SampleRate::new(2_822_400).unwrap(),
        ChannelLayout::STEREO,
    );
    let opened = backend.open(&StreamConfig::new(spec)).unwrap();
    assert_eq!(
        opened.stream.spec().format,
        SampleFormat::F32(Endianness::NATIVE)
    );
    drop(opened);
    let config = StreamConfig {
        device: DeviceSelection::Device(DeviceId::new("speakers").unwrap()),
        ..StreamConfig::new(spec)
    };
    assert!(matches!(
        backend.open(&config),
        Err(OutputError::DeviceNotFound { .. })
    ));
}

#[test]
fn dropping_a_stream_stops_it() {
    let backend = NullBackend::new();
    let spec = AudioSpec::new(
        SampleFormat::S16(LITTLE),
        SampleRate::HZ_44100,
        ChannelLayout::STEREO,
    );
    let OpenedStream { mut stream, writer } = backend.open(&StreamConfig::new(spec)).unwrap();
    stream.start().unwrap();
    drop(stream);
    assert!(writer.is_closed());
}

#[test]
fn backends_by_kind() {
    let (events, _received) = mpsc::channel();
    let null = connect_backend(OutputBackendKind::Null, &events).unwrap();
    assert_eq!(null.kind(), OutputBackendKind::Null);
    for kind in [
        OutputBackendKind::Wav,
        OutputBackendKind::PulseAudio,
        OutputBackendKind::Jack,
    ] {
        assert!(matches!(
            connect_backend(kind, &events),
            Err(OutputError::BackendUnavailable { .. })
        ));
    }
}
