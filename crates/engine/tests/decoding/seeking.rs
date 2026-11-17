// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Seeking lands on the exact frame, in whole files and in spans.

use helikodu_engine::decode::{BlockSamples, Decoder, DecoderRegistry, PcmBlock};
use helikodu_model::audio::{MediaTime, SampleRate};
use helikodu_model::library::{MediaSource, SourceSection};

use crate::media::{self, FRAMES_44K, Source};

/// The next `frames` frames of 16-bit stereo output.
fn next_frames(decoder: &mut dyn Decoder, frames: usize) -> Vec<i16> {
    let mut block = PcmBlock::new();
    let mut out = Vec::new();
    while out.len() < frames * 2 && decoder.read(&mut block).unwrap() {
        let BlockSamples::S16(v) = block.samples() else {
            panic!("16-bit output expected");
        };
        out.extend_from_slice(v);
    }
    out.truncate(frames * 2);
    out
}

fn at(frame: usize) -> MediaTime {
    MediaTime::from_frames(frame as u64, SampleRate::HZ_44100)
}

#[test]
fn seeks_in_a_flac_file_are_exact() {
    let Some(path) = media::ffmpeg("seek.flac", Source::S44k, "flac", &["-c:a", "flac"], "flac")
    else {
        return;
    };
    let reference = media::reference(Source::S44k);
    let mut decoder = DecoderRegistry::default()
        .open(&MediaSource::file(&path))
        .unwrap();
    for target in [100_000, 7, 219_000, 4_608, 150_001, 0] {
        decoder.seek(at(target)).unwrap();
        assert_eq!(decoder.position(), target as u64);
        let got = next_frames(decoder.as_mut(), 1000);
        let want = &reference[target * 2..(target + 1000).min(FRAMES_44K) * 2];
        assert!(got == want, "seek to {target}");
    }
    decoder.seek(MediaTime::from_millis(60_000)).unwrap();
    assert!(!decoder.read(&mut PcmBlock::new()).unwrap());
}

#[test]
fn seeks_in_a_span_are_relative_to_its_start() {
    let path = media::source_wav(Source::S44k);
    let reference = media::reference(Source::S44k);
    let start = 44_100;
    let end = 132_300;
    let source = MediaSource::file(&path).with_section(SourceSection::Span {
        start: at(start),
        end: Some(at(end)),
    });
    let mut decoder = DecoderRegistry::default().open(&source).unwrap();
    assert_eq!(decoder.position(), 0);
    for target in [10_000, 0, 88_199, 50_000] {
        decoder.seek(at(target)).unwrap();
        assert_eq!(decoder.position(), target as u64);
        let mut block = PcmBlock::new();
        assert!(decoder.read(&mut block).unwrap());
        assert_eq!(block.position(), target as u64);
        let got = next_frames(decoder.as_mut(), 500);
        let from = start + target + block.frames();
        let to = (from + 500).min(end);
        assert!(got == reference[from * 2..to * 2], "seek to {target}");
    }
    // Past the end of the span: nothing more, not the rest of the file.
    decoder.seek(at(100_000)).unwrap();
    assert_eq!(decoder.position(), (end - start) as u64);
    assert!(!decoder.read(&mut PcmBlock::new()).unwrap());
}
