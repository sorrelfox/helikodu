// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSP chain through its public API: the limiter's true-peak ceiling, measured
//! by oversampling; click-free changes; bit-identical bypass; equalizer responses
//! measured on signals; and AutoEQ files in the form AutoEQ and EqualizerAPO write.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

use std::f64::consts::PI;

use helikodu_engine::dsp::{DspChain, DspStage, Equalizer, Limiter, autoeq};
use helikodu_model::settings::{
    ChannelMode, CrossfeedPreset, DspSettings, EqMode, EqualizerSettings, FilterKind,
    GRAPHIC_10_BANDS_HZ,
};

const RATE: u32 = 48_000;

/// A deterministic noise source in -1..1.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

/// The true peak of one channel of `block`: the largest value of the band-limited
/// signal through its samples, found by interpolating 16 points between every two
/// samples with a long Kaiser-windowed sinc (a far finer and longer filter than the
/// limiter's own detector).
fn true_peak(block: &[f32], channels: usize, channel: usize) -> f64 {
    const HALF: isize = 64;
    const STEPS: usize = 16;
    let x: Vec<f64> = block
        .iter()
        .skip(channel)
        .step_by(channels)
        .map(|s| f64::from(*s))
        .collect();
    let i0 = |v: f64| {
        let (mut sum, mut term) = (1.0, 1.0);
        for k in 1..60 {
            term *= v / 2.0 / k as f64;
            sum += term * term;
        }
        sum
    };
    let beta = 8.0;
    let window = |u: f64| {
        let r = u / (HALF as f64 + 1.0);
        if r.abs() >= 1.0 {
            0.0
        } else {
            i0(beta * (1.0 - r * r).sqrt()) / i0(beta)
        }
    };
    let sinc = |u: f64| {
        if u == 0.0 {
            1.0
        } else {
            (PI * u).sin() / (PI * u)
        }
    };
    let table: Vec<Vec<f64>> = (1..STEPS)
        .map(|step| {
            let t = step as f64 / STEPS as f64;
            (-HALF + 1..=HALF)
                .map(|k| {
                    let u = t - k as f64;
                    sinc(u) * window(u)
                })
                .collect()
        })
        .collect();
    let mut peak: f64 = x.iter().fold(0.0, |m, v| m.max(v.abs()));
    let n = x.len() as isize;
    for i in HALF..n - HALF {
        let around = &x[(i - HALF + 1) as usize..=(i + HALF) as usize];
        for taps in &table {
            let value: f64 = taps.iter().zip(around).map(|(h, v)| h * v).sum();
            peak = peak.max(value.abs());
        }
    }
    peak
}

fn db(linear: f64) -> f64 {
    20.0 * linear.log10()
}

/// Test signals that exceed full scale in their samples, between them, or both.
fn loud_signals() -> Vec<(&'static str, Vec<f32>)> {
    let frames = RATE as usize / 8;
    // Each signal fades in over 5 ms: a sudden start is not band-limited, and its
    // overshoot between the samples depends on the reconstruction filter.
    let onset = f64::from(RATE) * 0.005;
    let fade = move |n: usize| 0.5 - 0.5 * (PI * (n as f64 / onset).min(1.0)).cos();
    let stereo = |f: &dyn Fn(usize) -> (f64, f64)| -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let (l, r) = f(n);
                [(l * fade(n)) as f32, (r * fade(n)) as f32]
            })
            .collect()
    };
    let t = |n: usize| n as f64 / f64::from(RATE);
    // Noise in the audio band: white noise through a 20 kHz low pass, at a level
    // that keeps the limiter working hard.
    let mut noise = Noise(3);
    let white: Vec<f64> = (0..frames * 2 + 256).map(|_| noise.next()).collect();
    let low_pass: Vec<f64> = (-63..=63)
        .map(|k: i32| {
            let u = f64::from(k);
            let cutoff = 20_000.0 / f64::from(RATE);
            let sinc = if k == 0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * u).sin() / (PI * u)
            };
            sinc * (0.42 + 0.5 * (PI * u / 64.0).cos() + 0.08 * (2.0 * PI * u / 64.0).cos())
        })
        .collect();
    let noisy: Vec<f32> = (0..frames * 2)
        .map(|i| {
            // Each channel is every other value of the white noise.
            let value: f64 = low_pass
                .iter()
                .enumerate()
                .map(|(k, h)| h * white[i + 2 * k])
                .sum();
            (value * 4.0 * fade(i / 2)) as f32
        })
        .collect();
    vec![
        // A quarter of the rate at 45°: samples at 0.707 of a peak 3 dB higher.
        (
            "fs/4 sine",
            stereo(&|n| {
                let v = 1.0 * (PI / 2.0 * n as f64 + PI / 4.0).sin();
                (v, -v)
            }),
        ),
        (
            "19 kHz sine",
            stereo(&|n| {
                let v = 1.2 * (2.0 * PI * 19_000.0 * t(n) + 0.3).sin();
                (v, v * 0.5)
            }),
        ),
        (
            "bass and treble",
            stereo(&|n| {
                let v =
                    1.5 * (2.0 * PI * 60.0 * t(n)).sin() + 0.6 * (2.0 * PI * 9_000.0 * t(n)).sin();
                (v, 0.3 * v)
            }),
        ),
        ("loud noise below 20 kHz", noisy),
        (
            "clipped square",
            stereo(&|n| {
                let v = if (n / 37) % 2 == 0 { 1.0 } else { -1.0 };
                (v, v)
            }),
        ),
        (
            "bursts",
            stereo(&|n| {
                let burst = if (n / 4000) % 2 == 0 { 3.0 } else { 0.1 };
                let v = burst * (2.0 * PI * 3_000.0 * t(n)).sin();
                (v, -v)
            }),
        ),
    ]
}

#[test]
fn the_limiter_holds_the_ceiling_between_samples() {
    for lookahead in [1.0, 2.0, 5.0] {
        for ceiling in [-1.0, -0.1] {
            for (name, signal) in loud_signals() {
                let mut limiter = Limiter::new(lookahead, ceiling, RATE, 2);
                let mut block = signal;
                // In blocks of uneven sizes, as the render thread gives them.
                let mut start = 0;
                for size in [1usize, 511, 3, 4096, 7].iter().cycle() {
                    if start >= block.len() {
                        break;
                    }
                    let end = (start + size * 2).min(block.len());
                    limiter.process(&mut block[start..end]);
                    start = end;
                }
                for channel in 0..2 {
                    let peak = db(true_peak(&block, 2, channel));
                    assert!(
                        peak <= ceiling + 1e-6,
                        "{name}, {lookahead} ms, ceiling {ceiling}: {peak:.4} dBTP"
                    );
                }
                assert!(limiter.activity().times > 0, "{name}: never limited");
                assert!(limiter.activity().largest_db > 0.0);
            }
        }
    }
}

#[test]
fn the_limiter_leaves_room_below_the_ceiling() {
    // A peak just under the ceiling passes untouched, only delayed.
    let mut limiter = Limiter::new(2.0, -1.0, RATE, 1);
    let level = 10f64.powf(-1.5 / 20.0);
    let input: Vec<f32> = (0..RATE as usize / 10)
        .map(|n| (level * (2.0 * PI * 1000.0 * n as f64 / f64::from(RATE)).sin()) as f32)
        .collect();
    let mut block = input.clone();
    limiter.process(&mut block);
    let latency = limiter.latency();
    assert_eq!(&block[latency..], &input[..input.len() - latency]);
}

/// The largest step between consecutive samples of channel `channel`.
fn max_step(block: &[f32], channels: usize, channel: usize) -> f64 {
    let x: Vec<f64> = block
        .iter()
        .skip(channel)
        .step_by(channels)
        .map(|s| f64::from(*s))
        .collect();
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f64::max)
}

#[test]
fn changes_while_processing_do_not_click() {
    // A slow sine whose own steps are small; settings change every 256 frames, as
    // while a slider is dragged, through every stage, including switching stages
    // on and off and the whole chain off and on again.
    let hz = 100.0;
    let amplitude = 0.2;
    let frames = RATE as usize * 2;
    let input: Vec<f32> = (0..frames)
        .flat_map(|n| {
            let v = amplitude * (2.0 * PI * hz * n as f64 / f64::from(RATE)).sin();
            [v as f32, (v * 0.8) as f32]
        })
        .collect();
    let mut settings = DspSettings::default();
    settings.equalizer.mode = EqMode::Graphic10;
    let mut chain = DspChain::new(&settings, RATE, 2);
    let mut output = input.clone();
    for (round, block) in output.chunks_mut(256 * 2).enumerate() {
        let step = round % 120;
        settings.equalizer.enabled = step < 100;
        settings.equalizer.graphic_10[1] = (step as f64 * 0.5).min(12.0);
        settings.crossfeed.enabled = step > 20 && step < 110;
        settings.crossfeed.preset = if step % 40 < 20 {
            CrossfeedPreset::JanMeier
        } else {
            CrossfeedPreset::ChuMoy
        };
        settings.channel_mix.enabled = step > 40;
        settings.channel_mix.balance_db = -(step as f64 % 13.0);
        settings.channel_mix.channels = if step > 90 {
            ChannelMode::Mono
        } else {
            ChannelMode::Stereo
        };
        settings.limiter.enabled = step > 60 && step < 115;
        settings.limiter.lookahead_ms = if step < 90 { 2.0 } else { 3.0 };
        chain.set_settings(&settings);
        chain.process(block);
    }
    // The largest boost (12 dB at the sine's band, preamp off by the automatic
    // preamp's cut) leaves the level at most what it was, so the sine's own largest
    // step bounds every step, with room for the filters' and fades' smooth changes.
    let natural = 2.0 * PI * hz / f64::from(RATE) * amplitude;
    for channel in 0..2 {
        let step = max_step(&output, 2, channel);
        assert!(
            step < natural * 1.05,
            "channel {channel}: {step} vs {natural}"
        );
    }
}

#[test]
fn a_chain_that_is_off_is_bit_identical() {
    let mut noise = Noise(9);
    let input: Vec<f32> = (0..20_000).map(|_| (noise.next() * 1.3) as f32).collect();
    let mut settings = DspSettings::default();
    let mut chain = DspChain::new(&settings, RATE, 2);
    let mut block = input.clone();
    chain.process(&mut block);
    assert_eq!(block, input);
    // On, then off again: once the crossfade back has ended, the samples are exact.
    settings.limiter.enabled = true;
    settings.crossfeed.enabled = true;
    chain.set_settings(&settings);
    chain.process(&mut block);
    assert_ne!(block, input);
    settings.limiter.enabled = false;
    settings.crossfeed.enabled = false;
    chain.set_settings(&settings);
    block.copy_from_slice(&input);
    chain.process(&mut block);
    assert!(chain.plan().is_empty() && !chain.is_active());
    block.copy_from_slice(&input);
    chain.process(&mut block);
    assert_eq!(block, input);
}

/// The steady-state gain, in dB, of `stage` for a sine at `hz` on one channel:
/// the amplitude of the output's component at `hz` over the last two seconds of
/// three, by correlation (sample peaks would miss the crests of high tones).
fn measured_gain_db(stage: &mut impl DspStage, hz: f64) -> f64 {
    stage.reset();
    let frames = 3 * RATE as usize;
    let phase = |n: usize| 2.0 * PI * hz * n as f64 / f64::from(RATE);
    let mut block: Vec<f32> = (0..frames).map(|n| (0.1 * phase(n).sin()) as f32).collect();
    stage.process(&mut block);
    let (mut re, mut im) = (0.0, 0.0);
    let from = RATE as usize;
    for (n, y) in block.iter().enumerate().skip(from) {
        re += f64::from(*y) * phase(n).cos();
        im += f64::from(*y) * phase(n).sin();
    }
    let count = (frames - from) as f64;
    let amplitude = 2.0 * (re * re + im * im).sqrt() / count;
    db(amplitude / 0.1)
}

#[test]
fn the_graphic_equalizer_meets_its_sliders_on_a_signal() {
    let mut settings = EqualizerSettings {
        mode: EqMode::Graphic10,
        auto_preamp: false,
        ..EqualizerSettings::default()
    };
    settings.graphic_10 = [6.0, 6.0, 6.0, -3.0, 0.0, 4.5, 4.5, -8.0, 2.0, 1.0];
    let mut eq = Equalizer::new(&settings, RATE, 1);
    for (hz, want) in GRAPHIC_10_BANDS_HZ.iter().zip(settings.graphic_10) {
        let got = measured_gain_db(&mut eq, *hz);
        assert!(
            (got - want).abs() < 0.1,
            "{hz} Hz: {got:.3} dB, slider {want}"
        );
    }
}

/// A headphone correction in the form AutoEQ publishes it, with Windows line
/// endings.
const AUTOEQ_TEN_BANDS: &str = "Preamp: -6.9 dB\r\n\
Filter 1: ON LSC Fc 105 Hz Gain 6.3 dB Q 0.70\r\n\
Filter 2: ON PK Fc 52 Hz Gain -0.9 dB Q 1.43\r\n\
Filter 3: ON PK Fc 180 Hz Gain -2.6 dB Q 0.66\r\n\
Filter 4: ON PK Fc 1342 Hz Gain -1.7 dB Q 1.94\r\n\
Filter 5: ON PK Fc 3091 Hz Gain 4.4 dB Q 3.21\r\n\
Filter 6: ON PK Fc 4469 Hz Gain -3.3 dB Q 4.24\r\n\
Filter 7: ON PK Fc 6022 Hz Gain 2.6 dB Q 4.66\r\n\
Filter 8: ON PK Fc 7593 Hz Gain -2.4 dB Q 3.13\r\n\
Filter 9: ON PK Fc 9861 Hz Gain 1.4 dB Q 1.84\r\n\
Filter 10: ON HSC Fc 10000 Hz Gain -2.9 dB Q 0.70\r\n";

/// The older AutoEQ form: peaking filters only, Unix line endings, a byte-order
/// mark.
const AUTOEQ_PEAKS_ONLY: &str = "\u{feff}Preamp: -5.2 dB\n\
Filter 1: ON PK Fc 21 Hz Gain 4.9 dB Q 0.48\n\
Filter 2: ON PK Fc 108 Hz Gain -2.6 dB Q 0.82\n\
Filter 3: ON PK Fc 1130 Hz Gain 1.3 dB Q 1.05\n\
Filter 4: ON PK Fc 2953 Hz Gain 5.2 dB Q 2.25\n\
Filter 5: ON PK Fc 5302 Hz Gain -4.1 dB Q 3.87\n\
Filter 6: ON PK Fc 19880 Hz Gain -6.7 dB Q 0.34\n";

/// An EqualizerAPO configuration around AutoEQ filters.
const EQUALIZER_APO: &str = "# Headphones, from AutoEQ\n\
Device: Headphones (USB Audio)\n\
Channel: all\n\
Preamp: -4.0 dB\n\
Preamp: -0.5 dB\n\
Filter 1: ON LSC 12dB Fc 105 Hz Gain 4.5 dB Q 0.71\n\
Filter 2: OFF PK Fc 2000 Hz Gain 3.0 dB Q 2.00\n\
Filter 3: ON HP Fc 15 Hz\n\
Filter 4: ON PEQ Fc 6000 Hz Gain -3.5 dB BW Oct 0.5\n\
Include: other.txt\n";

#[test]
fn autoeq_files_as_published() {
    let ten = autoeq::parse(AUTOEQ_TEN_BANDS).unwrap();
    assert_eq!(ten.preamp_db, Some(-6.9));
    assert_eq!(ten.filters.len(), 10);
    assert!(ten.warnings.is_empty(), "{:?}", ten.warnings);
    assert_eq!(ten.filters[0].kind, FilterKind::LowShelf);
    assert_eq!(ten.filters[9].kind, FilterKind::HighShelf);
    assert_eq!(ten.filters[4].frequency_hz, 3091.0);
    assert_eq!(ten.filters[4].q, 3.21);

    let peaks = autoeq::parse_bytes(AUTOEQ_PEAKS_ONLY.as_bytes()).unwrap();
    assert_eq!(peaks.preamp_db, Some(-5.2));
    assert_eq!(peaks.filters.len(), 6);
    assert!(peaks.filters.iter().all(|f| f.kind == FilterKind::Peak));

    let apo = autoeq::parse(EQUALIZER_APO).unwrap();
    assert_eq!(apo.preamp_db, Some(-4.5));
    assert_eq!(apo.filters.len(), 4);
    assert!(!apo.filters[1].enabled);
    assert_eq!(apo.filters[2].kind, FilterKind::HighPass);
    assert_eq!(apo.filters[3].kind, FilterKind::Peak);
    let lines: Vec<usize> = apo.warnings.iter().map(|w| w.line).collect();
    assert_eq!(lines, [2, 3, 10]);

    // Imported into the settings and built, the response is the filters' sum, and
    // a sine through the equalizer has that gain.
    let settings = EqualizerSettings {
        enabled: true,
        mode: EqMode::Parametric,
        auto_preamp: false,
        preamp_db: ten.preamp_db.unwrap(),
        parametric: ten.filters.clone(),
        ..EqualizerSettings::default()
    };
    assert_eq!(settings.validate(), Ok(()));
    let mut eq = Equalizer::new(&settings, RATE, 1);
    assert_eq!(eq.sections().len(), 10);
    for hz in [50.0, 105.0, 1000.0, 3091.0, 12_000.0] {
        let want = eq.magnitude_db(hz, RATE);
        let got = measured_gain_db(&mut eq, hz);
        assert!((got - want).abs() < 0.1, "{hz} Hz: {got:.3} vs {want:.3}");
    }
    // What is written reads back the same.
    let again = autoeq::parse(&autoeq::format(ten.preamp_db, &ten.filters)).unwrap();
    assert_eq!(again.filters, ten.filters);
}
