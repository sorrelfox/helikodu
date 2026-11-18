// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The processing stages never allocate once built.
//!
//! A counting allocator wraps the system allocator and counts the allocations of the
//! thread that asks for it, while the stages process blocks.

// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use helikodu_engine::pipeline::{
    FadeDurations, LoudnessInfo, LoudnessMode, LoudnessSettings, OutputChain, PathPlan,
    PathRequest, ProcessingSettings, SharedVolume, SoftwareVolume, SourceChain, SourceFormat,
    Transition, VolumePlan,
};
use helikodu_model::audio::{AudioSpec, ChannelLayout, Endianness, SampleFormat, SampleRate};
use helikodu_model::player::Volume;

struct Counting;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

fn note() {
    // `try_with`: the thread-local may already be gone while a thread exits.
    let _ = ACTIVE.try_with(|active| {
        if active.get() {
            let _ = COUNT.try_with(|count| count.set(count.get() + 1));
        }
    });
}

// SAFETY: every call is passed on unchanged to the system allocator, which upholds
// the `GlobalAlloc` contract; counting touches only const-initialized thread-locals,
// which do not allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller's guarantees for `layout` are passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: as for `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: `ptr` came from this allocator, which is the system allocator.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from this allocator, which is the system allocator.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The number of allocations `f` makes on this thread.
fn allocations(f: impl FnOnce()) -> usize {
    COUNT.with(|c| c.set(0));
    ACTIVE.with(|a| a.set(true));
    f();
    ACTIVE.with(|a| a.set(false));
    COUNT.with(Cell::get)
}

fn plan(source_hz: u32, device_hz: u32) -> PathPlan {
    let source = AudioSpec::new(
        SampleFormat::F32(Endianness::NATIVE),
        SampleRate::new(source_hz).unwrap(),
        ChannelLayout::SURROUND_5_1,
    );
    let device = AudioSpec::new(
        SampleFormat::S16(Endianness::Little),
        SampleRate::new(device_hz).unwrap(),
        ChannelLayout::STEREO,
    );
    let request = PathRequest {
        source: SourceFormat::new(source),
        loudness: LoudnessInfo {
            track_gain_db: Some(-4.0),
            ..LoudnessInfo::default()
        },
        album_context: false,
        device,
        volume: VolumePlan::Software(Volume::new(0.7).unwrap()),
        dsp: false,
        crossfade: false,
    };
    let settings = ProcessingSettings {
        loudness: LoudnessSettings {
            mode: LoudnessMode::Track,
            ..LoudnessSettings::default()
        },
        ..ProcessingSettings::default()
    };
    PathPlan::new(&request, &settings)
}

#[test]
fn processing_does_not_allocate() {
    // The counter itself works.
    assert!(allocations(|| drop(std::hint::black_box(vec![1u8; 10]))) > 0);

    // Synchronous (FFT) and asynchronous (sinc) resampling.
    for (from, to) in [(44_100, 48_000), (44_100, 44_056)] {
        let plan = plan(from, to);
        let block = 1000;
        let mut source = SourceChain::new(&plan, block).unwrap();
        let mut output = OutputChain::new(&plan, FadeDurations::default());
        let shared = SharedVolume::new(Volume::new(0.7).unwrap());
        output.set_volume_stage(Some(SoftwareVolume::following(to, 2, shared.clone())));
        let input: Vec<f32> = (0..block * 6).map(|i| (i % 97) as f32 / 200.0).collect();
        let mut buffer = vec![0.0f32; source.max_output_frames() * 2];

        let count = allocations(|| {
            output.fade_in(Transition::Start);
            for i in 0..50 {
                if i == 10 {
                    shared.set_volume(Volume::new(0.2).unwrap());
                    source.gain_mut().unwrap().set_gain(0.5);
                }
                if i == 30 {
                    output.fade_out(Transition::Seek);
                }
                if i == 32 {
                    source.reset();
                    output.fade_in(Transition::Seek);
                }
                let mut rest = &input[..];
                while !rest.is_empty() {
                    let (taken, written) = source.process(rest, &mut buffer);
                    rest = &rest[taken * 6..];
                    output.apply(&mut buffer[..written * 2]);
                }
            }
            output.fade_out(Transition::Stop);
            loop {
                let written = source.finish(&mut buffer);
                if written == 0 {
                    break;
                }
                output.apply(&mut buffer[..written * 2]);
            }
        });
        assert_eq!(count, 0, "{from} -> {to}: {count} allocations");
    }
}

#[test]
fn the_dsp_chain_does_not_allocate() {
    use helikodu_engine::dsp::DspChain;
    use helikodu_model::settings::{ChannelMode, DspSettings, EqMode, ParametricFilter};

    let mut settings = DspSettings::default();
    settings.equalizer.enabled = true;
    settings.equalizer.mode = EqMode::Graphic31;
    settings.equalizer.graphic_31[10] = 6.0;
    settings.equalizer.parametric = vec![ParametricFilter::default(); 64];
    settings.crossfeed.enabled = true;
    settings.channel_mix.enabled = true;
    settings.channel_mix.channels = ChannelMode::Mono;
    settings.channel_mix.balance_db = 3.0;
    settings.limiter.enabled = true;
    let mut chain = DspChain::new(&settings, 48_000, 2);
    let input: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 1.8).collect();
    let mut block = input.clone();
    let mut tail = vec![0.0f32; 4096];
    for round in 0..4 {
        // Building the new stages allocates; processing, crossfades included, not.
        match round {
            1 => settings.equalizer.graphic_31[10] = -6.0,
            2 => {
                settings.equalizer.mode = EqMode::Parametric;
                settings.equalizer.parametric[3].gain_db = 4.0;
                settings.limiter.lookahead_ms = 4.0;
            }
            3 => settings.channel_mix.channels = ChannelMode::Swap,
            _ => {}
        }
        chain.set_settings(&settings);
        let count = allocations(|| {
            for _ in 0..10 {
                block.copy_from_slice(&input);
                chain.process(&mut block);
            }
            let _ = chain.flush(&mut tail);
            chain.reset();
        });
        assert_eq!(count, 0, "round {round}: {count} allocations");
    }
}
