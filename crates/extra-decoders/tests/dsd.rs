// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! DSF and DFF files written by the test from a sigma-delta modulated sine, read
//! back and decoded to PCM.

#![cfg(feature = "dsd")]
#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};

use helikodu_extra_decoders::dsd::{DsdContainer, DsdError, DsdPcmDecoder, DsdReader, sniff};
use helikodu_model::audio::ChannelLayout;

const DSD64: u32 = 2_822_400;
/// A quarter of a second plus one byte: not a whole number of PCM frames.
const BITS: u64 = 705_608;

/// One test channel: a sine of `frequency` Hz with peak `amplitude` relative to
/// full modulation (0.5 is the DSD 0 dB reference).
#[derive(Clone, Copy)]
struct Tone {
    frequency: f64,
    amplitude: f64,
}

const LEFT: Tone = Tone {
    frequency: 1000.0,
    amplitude: 0.25,
};
const RIGHT: Tone = Tone {
    frequency: 2500.0,
    amplitude: 0.125,
};

/// A second-order sigma-delta modulator: the bits of `tone`, earliest bit most
/// significant in each byte.
fn modulate(tone: Tone, rate: u32, bits: u64) -> Vec<u8> {
    let (mut i1, mut i2) = (0.0f64, 0.0f64);
    let mut out = vec![0u8; bits.div_ceil(8) as usize];
    for n in 0..bits {
        let x = tone.amplitude * (TAU * tone.frequency * n as f64 / f64::from(rate)).sin();
        let y = if i2 >= 0.0 { 1.0 } else { -1.0 };
        if y > 0.0 {
            out[(n / 8) as usize] |= 0x80 >> (n % 8);
        }
        i1 += x - y;
        i2 += i1 - y;
    }
    out
}

fn dir() -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("extra-decoders-dsd");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A DSF file: little-endian, 4096-byte blocks per channel, LSB first, zero padding.
fn dsf_bytes(channels: &[Vec<u8>], rate: u32, bits: u64) -> Vec<u8> {
    const BLOCK: usize = 4096;
    let bytes = channels[0].len();
    let groups = bytes.div_ceil(BLOCK);
    let data_len = groups * BLOCK * channels.len();
    let mut b = Vec::new();
    b.extend_from_slice(b"DSD ");
    b.extend_from_slice(&28u64.to_le_bytes());
    b.extend_from_slice(&((28 + 52 + 12 + data_len) as u64).to_le_bytes());
    b.extend_from_slice(&0u64.to_le_bytes());
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&52u64.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes()); // version
    b.extend_from_slice(&0u32.to_le_bytes()); // DSD raw
    b.extend_from_slice(&(channels.len() as u32).to_le_bytes()); // channel type
    b.extend_from_slice(&(channels.len() as u32).to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes()); // bits per sample: LSB first
    b.extend_from_slice(&bits.to_le_bytes());
    b.extend_from_slice(&(BLOCK as u32).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&((12 + data_len) as u64).to_le_bytes());
    for g in 0..groups {
        for ch in channels {
            for i in g * BLOCK..(g + 1) * BLOCK {
                b.push(ch.get(i).map_or(0, |x| x.reverse_bits()));
            }
        }
    }
    b
}

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = id.to_vec();
    b.extend_from_slice(&(body.len() as u64).to_be_bytes());
    b.extend_from_slice(body);
    if body.len() % 2 == 1 {
        b.push(0);
    }
    b
}

/// A DFF file: big-endian chunks, bytes interleaved per channel, MSB first.
/// `ids` are the channel identifiers in file order; `compression` is `DSD ` or `DST `.
fn dff_bytes(channels: &[Vec<u8>], ids: &[&[u8; 4]], rate: u32, compression: &[u8; 4]) -> Vec<u8> {
    let mut chnl = (channels.len() as u16).to_be_bytes().to_vec();
    for id in ids {
        chnl.extend_from_slice(*id);
    }
    let mut cmpr = compression.to_vec();
    cmpr.extend_from_slice(b"\x0eNot compressed\x00");
    let mut prop = b"SND ".to_vec();
    prop.extend(chunk(b"FS  ", &rate.to_be_bytes()));
    prop.extend(chunk(b"CHNL", &chnl));
    prop.extend(chunk(b"CMPR", &cmpr));
    prop.extend(chunk(b"ABSS", &[0, 0, 0, 0, 0, 0, 0, 0]));
    let mut sound = Vec::new();
    for i in 0..channels[0].len() {
        for ch in channels {
            sound.push(ch[i]);
        }
    }
    let mut form = b"DSD ".to_vec();
    form.extend(chunk(b"FVER", &0x0105_0000u32.to_be_bytes()));
    form.extend(chunk(b"PROP", &prop));
    form.extend(chunk(b"COMT", &[0, 0]));
    if compression == b"DSD " {
        form.extend(chunk(b"DSD ", &sound));
    } else {
        form.extend(chunk(b"DST ", &[0; 16]));
    }
    chunk(b"FRM8", &form)
}

fn write(name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn stereo() -> Vec<Vec<u8>> {
    vec![modulate(LEFT, DSD64, BITS), modulate(RIGHT, DSD64, BITS)]
}

/// Decodes a whole file into one vector per channel.
fn decode(decoder: &mut DsdPcmDecoder) -> Vec<Vec<f64>> {
    let channels = usize::from(decoder.spec().channel_count());
    let mut out = vec![Vec::new(); channels];
    let mut block = Vec::new();
    loop {
        let n = decoder.read(1000, &mut block).unwrap();
        if n == 0 {
            break;
        }
        assert_eq!(block.len(), n * channels);
        for (i, x) in block.iter().enumerate() {
            out[i % channels].push(f64::from(*x));
        }
    }
    out
}

/// The amplitude of the `frequency` component of `x` and the RMS level of
/// everything else in dBFS, over the middle of the signal.
fn analyse(x: &[f64], rate: f64, frequency: f64) -> (f64, f64) {
    // Whole periods away from the edges.
    let period = rate / frequency;
    let periods = ((x.len() as f64 * 0.6) / period).floor();
    let start = x.len() / 5;
    let n = (periods * period).round() as usize;
    let x = &x[start..start + n];
    let (mut s, mut c) = (0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let phase = TAU * frequency * (start + i) as f64 / rate;
        s += v * phase.sin();
        c += v * phase.cos();
    }
    let (s, c) = (2.0 * s / n as f64, 2.0 * c / n as f64);
    let amplitude = s.hypot(c);
    let mut residual = 0.0;
    for (i, v) in x.iter().enumerate() {
        let phase = TAU * frequency * (start + i) as f64 / rate;
        let r = v - s * phase.sin() - c * phase.cos();
        residual += r * r;
    }
    let rest_db = 20.0 * (residual / n as f64).sqrt().log10();
    (amplitude, rest_db)
}

/// The phase of the `frequency` component, in radians.
fn phase(x: &[f64], rate: f64, frequency: f64) -> f64 {
    let (mut s, mut c) = (0.0, 0.0);
    let start = x.len() / 5;
    for (i, v) in x[start..x.len() - start].iter().enumerate() {
        let p = TAU * frequency * (start + i) as f64 / rate;
        s += v * p.sin();
        c += v * p.cos();
    }
    c.atan2(s)
}

fn check_tones(pcm: &[Vec<f64>], rate: f64) {
    for (x, tone) in pcm.iter().zip([LEFT, RIGHT]) {
        let (amplitude, rest_db) = analyse(x, rate, tone.frequency);
        // +6 dB: DSD modulation `a` becomes PCM amplitude `2a`.
        let expected = 2.0 * tone.amplitude;
        let error_db = 20.0 * (amplitude / expected).log10();
        assert!(
            error_db.abs() < 0.05,
            "{} Hz: level {amplitude} instead of {expected}",
            tone.frequency
        );
        // The test's second-order modulator leaves its noise up to the filter's
        // cutoff (about -63 dBFS); a real fifth-order one is far quieter.
        assert!(
            rest_db < -56.0,
            "{} Hz: noise and distortion at {rest_db:.1} dBFS",
            tone.frequency
        );
        // No filter delay: the sine is where the modulator put it, give or take the
        // modulator's own delay of a few DSD bits (2 µs is 5.6 bits at DSD64).
        let delay = phase(x, rate, tone.frequency) / (TAU * tone.frequency);
        assert!(
            delay.abs() < 2e-6,
            "{} Hz: shifted by {delay} s",
            tone.frequency
        );
    }
}

#[test]
fn dsf_decodes_to_the_sine() {
    let path = write("sine.dsf", &dsf_bytes(&stereo(), DSD64, BITS));
    let mut decoder = DsdPcmDecoder::open(&path).unwrap();
    assert_eq!(decoder.info().container, DsdContainer::Dsf);
    assert_eq!(decoder.info().dsd_rate, DSD64);
    assert_eq!(decoder.info().samples, BITS);
    assert_eq!(decoder.spec().rate.hz(), 176_400);
    assert_eq!(decoder.spec().layout, ChannelLayout::STEREO);
    assert_eq!(decoder.frames(), BITS / 16);
    let pcm = decode(&mut decoder);
    assert_eq!(pcm[0].len() as u64, BITS / 16);
    check_tones(&pcm, 176_400.0);
}

#[test]
fn dff_decodes_to_the_same_pcm_as_dsf() {
    let channels = stereo();
    let dsf = write("same.dsf", &dsf_bytes(&channels, DSD64, BITS));
    // Whole bytes: the DFF length is in bytes.
    let dff = write(
        "same.dff",
        &dff_bytes(&channels, &[b"SLFT", b"SRGT"], DSD64, b"DSD "),
    );
    let mut from_dff = DsdPcmDecoder::open(&dff).unwrap();
    assert_eq!(from_dff.info().container, DsdContainer::Dff);
    assert_eq!(from_dff.info().samples, BITS);
    let a = decode(&mut DsdPcmDecoder::open(&dsf).unwrap());
    let b = decode(&mut from_dff);
    assert!(a == b, "DSF and DFF decode differently");
    check_tones(&b, 176_400.0);
}

#[test]
fn dff_channels_are_reordered_to_wave_order() {
    let channels = stereo();
    // Right channel stored first.
    let swapped = vec![channels[1].clone(), channels[0].clone()];
    let path = write(
        "swapped.dff",
        &dff_bytes(&swapped, &[b"SRGT", b"SLFT"], DSD64, b"DSD "),
    );
    let pcm = decode(&mut DsdPcmDecoder::open(&path).unwrap());
    check_tones(&pcm, 176_400.0);
}

#[test]
fn raw_stream_is_msb_first_in_time_order() {
    let channels = stereo();
    for path in [
        write("raw.dsf", &dsf_bytes(&channels, DSD64, BITS)),
        write(
            "raw.dff",
            &dff_bytes(&channels, &[b"SLFT", b"SRGT"], DSD64, b"DSD "),
        ),
    ] {
        let mut reader = DsdReader::open(&path).unwrap();
        assert_eq!(reader.bytes_per_channel(), BITS / 8);
        let mut out = vec![Vec::new(), Vec::new()];
        // Odd sizes cross the 4096-byte DSF blocks.
        while reader.read(3001, &mut out).unwrap() > 0 {}
        assert!(out == channels, "{} differs", path.display());
        reader.seek(5000);
        let mut part = vec![Vec::new(), Vec::new()];
        reader.read(10, &mut part).unwrap();
        assert_eq!(part[1], channels[1][5000..5010]);
    }
}

#[test]
fn seeking_lands_on_the_exact_frame() {
    let path = write("seek.dsf", &dsf_bytes(&stereo(), DSD64, BITS));
    let mut decoder = DsdPcmDecoder::open(&path).unwrap();
    let whole = decode(&mut decoder);
    let mut block = Vec::new();
    for target in [0u64, 1, 4095, 4096, 17_001, 30_000, 44_099] {
        decoder.seek(target);
        assert_eq!(decoder.position(), target);
        let n = decoder.read(100, &mut block).unwrap();
        assert_eq!(n as u64, (44_100 - target).min(100));
        for i in 0..n {
            assert_eq!(f64::from(block[2 * i]), whole[0][target as usize + i]);
            assert_eq!(f64::from(block[2 * i + 1]), whole[1][target as usize + i]);
        }
    }
    decoder.seek(1 << 40);
    assert_eq!(decoder.position(), decoder.frames());
    assert_eq!(decoder.read(100, &mut block).unwrap(), 0);
}

#[test]
fn dsd128_decodes_to_176_4_khz() {
    let rate = 2 * DSD64;
    let channels = vec![
        modulate(LEFT, rate, 2 * BITS),
        modulate(RIGHT, rate, 2 * BITS),
    ];
    let path = write("dsd128.dsf", &dsf_bytes(&channels, rate, 2 * BITS));
    let mut decoder = DsdPcmDecoder::open(&path).unwrap();
    assert_eq!(decoder.spec().rate.hz(), 176_400);
    assert_eq!(decoder.frames(), 2 * BITS / 32);
    check_tones(&decode(&mut decoder), 176_400.0);
}

#[test]
fn dst_compression_is_unsupported() {
    let path = write(
        "dst.dff",
        &dff_bytes(&stereo(), &[b"SLFT", b"SRGT"], DSD64, b"DST "),
    );
    assert!(matches!(
        DsdReader::open(&path),
        Err(DsdError::Unsupported(_))
    ));
}

#[test]
fn other_files_are_refused() {
    let path = write("not-dsd.dsf", b"fLaC\0\0\0\x22 and so on");
    assert!(matches!(DsdReader::open(&path), Err(DsdError::NotDsd)));
    assert_eq!(sniff(b"fLaC"), None);
    let missing = dir().join("missing.dsf");
    assert!(matches!(DsdReader::open(&missing), Err(DsdError::Io(_))));
    let mut broken = dsf_bytes(&stereo(), DSD64, BITS);
    broken[28..32].copy_from_slice(b"xxxx");
    let path = write("broken.dsf", &broken);
    assert!(matches!(
        DsdReader::open(&path),
        Err(DsdError::Malformed(_))
    ));
    let path = write("short.dsf", &dsf_bytes(&stereo(), DSD64, BITS)[..60]);
    assert!(matches!(
        DsdReader::open(&path),
        Err(DsdError::Malformed(_))
    ));
}

#[test]
fn truncated_file_plays_what_is_there() {
    let bytes = dsf_bytes(&stereo(), DSD64, BITS);
    // Cut in the middle of the third block group's right channel.
    let cut = 92 + 2 * 2 * 4096 + 4096 + 100;
    let path = write("truncated.dsf", &bytes[..cut]);
    let decoder = DsdPcmDecoder::open(&path).unwrap();
    assert_eq!(decoder.info().samples, (2 * 4096 + 100) * 8);
    assert_eq!(decoder.frames(), (2 * 4096 + 100) * 8 / 16);
}
