// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The processing path through its public API: path plans, the bit-perfect bypass,
//! resampling accuracy and complete chains.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

use std::f64::consts::PI;

use helikodu_engine::pipeline::{
    Encoder, FadeDurations, LoudnessInfo, LoudnessMode, LoudnessSettings, OutputChain, PathPlan,
    PathRequest, ProcessingSettings, ResampleMethod, ResampleStage, ResamplerQuality, SourceChain,
    SourceFormat, Transition, VolumePlan, to_float,
};
use helikodu_model::audio::{AudioSpec, ChannelLayout, Endianness, SampleFormat, SampleRate};
use helikodu_model::player::Volume;

fn rate(hz: u32) -> SampleRate {
    SampleRate::new(hz).unwrap()
}

fn spec(format: SampleFormat, hz: u32, layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(format, rate(hz), layout)
}

fn request(source: SourceFormat, device: AudioSpec, volume: VolumePlan) -> PathRequest {
    PathRequest {
        source,
        loudness: LoudnessInfo::default(),
        album_context: false,
        track_volume_db: 0.0,
        device,
        volume,
        dsp: false,
        crossfade: false,
    }
}

/// Runs `input` through both chains in blocks of `block` frames, then drains them.
fn run(plan: &PathPlan, input: &[f32], block: usize) -> Vec<f32> {
    let mut source = SourceChain::new(plan, block).unwrap();
    let mut output = OutputChain::new(plan, FadeDurations::default());
    output.set_open(true);
    let in_channels = source.input_channels();
    let out_channels = source.output_channels();
    let mut buffer = vec![0.0f32; source.max_output_frames() * out_channels];
    let mut result = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        let (taken, written) = source.process(rest, &mut buffer);
        rest = &rest[taken * in_channels..];
        let out = &mut buffer[..written * out_channels];
        output.apply(out);
        result.extend_from_slice(out);
    }
    loop {
        let written = source.finish(&mut buffer);
        if written == 0 {
            break;
        }
        let out = &mut buffer[..written * out_channels];
        output.apply(out);
        result.extend_from_slice(out);
    }
    result
}

/// A deterministic test signal of `frames` frames of `channels` channels: a
/// different tone per channel with a little added pseudo-random content.
fn signal_i16(frames: usize, channels: usize) -> Vec<i16> {
    let mut state = 0x1234_5678u32;
    (0..frames * channels)
        .map(|i| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let (n, c) = ((i / channels) as f64, (i % channels) as f64);
            let tone = (2.0 * PI * (440.0 + 110.0 * c) * n / 44_100.0).sin() * 20_000.0;
            (tone + f64::from(state >> 24) - 128.0) as i16
        })
        .collect()
}

#[test]
fn bypass_is_bit_identical() {
    let cd = spec(
        SampleFormat::S16(Endianness::Little),
        44_100,
        ChannelLayout::STEREO,
    );
    let plan = PathPlan::new(
        &request(
            SourceFormat::new(cd),
            cd,
            VolumePlan::Software(Volume::FULL),
        ),
        &ProcessingSettings::default(),
    );
    assert!(plan.is_bit_perfect());

    let samples = signal_i16(10_000, 2);
    let mut floats = vec![0.0f32; samples.len()];
    to_float(&samples, &mut floats);
    let out = run(&plan, &floats, 777);
    assert_eq!(
        out.iter().map(|s| s.to_bits()).collect::<Vec<_>>(),
        floats.iter().map(|s| s.to_bits()).collect::<Vec<_>>()
    );

    // And the device receives the decoded integers.
    let encoder = Encoder::new(cd.format, 2).unwrap();
    let mut bytes = vec![0u8; out.len() * 2];
    encoder.encode(&out, &mut bytes);
    let original: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    assert_eq!(bytes, original);
}

#[test]
fn bypass_carries_24_bit_samples_in_32_bit_words() {
    let source = SourceFormat::new(spec(
        SampleFormat::S32(Endianness::NATIVE),
        96_000,
        ChannelLayout::STEREO,
    ))
    .with_bits(24);
    let device = spec(
        SampleFormat::S24Packed(Endianness::Little),
        96_000,
        ChannelLayout::STEREO,
    );
    let plan = PathPlan::new(
        &request(source, device, VolumePlan::Stream),
        &ProcessingSettings::default(),
    );
    assert!(plan.is_bit_perfect());
    let values: Vec<i32> = (0..4000i32)
        .map(|i| (i * 2099).wrapping_mul(7919) % 8_388_608)
        .collect();
    let words: Vec<i32> = values.iter().map(|v| v << 8).collect();
    let mut floats = vec![0.0f32; words.len()];
    to_float(&words, &mut floats);
    let out = run(&plan, &floats, 512);
    let mut bytes = vec![0u8; out.len() * 3];
    Encoder::new(device.format, 2)
        .unwrap()
        .encode(&out, &mut bytes);
    let decoded: Vec<i32> = bytes
        .chunks(3)
        .map(|b| i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8)
        .collect();
    assert_eq!(decoded, values);
}

/// The frequency of a sine, from its rising zero crossings (linearly interpolated).
fn frequency(samples: &[f32], rate: f64) -> f64 {
    let mut crossings = Vec::new();
    for (i, w) in samples.windows(2).enumerate() {
        if w[0] < 0.0 && w[1] >= 0.0 {
            let t = f64::from(-w[0]) / f64::from(w[1] - w[0]);
            crossings.push(i as f64 + t);
        }
    }
    let (first, last) = (crossings[0], crossings[crossings.len() - 1]);
    (crossings.len() - 1) as f64 / (last - first) * rate
}

/// The amplitude and the delay in frames of `samples`, which start at frame `start`
/// of a sine of `frequency` that began at frame 0, by least squares.
fn fit(samples: &[f32], start: usize, frequency: f64, rate: f64) -> (f64, f64) {
    let w = 2.0 * PI * frequency / rate;
    let (mut ss, mut cc, mut sc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &y) in samples.iter().enumerate() {
        let (s, c) = (w * (start + i) as f64).sin_cos();
        let y = f64::from(y);
        ss += s * s;
        cc += c * c;
        sc += s * c;
        ys += y * s;
        yc += y * c;
    }
    let det = ss * cc - sc * sc;
    let a = (ys * cc - yc * sc) / det;
    let b = (yc * ss - ys * sc) / det;
    // y = a sin(wn) + b cos(wn) = r sin(w(n - d)) with tan(-w d) = b / a.
    (a.hypot(b), -b.atan2(a) / w)
}

fn sine(frequency: f64, rate: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (0.5 * (2.0 * PI * frequency * n as f64 / rate).sin()) as f32)
        .collect()
}

#[test]
fn resampled_sine_keeps_its_frequency_phase_and_length() {
    for (from, to) in [(44_100, 48_000), (48_000, 44_100), (44_100, 44_056)] {
        for quality in [ResamplerQuality::Low, ResamplerQuality::High] {
            let mut stage = ResampleStage::new(rate(from), rate(to), 1, quality).unwrap();
            let input = sine(1000.0, f64::from(from), from as usize);
            let mut out = vec![0.0f32; stage.max_output_frames(1000)];
            let mut result = Vec::new();
            for block in input.chunks(1000) {
                let (taken, written) = stage.process(block, &mut out);
                assert_eq!(taken, block.len());
                result.extend_from_slice(&out[..written]);
            }
            loop {
                let written = stage.finish(&mut out);
                if written == 0 {
                    break;
                }
                result.extend_from_slice(&out[..written]);
            }
            // One second in, one second out.
            assert!(result.len().abs_diff(to as usize) <= 1, "{from} -> {to}");

            let middle = &result[2000..result.len() - 2000];
            let measured = frequency(middle, f64::from(to));
            assert!(
                (measured - 1000.0).abs() < 0.05,
                "{from} -> {to} ({quality}): {measured} Hz"
            );
            // The start-up delay is removed and the level kept: fit the output to
            // a sine and a cosine of the ideal frequency.
            let (amplitude, delay) = fit(middle, 2000, 1000.0, f64::from(to));
            assert!(
                (amplitude - 0.5).abs() < 0.005,
                "{from} -> {to} ({quality}): amplitude {amplitude}"
            );
            assert!(
                delay.abs() < 1.0,
                "{from} -> {to} ({quality}): delay {delay} frames"
            );
            let method = stage.method();
            if to == 44_056 {
                assert_eq!(method, ResampleMethod::Asynchronous);
            } else {
                assert_eq!(method, ResampleMethod::Synchronous);
            }
        }
    }
}

#[test]
fn processed_path_downmixes_resamples_and_dithers() {
    let source = SourceFormat::new(spec(
        SampleFormat::F32(Endianness::NATIVE),
        96_000,
        ChannelLayout::SURROUND_5_1,
    ));
    let device = spec(
        SampleFormat::S16(Endianness::Little),
        48_000,
        ChannelLayout::STEREO,
    );
    let mut r = request(
        source,
        device,
        VolumePlan::Software(Volume::new(0.9).unwrap()),
    );
    r.loudness = LoudnessInfo {
        track_gain_db: Some(-3.0),
        ..LoudnessInfo::default()
    };
    let settings = ProcessingSettings {
        loudness: LoudnessSettings {
            mode: LoudnessMode::Track,
            ..LoudnessSettings::default()
        },
        ..ProcessingSettings::default()
    };
    let plan = PathPlan::new(&r, &settings);
    assert!(!plan.is_bit_perfect());
    assert_eq!(plan.stages().len(), 5, "{plan}");

    // Full-scale, in-phase content on every channel must not clip after the downmix.
    let frames = 96_000 / 2;
    let tone = sine(500.0, 96_000.0, frames);
    let input: Vec<f32> = tone.iter().flat_map(|&s| [s * 2.0; 6]).collect();
    let out = run(&plan, &input, 4096);
    assert_eq!(out.len(), frames / 2 * 2);
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let gain = 10f32.powf(-3.0 / 20.0) * 0.9f32.powi(3);
    assert!(peak <= 1.0 && (peak - gain).abs() < 0.01, "peak {peak}");
    for s in &out {
        let v = f64::from(*s) * 32_768.0;
        assert_eq!(v, v.round(), "not quantized to 16 bits");
    }
}

#[test]
fn output_chain_fades_and_ramps_without_clicks() {
    let cd = spec(
        SampleFormat::S16(Endianness::Little),
        44_100,
        ChannelLayout::MONO,
    );
    let plan = PathPlan::new(
        &request(
            SourceFormat::new(cd),
            cd,
            VolumePlan::Software(Volume::FULL),
        ),
        &ProcessingSettings::default(),
    );
    let mut chain = OutputChain::new(&plan, FadeDurations::default());
    // A full-scale sine whose own largest step is known.
    let tone: Vec<f32> = sine(441.0, 44_100.0, 44_100)
        .iter()
        .map(|s| s * 2.0)
        .collect();
    let largest_step = |s: &[f32]| {
        s.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let natural = largest_step(&tone);
    let mut out = tone.clone();
    let mut blocks = out.chunks_mut(512).enumerate();
    for (i, block) in &mut blocks {
        match i {
            0 => chain.fade_in(Transition::Start),
            10 => chain
                .volume_mut()
                .unwrap()
                .set_volume(Volume::new(0.3).unwrap()),
            20 => chain.volume_mut().unwrap().set_volume(Volume::FULL),
            40 => chain.fade_out(Transition::Pause),
            _ => {}
        }
        chain.apply(block);
    }
    assert!(chain.fader().is_silent());
    // Every change is spread out: no step is much larger than the sine's own.
    let threshold = natural + 0.01;
    let step = largest_step(&out);
    assert!(step <= threshold, "step {step}, natural {natural}");
    assert!(out[..1].iter().all(|s| s.abs() < 0.01), "starts silent");
    assert!(out[41 * 512..].iter().all(|&s| s == 0.0), "ends silent");
}
