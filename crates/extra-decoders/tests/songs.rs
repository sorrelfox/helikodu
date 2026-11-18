// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The synthesizing decoders on the project's own sample files (from
//! `helikodu-testkit`): recognition, subsongs, metadata, lengths, the fade at the
//! end, and exact seeks. Nothing is played.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used, dead_code)]

#[cfg(feature = "song")]
use std::time::Duration;

#[cfg(feature = "song")]
use helikodu_extra_decoders::song::{SongDecoder, SongOptions};

/// Options for short tests: 2 s before the fade, 1 s of fade.
#[cfg(feature = "song")]
fn short() -> SongOptions {
    SongOptions {
        default_length: Duration::from_secs(2),
        fade: Duration::from_secs(1),
        ..SongOptions::default()
    }
}

/// Every frame the decoder gives, read in blocks of 1000.
#[cfg(feature = "song")]
fn drain(decoder: &mut dyn SongDecoder) -> Vec<f32> {
    let mut all = Vec::new();
    let mut block = Vec::new();
    loop {
        let got = decoder.read(1000, &mut block).unwrap();
        assert_eq!(block.len(), got * usize::from(decoder.channels()));
        if got == 0 {
            return all;
        }
        all.extend_from_slice(&block);
    }
}

/// Asserts that two runs of samples differ by no more than `tolerance` (0: are
/// identical), naming the first difference.
fn same(a: &[f32], b: &[f32], tolerance: f32) {
    assert_eq!(a.len(), b.len());
    let differ = |x: &f32, y: &f32| {
        if tolerance == 0.0 {
            x.to_bits() != y.to_bits()
        } else {
            (x - y).abs() > tolerance
        }
    };
    if let Some(i) = a.iter().zip(b).position(|(x, y)| differ(x, y)) {
        panic!("the samples differ from index {i}: {} and {}", a[i], b[i]);
    }
}

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

/// Seeks a fresh decoder to `at` and checks that what follows is what a straight
/// run gave there.
#[cfg(feature = "song")]
fn check_exact_seek(decoder: Box<dyn SongDecoder>, all: &[f32], at: u64) {
    check_seek(decoder, all, at, 0.0);
}

/// [`check_exact_seek`] for decoders whose runs differ by up to `tolerance`.
#[cfg(feature = "song")]
fn check_seek(mut decoder: Box<dyn SongDecoder>, all: &[f32], at: u64, tolerance: f32) {
    decoder.seek(at).unwrap();
    assert_eq!(decoder.position(), at);
    let mut block = Vec::new();
    let got = decoder.read(500, &mut block).unwrap();
    assert!(got > 0);
    let from = at as usize * 2;
    same(&block, &all[from..from + got * 2], tolerance);
    // And back to the start.
    decoder.seek(0).unwrap();
    let got = decoder.read(500, &mut block).unwrap();
    same(&block, &all[..got * 2], tolerance);
}

#[cfg(feature = "openmpt")]
mod openmpt {
    use helikodu_extra_decoders::openmpt::{OpenMptDecoder, library_version, probe};
    use helikodu_extra_decoders::song::{SongDecoder, SongError, SongOptions};
    use helikodu_testkit::longtail;

    use super::*;

    #[test]
    fn recognises_modules_by_content() {
        let module = longtail::tracker_module();
        assert!(probe(&module));
        assert!(!probe(&longtail::midi_file()));
        assert!(!probe(&longtail::nsf_file()));
        assert!(!probe(b"RIFF\0\0\0\0WAVEfmt "));
        assert!(!probe(b""));
        assert!(library_version().starts_with("0."));
    }

    #[test]
    fn subsongs_lengths_and_text() {
        let module = longtail::tracker_module();
        let options = SongOptions::default();
        let mut decoder = OpenMptDecoder::open(&module, None, &options).unwrap();
        let info = decoder.info().clone();
        assert_eq!(info.title.as_deref(), Some(longtail::MODULE_TITLE));
        assert!(info.format.starts_with("ProTracker"), "{}", info.format);
        assert_eq!(info.subsong_count(), 2, "{info:?}");
        let lengths: Vec<f64> = info
            .subsongs
            .iter()
            .map(|s| s.length.unwrap().as_secs_f64())
            .collect();
        assert!((lengths[0] - 1.92).abs() < 0.01, "{lengths:?}");
        assert!((lengths[1] - 0.96).abs() < 0.01, "{lengths:?}");

        assert_eq!(decoder.subsong(), 0);
        let frames = decoder.frames().unwrap();
        let all = drain(&mut decoder);
        assert_eq!(all.len() as u64, frames * 2);
        assert!(rms(&all) > 0.01, "silent");

        let mut second = OpenMptDecoder::open(&module, Some(1), &options).unwrap();
        assert_eq!(second.subsong(), 1);
        let all = drain(&mut second);
        assert!((all.len() as f64 / 2.0 / 44_100.0 - 0.96).abs() < 0.01);

        assert!(matches!(
            OpenMptDecoder::open(&module, Some(2), &options),
            Err(SongError::NoSuchSubsong { index: 2, count: 2 })
        ));
        assert!(matches!(
            OpenMptDecoder::open(b"not a module at all", None, &options),
            Err(SongError::Unsupported(_))
        ));
    }

    #[test]
    fn seeks_to_the_frame() {
        let module = longtail::tracker_module();
        let mut decoder = OpenMptDecoder::open(&module, None, &SongOptions::default()).unwrap();
        decoder.seek(50_001).unwrap();
        assert_eq!(decoder.position(), 50_001);
        let rest = drain(&mut decoder);
        assert_eq!(rest.len() as u64 / 2, decoder.frames().unwrap() - 50_001);
        decoder.seek(u64::MAX).unwrap();
        assert_eq!(decoder.position(), decoder.frames().unwrap());
    }
}

#[cfg(feature = "gme")]
mod gme {
    use helikodu_extra_decoders::gme::{GmeDecoder, probe};
    use helikodu_extra_decoders::song::{SongDecoder, SongError};
    use helikodu_testkit::longtail;

    use super::*;

    #[test]
    fn recognises_game_music_by_content() {
        assert!(probe(&longtail::nsf_file()));
        assert!(!probe(&longtail::tracker_module()));
        assert!(!probe(&longtail::psid_file()));
        assert!(!probe(b"NE"));
    }

    #[test]
    fn tracks_are_subsongs_with_the_default_length_and_a_fade() {
        let nsf = longtail::nsf_file();
        let options = short();
        let mut decoder = GmeDecoder::open(&nsf, None, &options).unwrap();
        let info = decoder.info().clone();
        assert_eq!(info.format, "Nintendo NES");
        assert_eq!(info.subsong_count(), 3);
        assert_eq!(info.album.as_deref(), Some(longtail::NSF_TITLE));
        assert_eq!(info.title.as_deref(), Some(longtail::NSF_TITLE));
        assert_eq!(info.artist.as_deref(), Some(longtail::NSF_ARTIST));
        assert_eq!(info.date.as_deref(), Some(longtail::NSF_COPYRIGHT));
        assert!(
            info.subsongs
                .iter()
                .all(|s| s.length == Some(Duration::from_secs(3)))
        );
        assert_eq!(decoder.frames(), Some(3 * 44_100));
        let all = drain(&mut decoder);
        assert_eq!(all.len(), 3 * 44_100 * 2);
        let second = &all[44_100 * 2..2 * 44_100 * 2];
        let last = &all[all.len() - 2000..];
        assert!(rms(second) > 0.02, "silent: {}", rms(second));
        assert!(rms(last) < rms(second) / 20.0, "no fade");

        // Each track plays its own pitch: count the zero crossings.
        let crossings = |samples: &[f32]| {
            samples
                .iter()
                .step_by(2)
                .collect::<Vec<_>>()
                .windows(2)
                .filter(|w| (*w[0] < 0.0) != (*w[1] < 0.0))
                .count()
        };
        let mut third = GmeDecoder::open(&nsf, Some(2), &options).unwrap();
        assert_eq!(third.subsong(), 2);
        let other = drain(&mut third);
        assert!(crossings(&other[..88_200]) > crossings(&all[..88_200]) * 6 / 5);

        assert!(matches!(
            GmeDecoder::open(&nsf, Some(3), &options),
            Err(SongError::NoSuchSubsong { index: 3, count: 3 })
        ));
    }

    #[test]
    fn seeks_to_the_exact_frame() {
        let nsf = longtail::nsf_file();
        let mut decoder = GmeDecoder::open(&nsf, Some(1), &short()).unwrap();
        let all = drain(&mut decoder);
        let fresh = GmeDecoder::open(&nsf, Some(1), &short()).unwrap();
        check_exact_seek(Box::new(fresh), &all, 70_001);
    }

    #[test]
    fn loops_play_the_stated_number_of_times() {
        // An NSF has no loop information; the loop count leaves its length alone.
        let nsf = longtail::nsf_file();
        let mut options = short();
        options.loops = 5;
        let decoder = GmeDecoder::open(&nsf, None, &options).unwrap();
        assert_eq!(decoder.frames(), Some(3 * 44_100));
    }
}

#[cfg(feature = "sid")]
mod sid {
    use helikodu_extra_decoders::sid::{SidDecoder, probe, tune_md5};
    use helikodu_extra_decoders::song::SongDecoder;
    use helikodu_testkit::longtail;

    use super::*;

    #[test]
    fn recognises_tunes_by_content() {
        assert!(probe(&longtail::psid_file()));
        assert!(!probe(&longtail::nsf_file()));
        assert!(!probe(b"PSID"));
    }

    #[test]
    fn songs_text_and_default_length() {
        let tune = longtail::psid_file();
        let mut decoder = SidDecoder::open(&tune, None, &short()).unwrap();
        let info = decoder.info().clone();
        assert_eq!(info.subsong_count(), 2);
        assert_eq!(info.default_subsong, 1);
        assert_eq!(decoder.subsong(), 1);
        assert_eq!(info.title.as_deref(), Some(longtail::SID_TITLE));
        assert_eq!(info.artist.as_deref(), Some(longtail::SID_AUTHOR));
        assert_eq!(info.date.as_deref(), Some(longtail::SID_RELEASED));
        assert!(info.format.contains("PSID"), "{}", info.format);
        assert_eq!(decoder.frames(), Some(3 * 44_100));
        let all = drain(&mut decoder);
        assert_eq!(all.len(), 3 * 44_100 * 2);
        let second = &all[44_100 * 2..2 * 44_100 * 2];
        assert!(rms(second) > 0.01, "silent: {}", rms(second));
        assert!(
            rms(&all[all.len() - 2000..]) < rms(second) / 20.0,
            "no fade"
        );
    }

    #[test]
    fn seeks_to_the_exact_frame() {
        let tune = longtail::psid_file();
        let mut decoder = SidDecoder::open(&tune, Some(0), &short()).unwrap();
        let all = drain(&mut decoder);
        let fresh = SidDecoder::open(&tune, Some(0), &short()).unwrap();
        // The emulation differs from run to run in the last bits.
        check_seek(Box::new(fresh), &all, 60_003, 8.0 / 32_768.0);
    }

    #[test]
    fn lengths_come_from_the_song_length_database() {
        let tune = longtail::psid_file();
        let md5 = tune_md5(&tune).unwrap();
        assert_eq!(md5.len(), 32);
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("sid-songlengths");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Songlengths.md5");
        std::fs::write(
            &path,
            format!("; /Helikodu/Triangle.sid\n[Database]\n{md5}=0:03 0:01.500\n"),
        )
        .unwrap();
        let options = SongOptions {
            sid_song_lengths: Some(path),
            ..short()
        };
        let decoder = SidDecoder::open(&tune, None, &options).unwrap();
        let lengths: Vec<_> = decoder.info().subsongs.iter().map(|s| s.length).collect();
        assert_eq!(
            lengths,
            [
                Some(Duration::from_secs(3)),
                Some(Duration::from_millis(1500))
            ]
        );
        assert_eq!(decoder.frames(), Some(66_150));
    }
}

#[cfg(feature = "midi")]
mod midi {
    use std::path::PathBuf;

    use helikodu_extra_decoders::midi::{MidiDecoder, RELEASE, probe};
    use helikodu_extra_decoders::song::{SongDecoder, SongOptions};
    use helikodu_testkit::longtail;

    use super::*;

    fn soundfont() -> PathBuf {
        static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        PATH.get_or_init(|| {
            let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("midi");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("sine.sf2");
            std::fs::write(&path, longtail::soundfont()).unwrap();
            path
        })
        .clone()
    }

    fn options() -> SongOptions {
        SongOptions {
            soundfont: Some(soundfont()),
            ..SongOptions::default()
        }
    }

    #[test]
    fn recognises_midi_by_content() {
        assert!(probe(&longtail::midi_file()));
        assert!(!probe(&longtail::soundfont()));
    }

    #[test]
    fn plays_with_the_soundfont_and_reads_the_text() {
        let mut decoder = MidiDecoder::open(&longtail::midi_file(), None, &options()).unwrap();
        let info = decoder.info().clone();
        assert_eq!(info.title.as_deref(), Some(longtail::MIDI_TITLE));
        assert_eq!(info.date.as_deref(), Some(longtail::MIDI_COPYRIGHT));
        assert_eq!(info.subsong_count(), 1);
        let expected = Duration::from_secs(2) + RELEASE;
        assert_eq!(info.subsongs[0].length, Some(expected));
        let all = drain(&mut decoder);
        assert_eq!(all.len() as u64, decoder.frames().unwrap() * 2);
        assert!((all.len() as f64 / 2.0 / 44_100.0 - expected.as_secs_f64()).abs() < 0.001);
        assert!(
            rms(&all[..88_200]) > 0.01,
            "silent: {}",
            rms(&all[..88_200])
        );
    }

    #[test]
    fn seeks_to_the_exact_frame() {
        let midi = longtail::midi_file();
        let mut decoder = MidiDecoder::open(&midi, None, &options()).unwrap();
        let all = drain(&mut decoder);
        let fresh = MidiDecoder::open(&midi, None, &options()).unwrap();
        check_exact_seek(Box::new(fresh), &all, 40_007);
    }

    #[test]
    fn a_missing_soundfont_is_an_error() {
        let options = SongOptions {
            soundfont: Some("/nonexistent/none.sf2".into()),
            ..SongOptions::default()
        };
        assert!(MidiDecoder::open(&longtail::midi_file(), None, &options).is_err());
        assert!(MidiDecoder::open(&longtail::midi_file(), Some(1), &self::options()).is_err());
    }
}
