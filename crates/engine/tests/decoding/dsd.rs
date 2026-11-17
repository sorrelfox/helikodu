// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! DSF and DFF files go to the DSD backend and play as PCM.

use std::f64::consts::TAU;
use std::path::PathBuf;
use std::sync::OnceLock;

use helikodu_engine::decode::{DecodeError, DecoderRegistry, PcmBlock, ProbeScore};
use helikodu_model::audio::{ChannelLayout, Endianness, MediaTime, SampleFormat};
use helikodu_model::library::{MediaSource, SourceSection};

use crate::media;

const DSD64: u32 = 2_822_400;
/// Half a second plus one byte: not a whole number of PCM frames.
const BITS: u64 = 1_411_208;
const FRAMES: usize = (BITS / 16) as usize;
const FREQUENCY: f64 = 1000.0;
/// Peak modulation of the test sine; 0.5 is the DSD 0 dB reference.
const AMPLITUDE: f64 = 0.25;

/// A second-order sigma-delta modulator: the sine as bytes, earliest bit most
/// significant.
fn modulate(amplitude: f64) -> Vec<u8> {
    let (mut i1, mut i2) = (0.0f64, 0.0f64);
    let mut out = vec![0u8; BITS.div_ceil(8) as usize];
    for n in 0..BITS {
        let x = amplitude * (TAU * FREQUENCY * n as f64 / f64::from(DSD64)).sin();
        let y = if i2 >= 0.0 { 1.0 } else { -1.0 };
        if y > 0.0 {
            out[(n / 8) as usize] |= 0x80 >> (n % 8);
        }
        i1 += x - y;
        i2 += i1 - y;
    }
    out
}

/// The left channel carries the sine, the right one half of it.
fn channels() -> &'static [Vec<u8>; 2] {
    static CHANNELS: OnceLock<[Vec<u8>; 2]> = OnceLock::new();
    CHANNELS.get_or_init(|| [modulate(AMPLITUDE), modulate(AMPLITUDE / 2.0)])
}

fn dsf_bytes() -> Vec<u8> {
    const BLOCK: usize = 4096;
    let channels = channels();
    let groups = channels[0].len().div_ceil(BLOCK);
    let data_len = (groups * BLOCK * 2) as u64;
    let mut b = Vec::new();
    b.extend_from_slice(b"DSD ");
    b.extend_from_slice(&28u64.to_le_bytes());
    b.extend_from_slice(&(92 + data_len).to_le_bytes());
    b.extend_from_slice(&0u64.to_le_bytes());
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&52u64.to_le_bytes());
    for v in [1u32, 0, 2, 2, DSD64, 1] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&BITS.to_le_bytes());
    b.extend_from_slice(&(BLOCK as u32).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(12 + data_len).to_le_bytes());
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

fn dff_bytes(compression: &[u8; 4]) -> Vec<u8> {
    let channels = channels();
    let mut prop = b"SND ".to_vec();
    prop.extend(chunk(b"FS  ", &DSD64.to_be_bytes()));
    prop.extend(chunk(b"CHNL", b"\0\x02SLFTSRGT"));
    let mut cmpr = compression.to_vec();
    cmpr.extend_from_slice(b"\x0eNot compressed\0");
    prop.extend(chunk(b"CMPR", &cmpr));
    let mut form = b"DSD ".to_vec();
    form.extend(chunk(b"FVER", &0x0105_0000u32.to_be_bytes()));
    form.extend(chunk(b"PROP", &prop));
    if compression == b"DSD " {
        let sound: Vec<u8> = channels[0]
            .iter()
            .zip(&channels[1])
            .flat_map(|(l, r)| [*l, *r])
            .collect();
        form.extend(chunk(b"DSD ", &sound));
    } else {
        form.extend(chunk(b"DST ", &[0; 64]));
    }
    chunk(b"FRM8", &form)
}

fn dsf() -> PathBuf {
    media::once("sine.dsf", |p| {
        std::fs::write(p, dsf_bytes()).map_err(|e| e.to_string())
    })
    .unwrap()
}

fn dff() -> PathBuf {
    media::once("sine.dff", |p| {
        std::fs::write(p, dff_bytes(b"DSD ")).map_err(|e| e.to_string())
    })
    .unwrap()
}

/// The amplitude of the test frequency in `x`, over its middle.
fn amplitude(x: &[f64], rate: f64) -> f64 {
    let period = rate / FREQUENCY;
    let start = x.len() / 5;
    let n = (((x.len() as f64 * 0.6) / period).floor() * period).round() as usize;
    let (mut s, mut c) = (0.0, 0.0);
    for (i, v) in x[start..start + n].iter().enumerate() {
        let phase = TAU * FREQUENCY * (start + i) as f64 / rate;
        s += v * phase.sin();
        c += v * phase.cos();
    }
    (2.0 * s / n as f64).hypot(2.0 * c / n as f64)
}

#[test]
fn dsf_and_dff_go_to_the_dsd_backend_and_play_as_pcm() {
    for (path, container) in [(dsf(), "dsf"), (dff(), "dff")] {
        let candidates: Vec<_> = DecoderRegistry::default()
            .candidates(&path)
            .unwrap()
            .into_iter()
            .map(|(f, score)| (f.name(), score))
            .collect();
        assert_eq!(
            candidates,
            [
                ("dsd", ProbeScore::SIGNATURE),
                ("ffmpeg", ProbeScore::FALLBACK)
            ]
        );
        let d = media::decode_file(&path);
        let info = d.decoder.info();
        assert_eq!((info.backend, info.container.as_str()), ("dsd", container));
        assert_eq!(d.spec.format, SampleFormat::F32(Endianness::NATIVE));
        assert_eq!(d.spec.rate.hz(), 176_400);
        assert_eq!(d.spec.layout, ChannelLayout::STEREO);
        assert_eq!(d.frames(), FRAMES, "{container}: length");
        assert_eq!(
            d.decoder.duration(),
            Some(MediaTime::from_frames(FRAMES as u64, d.spec.rate))
        );
        // +6 dB: modulation `a` plays at PCM amplitude `2a`.
        for (channel, expected) in [(0, 2.0 * AMPLITUDE), (1, AMPLITUDE)] {
            let x: Vec<f64> = d
                .floats
                .iter()
                .skip(channel)
                .step_by(2)
                .map(|&v| f64::from(v))
                .collect();
            let level = amplitude(&x, 176_400.0);
            let error_db = 20.0 * (level / expected).log10();
            assert!(
                error_db.abs() < 0.05,
                "{container} channel {channel}: {level}"
            );
        }
    }
}

#[test]
fn a_dsf_file_with_another_name_is_still_dsd() {
    let path = media::dir().join("really-dsd.flac");
    std::fs::copy(dsf(), &path).unwrap();
    let d = media::decode_file(&path);
    assert_eq!(d.decoder.info().backend, "dsd");
    assert_eq!(d.frames(), FRAMES);
}

#[test]
fn dsd_spans_and_seeks_are_exact() {
    let whole = media::decode_file(&dsf()).floats;
    let rate = helikodu_model::audio::SampleRate::new(176_400).unwrap();
    let (start, end) = (10_001u64, 60_000u64);
    let source = MediaSource::file(dsf()).with_section(SourceSection::Span {
        start: MediaTime::from_frames(start, rate),
        end: Some(MediaTime::from_frames(end, rate)),
    });
    let d = media::decode(&source);
    assert_eq!(d.frames() as u64, end - start);
    assert!(d.floats == whole[start as usize * 2..end as usize * 2]);

    let mut decoder = DecoderRegistry::default()
        .open(&MediaSource::file(dsf()))
        .unwrap();
    let mut block = PcmBlock::new();
    for target in [70_000u64, 3, 44_100] {
        decoder.seek(MediaTime::from_frames(target, rate)).unwrap();
        assert!(decoder.read(&mut block).unwrap());
        assert_eq!(block.position(), target);
        let mut got = Vec::new();
        block.append_f32(&mut got);
        let t = target as usize * 2;
        assert!(got == whole[t..t + got.len()], "seek to {target}");
    }
}

/// DST-compressed DFF is left to FFmpeg; with this made-up DST data FFmpeg may
/// fail too, but the DSD backend's reason comes first.
#[test]
fn dst_compressed_dff_falls_through_to_ffmpeg() {
    let path = media::write("dst.dff", &dff_bytes(b"DST "));
    match DecoderRegistry::default().open(&MediaSource::file(&path)) {
        Ok(decoder) => assert_eq!(decoder.info().backend, "ffmpeg"),
        Err(DecodeError::NoDecoder { attempts }) => {
            assert_eq!(attempts.len(), 2, "{attempts:?}");
            assert_eq!(attempts[0].0, "dsd");
            assert!(attempts[0].1.contains("DST"), "{attempts:?}");
            assert_eq!(attempts[1].0, "ffmpeg");
        }
        Err(e) => panic!("{e:?}"),
    }
}
