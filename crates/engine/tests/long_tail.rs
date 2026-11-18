// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The long tail of formats in the engine: the synthesizing backends recognised by
//! content whatever the file's name, subsongs, the SoundFont setting, whole-file
//! buffering and its limit, the audio-stream selector, and DSD over PCM (DoP)
//! checked bit for bit through the WAV sink. Everything plays to files or nowhere;
//! nothing is audible.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]
// Frames read out of byte buffers read better as slices.
#![allow(clippy::chunks_exact_to_as_chunks)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use helikodu_engine::decode::{Decoder, DecoderRegistry, PcmBlock, ProbeScore};
use helikodu_engine::output::Pacing;
use helikodu_engine::transport::{Engine, EngineConfig};
use helikodu_model::audio::{
    BackendSelection, DeviceId, DeviceSelection, MediaTime, OutputBackendKind, OutputMode,
};
use helikodu_model::library::{MediaSource, SourceSection};
use helikodu_model::player::{EngineCommand, EngineEvent, ItemId, PlaybackItem, TransportState};
use helikodu_model::settings::{DecoderSettings, DsdOutput, OutputFormat, VolumeControl};
use helikodu_testkit::longtail;

const PATIENCE: Duration = Duration::from_secs(30);

fn dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine-long-tail");
        fs::create_dir_all(&d).unwrap();
        d
    })
}

fn write(name: &str, data: &[u8]) -> PathBuf {
    let path = dir().join(name);
    fs::write(&path, data).unwrap();
    path
}

/// Every frame of `decoder` as `f32`.
fn decode_all(decoder: &mut dyn Decoder) -> Vec<f32> {
    let mut block = PcmBlock::new();
    let mut out = Vec::new();
    while decoder.read(&mut block).unwrap() {
        block.append_f32(&mut out);
    }
    out
}

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

/// The decoder settings with the test SoundFont.
fn with_soundfont() -> DecoderSettings {
    DecoderSettings {
        soundfont: Some(write("sine.sf2", &longtail::soundfont())),
        default_length_s: 10,
        fade_ms: 1000,
        ..DecoderSettings::default()
    }
}

#[test]
fn long_tail_files_are_recognised_by_content_not_by_name() {
    let registry = DecoderRegistry::with_settings(&with_soundfont());
    // Misleading names: the content decides.
    let cases = [
        ("module.mp3", longtail::tracker_module(), "openmpt"),
        ("game.flac", longtail::nsf_file(), "gme"),
        ("tune.wav", longtail::psid_file(), "sidplayfp"),
        ("song.ogg", longtail::midi_file(), "rustysynth"),
    ];
    for (name, data, backend) in cases {
        let path = write(name, &data);
        let candidates: Vec<_> = registry
            .candidates(&path)
            .unwrap()
            .into_iter()
            .map(|(f, score)| (f.name(), score))
            .collect();
        assert_eq!(candidates[0], (backend, ProbeScore::SIGNATURE), "{name}");
        assert_eq!(candidates.last().unwrap().0, "ffmpeg", "{name}");
        let mut decoder = registry.open(&MediaSource::file(&path)).unwrap();
        let info = decoder.info().clone();
        assert_eq!(info.backend, backend, "{name}");
        assert!(!info.in_memory);
        let samples = decode_all(decoder.as_mut());
        let frames = samples.len() as u64 / 2;
        assert_eq!(
            Some(MediaTime::from_frames(frames, decoder.spec().rate)),
            decoder.duration(),
            "{name}: the decoded length is the stated one"
        );
        assert!(rms(&samples) > 0.005, "{name}: silent");
    }
}

#[test]
fn subsongs_are_chosen_through_the_section() {
    let registry = DecoderRegistry::with_settings(&with_soundfont());
    let nsf = write("tracks.nsf", &longtail::nsf_file());
    let decoder = registry
        .open(&MediaSource::file(&nsf).with_section(SourceSection::Subsong(2)))
        .unwrap();
    let info = decoder.info();
    assert_eq!((info.backend, info.subsong), ("gme", Some(2)));
    assert!(
        info.tags
            .contains(&("album".into(), longtail::NSF_TITLE.into()))
    );
    assert_eq!(
        decoder.duration().map(MediaTime::as_duration),
        Some(Duration::from_secs(11))
    );

    let module = write("subsongs.mod", &longtail::tracker_module());
    let decoder = registry
        .open(&MediaSource::file(&module).with_section(SourceSection::Subsong(1)))
        .unwrap();
    assert_eq!(decoder.info().subsong, Some(1));
    let length = decoder.duration().unwrap().as_duration().as_secs_f64();
    assert!((length - 0.96).abs() < 0.01, "{length}");

    // A subsong the file does not have: no backend opens it.
    match registry.open(&MediaSource::file(&nsf).with_section(SourceSection::Subsong(7))) {
        Err(error) => assert!(error.to_string().contains("no subsong 7"), "{error}"),
        Ok(_) => panic!("subsong 7 opened"),
    }
    // A span of a subsong file plays a part of its default subsong.
    let decoder = registry
        .open(&MediaSource::file(&nsf).with_section(SourceSection::Span {
            start: MediaTime::from_millis(1000),
            end: Some(MediaTime::from_millis(2000)),
        }))
        .unwrap();
    assert_eq!(
        decoder.duration().map(MediaTime::as_duration),
        Some(Duration::from_secs(1))
    );
}

#[test]
fn midi_needs_the_soundfont_setting() {
    let midi = write("needs-font.mid", &longtail::midi_file());
    let registry = DecoderRegistry::with_settings(&DecoderSettings {
        soundfont: Some(dir().join("missing.sf2")),
        ..DecoderSettings::default()
    });
    assert!(registry.open(&MediaSource::file(&midi)).is_err());
    registry.configure(&with_soundfont());
    let decoder = registry.open(&MediaSource::file(&midi)).unwrap();
    assert!(
        decoder
            .info()
            .tags
            .contains(&("title".into(), longtail::MIDI_TITLE.into()))
    );
}

#[test]
fn whole_file_buffering_is_off_by_default_and_capped() {
    let module = write("buffered.mod", &longtail::tracker_module());
    let wav = write("buffered.wav", &wav_s16(&[1000; 4410], 44_100, 1));
    let registry = DecoderRegistry::with_default_backends();
    assert!(!registry.options().settings.load_into_memory);
    assert_eq!(registry.options().memory_limit, 512 << 20);
    for path in [&module, &wav] {
        assert!(
            !registry
                .open(&MediaSource::file(path))
                .unwrap()
                .info()
                .in_memory
        );
    }
    registry.configure(&DecoderSettings {
        load_into_memory: true,
        ..DecoderSettings::default()
    });
    for path in [&module, &wav] {
        let mut decoder = registry.open(&MediaSource::file(path)).unwrap();
        assert!(decoder.info().in_memory, "{}", path.display());
        assert!(!decode_all(decoder.as_mut()).is_empty());
    }
    // Over the limit, files are read from the drive.
    registry.set_memory_limit(1024);
    for path in [&module, &wav] {
        assert!(
            !registry
                .open(&MediaSource::file(path))
                .unwrap()
                .info()
                .in_memory
        );
    }
}

/// A WAV file of 16-bit samples.
fn wav_s16(samples: &[i16], rate: u32, channels: u16) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let block = channels * 2;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    b.extend_from_slice(&block.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    b
}

/// A Matroska file with two audio streams, a loud one (index 0) and a silent one
/// (index 1), muxed by the `ffmpeg` program; `None` when it is not installed.
fn two_streams() -> Option<PathBuf> {
    let loud: Vec<i16> = (0..48_000)
        .map(|i| if i % 40 < 20 { 8000 } else { -8000 })
        .collect();
    let a = write("stream-loud.wav", &wav_s16(&loud, 48_000, 1));
    let b = write("stream-silent.wav", &wav_s16(&[0; 48_000], 48_000, 1));
    let out = dir().join("two-streams.mkv");
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&a)
        .arg("-i")
        .arg(&b)
        .args(["-map", "0:a", "-map", "1:a", "-c:a", "pcm_s16le"])
        .arg(&out)
        .status();
    match status {
        Ok(status) if status.success() => Some(out),
        _ => {
            println!("SKIPPED: the ffmpeg program is needed to mux two audio streams");
            None
        }
    }
}

#[test]
fn the_audio_stream_is_chosen_by_index() {
    let Some(path) = two_streams() else {
        return;
    };
    let registry = DecoderRegistry::with_default_backends();
    let source = MediaSource::file(&path);
    for (index, loud) in [(0u32, true), (1, false)] {
        let mut decoder = registry.open_stream(&source, Some(index)).unwrap();
        let info = decoder.info().clone();
        assert_eq!(info.stream, Some(index));
        let listed: Vec<u32> = info.streams.iter().map(|s| s.index).collect();
        assert_eq!(listed, [0, 1]);
        let level = rms(&decode_all(decoder.as_mut()));
        assert_eq!(level > 0.1, loud, "stream {index}: {level}");
    }
    // Not an audio stream.
    assert!(registry.open_stream(&source, Some(5)).is_err());
}

/// Engine settings that play to the WAV file `out` as fast as it goes.
fn wav_config(out: &Path) -> EngineConfig {
    let mut config = EngineConfig {
        sink_pacing: Pacing::Freewheel,
        ..EngineConfig::default()
    };
    config.output.backend = BackendSelection::Backend(OutputBackendKind::Wav);
    config.output.device = DeviceSelection::Device(DeviceId::new(out.to_str().unwrap()).unwrap());
    config
}

/// Waits for `wanted` to return a value for an event, answering `NearEnd` with
/// nothing; fails on an error.
fn wait_for<T>(
    events: &Receiver<EngineEvent>,
    engine: &Engine,
    mut wanted: impl FnMut(&EngineEvent) -> Option<T>,
) -> T {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = events
            .recv_timeout(left)
            .expect("the engine did not answer");
        if let Some(found) = wanted(&event) {
            return found;
        }
        match event {
            EngineEvent::NearEnd { .. } => engine.send(EngineCommand::SetNext(None)).unwrap(),
            EngineEvent::Error { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
}

fn stopped(event: &EngineEvent) -> Option<()> {
    matches!(event, EngineEvent::StateChanged(TransportState::Stopped)).then_some(())
}

/// The data of a WAV file the sink wrote (68-byte header) and its container bits.
fn read_output(path: &Path) -> (Vec<u8>, u16) {
    let b = fs::read(path).unwrap();
    assert_eq!(&b[0..4], b"RIFF");
    let container = u16::from_le_bytes([b[34], b[35]]);
    let len = u32::from_le_bytes(b[64..68].try_into().unwrap()) as usize;
    (b[68..68 + len].to_vec(), container)
}

fn play_item(config: EngineConfig, item: PlaybackItem) {
    let (engine, events) = Engine::spawn(config).unwrap();
    engine
        .send(EngineCommand::Load {
            item,
            start: MediaTime::ZERO,
            paused: false,
        })
        .unwrap();
    wait_for(&events, &engine, stopped);
}

#[test]
fn the_engine_plays_a_subsong() {
    let module = write("engine.mod", &longtail::tracker_module());
    let out = dir().join("engine-subsong.wav");
    let item = PlaybackItem::new(
        ItemId::new(1),
        MediaSource::file(&module).with_section(SourceSection::Subsong(1)),
    );
    play_item(wav_config(&out), item);
    let (data, container) = read_output(&out);
    let frames = data.len() / usize::from(container / 8) / 2;
    assert!(
        (frames as f64 / 44_100.0 - 0.96).abs() < 0.01,
        "{frames} frames"
    );
}

#[test]
fn the_engine_switches_the_audio_stream() {
    let Some(path) = two_streams() else {
        return;
    };
    let out = dir().join("engine-streams.wav");
    let (engine, events) = Engine::spawn(wav_config(&out)).unwrap();
    let item = PlaybackItem::new(ItemId::new(7), MediaSource::file(&path));
    engine
        .send(EngineCommand::Load {
            item,
            start: MediaTime::ZERO,
            paused: true,
        })
        .unwrap();
    let first = wait_for(&events, &engine, |event| match event {
        EngineEvent::AudioStreams {
            item,
            streams,
            playing,
        } => {
            assert_eq!(*item, ItemId::new(7));
            assert_eq!(streams.len(), 2);
            Some(*playing)
        }
        _ => None,
    });
    let other = 1 - first;
    engine
        .send(EngineCommand::SelectAudioStream(Some(other)))
        .unwrap();
    let now = wait_for(&events, &engine, |event| match event {
        EngineEvent::AudioStreams { playing, .. } => Some(*playing),
        _ => None,
    });
    assert_eq!(now, other);
    engine.send(EngineCommand::Play).unwrap();
    wait_for(&events, &engine, stopped);
    drop(engine);
    let (data, _) = read_output(&out);
    let samples: Vec<f32> = data
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0)
        .collect();
    assert!(!samples.is_empty());
    // Stream 0 is the loud one.
    assert_eq!(rms(&samples) > 0.1, other == 0, "rms {}", rms(&samples));
}

/// The canonical DSD bytes of the test file (earliest bit most significant), per
/// channel.
fn dsd_bytes(channel: usize, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i * [7, 13][channel] + [3, 5][channel]) as u8)
        .collect()
}

/// A stereo DSF file (DSD64) of `blocks` blocks of 4096 bytes per channel holding
/// [`dsd_bytes`].
fn write_dsf(name: &str, blocks: usize) -> PathBuf {
    const BLOCK: usize = 4096;
    let len = blocks * BLOCK;
    let channels = [dsd_bytes(0, len), dsd_bytes(1, len)];
    let mut data = Vec::with_capacity(2 * len);
    for block in 0..blocks {
        for channel in &channels {
            // DSF stores the earliest bit in the least significant position.
            data.extend(
                channel[block * BLOCK..(block + 1) * BLOCK]
                    .iter()
                    .map(|b| b.reverse_bits()),
            );
        }
    }
    let total = 28 + 52 + 12 + data.len() as u64;
    let mut b = Vec::new();
    b.extend_from_slice(b"DSD ");
    b.extend_from_slice(&28u64.to_le_bytes());
    b.extend_from_slice(&total.to_le_bytes());
    b.extend_from_slice(&0u64.to_le_bytes());
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&52u64.to_le_bytes());
    for value in [1u32, 0, 2, 2, 2_822_400, 1] {
        b.extend_from_slice(&value.to_le_bytes());
    }
    b.extend_from_slice(&(len as u64 * 8).to_le_bytes());
    b.extend_from_slice(&(BLOCK as u32).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(12 + data.len() as u64).to_le_bytes());
    b.extend_from_slice(&data);
    write(name, &b)
}

/// Plays `dsf` to a 32-bit WAV file in exclusive mode with DoP asked for, and the
/// volume control `volume`; returns the 32-bit samples written.
fn play_dop(dsf: &Path, name: &str, volume: VolumeControl, dsd: DsdOutput) -> Vec<i32> {
    let out = dir().join(name);
    let mut config = wav_config(&out);
    config.output.mode = OutputMode::Exclusive;
    config.output.format = OutputFormat::S32;
    config.output.volume = volume;
    config.output.dsd = dsd;
    play_item(
        config,
        PlaybackItem::new(ItemId::new(3), MediaSource::file(dsf)),
    );
    let (data, container) = read_output(&out);
    assert_eq!(container, 32);
    data.chunks_exact(4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

#[test]
fn dop_reaches_the_wav_sink_bit_for_bit() {
    let blocks = 3;
    let dsf = write_dsf("dop.dsf", blocks);
    let words = play_dop(&dsf, "dop-out.wav", VolumeControl::Fixed, DsdOutput::Dop);
    let bytes = blocks * 4096;
    let frames = bytes / 2;
    assert_eq!(
        words.len(),
        frames * 2,
        "176.4 kHz frames: one per 16 DSD bits"
    );
    let channels = [dsd_bytes(0, bytes), dsd_bytes(1, bytes)];
    for (frame, pair) in words.chunks_exact(2).enumerate() {
        let marker = if frame % 2 == 0 { 0x05 } else { 0xFA };
        for (channel, &word) in pair.iter().enumerate() {
            let expected = i32::from_be_bytes([
                marker,
                channels[channel][2 * frame],
                channels[channel][2 * frame + 1],
                0,
            ]);
            assert_eq!(
                word as u32, expected as u32,
                "frame {frame}, channel {channel}: {word:#010x} instead of {expected:#010x}"
            );
        }
    }
}

#[test]
fn dop_falls_back_to_pcm_on_a_path_that_changes_samples() {
    let dsf = write_dsf("dop-fallback.dsf", 3);
    let is_dop = |words: &[i32]| {
        words.chunks_exact(2).enumerate().all(|(frame, pair)| {
            let marker = if frame % 2 == 0 { 0x05 } else { 0xFA };
            pair.iter().all(|&w| (w as u32 >> 24) as u8 == marker)
        })
    };
    // Software volume may change the samples at any time: PCM.
    let words = play_dop(
        &dsf,
        "dop-software-volume.wav",
        VolumeControl::Software,
        DsdOutput::Dop,
    );
    assert!(!words.is_empty());
    assert!(!is_dop(&words));
    // DoP not asked for: PCM, as by default.
    let words = play_dop(&dsf, "dop-off.wav", VolumeControl::Fixed, DsdOutput::Pcm);
    assert!(!is_dop(&words));
}
