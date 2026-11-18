// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bit-perfect playback, checked bit for bit: the engine plays generated files of
//! 16-, 24- and 32-bit integer and 32-bit float samples to the WAV sink, and the
//! samples it wrote must be the bytes of the source. The signal path it reports must
//! say so, and must say otherwise when the device format cannot carry the source.
//!
//! With the `snd-aloop` kernel module loaded (`Loopback` in `/proc/asound/cards`),
//! the same is checked through ALSA: played to the loopback's playback side and
//! recorded from its capture side. Without it that test prints `SKIPPED:` and passes;
//! it never loads the module, and the loopback makes no sound.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use helikodu_engine::output::Pacing;
use helikodu_engine::transport::{Engine, EngineConfig};
use helikodu_model::audio::{
    BackendSelection, DeviceId, DeviceSelection, MediaTime, OutputBackendKind,
};
use helikodu_model::library::MediaSource;
use helikodu_model::player::{
    EngineCommand, EngineError, EngineEvent, ItemId, PlaybackItem, SignalLamp, SignalPath,
    TransportState,
};
use helikodu_model::settings::OutputFormat;

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const FRAMES: usize = 30_000;
const PATIENCE: Duration = Duration::from_secs(30);

/// A source file's sample format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    S16,
    S24,
    S32,
    F32,
}

impl Kind {
    fn bytes(self) -> usize {
        match self {
            Self::S16 => 2,
            Self::S24 => 3,
            Self::S32 | Self::F32 => 4,
        }
    }
}

fn dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine-bit-perfect");
        fs::create_dir_all(&d).unwrap();
        d
    })
}

/// Pseudo-random samples that use every bit of `kind`, as the little-endian bytes of
/// a WAV file's data chunk. No sample is zero, so that the start of the audio can be
/// found in a recording.
fn samples(kind: Kind, frames: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut out = Vec::with_capacity(frames * usize::from(CHANNELS) * kind.bytes());
    for _ in 0..frames * usize::from(CHANNELS) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let word = (state >> 32) as u32 | 1;
        match kind {
            Kind::S16 => out.extend_from_slice(&(word as u16 | 1).to_le_bytes()),
            Kind::S24 => out.extend_from_slice(&(word | 1).to_le_bytes()[..3]),
            Kind::S32 => out.extend_from_slice(&word.to_le_bytes()),
            Kind::F32 => {
                // Floats between -1 and 1 with a full mantissa.
                let x = (f64::from(word as i32) / 2_147_483_648.0) as f32;
                out.extend_from_slice(&x.to_le_bytes());
            }
        }
    }
    out
}

/// Writes `data` as a WAV file of `kind`.
fn write_wav(name: &str, kind: Kind, data: &[u8]) -> PathBuf {
    let bits = (kind.bytes() * 8) as u16;
    let block = CHANNELS * kind.bytes() as u16;
    let format: u16 = if kind == Kind::F32 { 3 } else { 1 };
    let mut b = Vec::with_capacity(44 + data.len());
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&format.to_le_bytes());
    b.extend_from_slice(&CHANNELS.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * u32::from(block)).to_le_bytes());
    b.extend_from_slice(&block.to_le_bytes());
    b.extend_from_slice(&bits.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(data);
    let path = dir().join(name);
    fs::write(&path, b).unwrap();
    path
}

/// The data chunk of a file the WAV sink wrote (68-byte header) and its container
/// and valid bits.
fn read_output(path: &Path) -> (Vec<u8>, u16, u16) {
    let b = fs::read(path).unwrap();
    assert_eq!(&b[0..4], b"RIFF");
    let container = u16::from_le_bytes([b[34], b[35]]);
    let valid = u16::from_le_bytes([b[38], b[39]]);
    let len = u32::from_le_bytes(b[64..68].try_into().unwrap()) as usize;
    (b[68..68 + len].to_vec(), container, valid)
}

/// Plays `source` to the WAV file `out` as fast as it goes, with `format` as the
/// device format setting, and returns the signal paths the engine reported.
fn play_to_wav(source: &Path, out: &Path, edit: impl FnOnce(&mut EngineConfig)) -> Vec<SignalPath> {
    let mut config = EngineConfig {
        sink_pacing: Pacing::Freewheel,
        ..EngineConfig::default()
    };
    config.output.backend = BackendSelection::Backend(OutputBackendKind::Wav);
    config.output.device = DeviceSelection::Device(DeviceId::new(out.to_str().unwrap()).unwrap());
    edit(&mut config);
    play(config, source).unwrap_or_else(|error| panic!("{error}"))
}

fn play(config: EngineConfig, source: &Path) -> Result<Vec<SignalPath>, EngineError> {
    let (engine, events) = Engine::spawn(config).unwrap();
    engine
        .send(EngineCommand::Load {
            item: PlaybackItem::new(ItemId::new(1), MediaSource::file(source)),
            start: MediaTime::ZERO,
            paused: false,
        })
        .unwrap();
    let paths = wait_stopped(&events, &engine);
    drop(engine);
    paths
}

/// Answers `NearEnd` with nothing until the engine stops; returns the signal paths,
/// or the first error.
fn wait_stopped(
    events: &Receiver<EngineEvent>,
    engine: &Engine,
) -> Result<Vec<SignalPath>, EngineError> {
    let deadline = Instant::now() + PATIENCE;
    let mut paths = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left).expect("the engine did not stop") {
            EngineEvent::NearEnd { .. } => engine.send(EngineCommand::SetNext(None)).unwrap(),
            EngineEvent::SignalPath(Some(path)) => paths.push(*path),
            EngineEvent::StateChanged(TransportState::Stopped) => return Ok(paths),
            EngineEvent::Error { error, .. } => return Err(error),
            _ => {}
        }
    }
}

fn check_bit_perfect(kind: Kind, format: OutputFormat) {
    let data = samples(kind, FRAMES);
    let name = format!("{kind:?}-{}", format.as_str()).to_lowercase();
    let source = write_wav(&format!("{name}-in.wav"), kind, &data);
    let out = dir().join(format!("{name}-out.wav"));
    let paths = play_to_wav(&source, &out, |config| config.output.format = format);
    let (written, container, valid) = read_output(&out);
    assert_eq!(usize::from(container), kind.bytes() * 8, "{name}");
    assert_eq!(usize::from(valid), kind.bytes() * 8, "{name}");
    assert_eq!(written.len(), data.len(), "{name}: length");
    if let Some(at) = written.iter().zip(&data).position(|(a, b)| a != b) {
        panic!("{name}: the output differs from byte {at} on");
    }
    let path = paths
        .last()
        .unwrap_or_else(|| panic!("{name}: no signal path"));
    assert_eq!(path.lamp, SignalLamp::BitPerfect, "{name}: {path}");
    assert_eq!(path.active_stages().count(), 0, "{name}: {path}");
}

#[test]
fn integer_and_float_sources_reach_the_wav_sink_bit_for_bit() {
    check_bit_perfect(Kind::S16, OutputFormat::Auto);
    check_bit_perfect(Kind::S24, OutputFormat::S24Packed);
    check_bit_perfect(Kind::S32, OutputFormat::Auto);
    check_bit_perfect(Kind::F32, OutputFormat::Auto);
}

#[test]
fn narrower_samples_widen_exactly_and_wider_ones_are_reported_lossy() {
    // 24-bit samples in a 32-bit device format: the same values, in the top bits.
    let data = samples(Kind::S24, FRAMES);
    let source = write_wav("widen-in.wav", Kind::S24, &data);
    let out = dir().join("widen-out.wav");
    let paths = play_to_wav(&source, &out, |config| {
        config.output.format = OutputFormat::S32
    });
    let (written, _, _) = read_output(&out);
    let expected: Vec<u8> = data
        .chunks(3)
        .flat_map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]])).to_le_bytes())
        .collect();
    assert!(written == expected, "24 bits in 32 differ");
    assert_eq!(paths.last().unwrap().lamp, SignalLamp::BitPerfect);

    // 32-bit samples into a 24-bit device: dithered, and the path says bits are lost.
    let data = samples(Kind::S32, FRAMES);
    let source = write_wav("narrow-in.wav", Kind::S32, &data);
    let out = dir().join("narrow-out.wav");
    let paths = play_to_wav(&source, &out, |config| {
        config.output.format = OutputFormat::S24Packed;
    });
    let path = paths.last().unwrap();
    assert_eq!(path.lamp, SignalLamp::Lossy, "{path}");
    assert!(
        path.active_stages()
            .any(|s| s.kind == helikodu_model::player::StageKind::Dither),
        "{path}"
    );
}

#[test]
fn dac_warm_up_is_silence_before_the_first_audio() {
    let data = samples(Kind::S16, FRAMES);
    let source = write_wav("preroll-in.wav", Kind::S16, &data);
    let out = dir().join("preroll-out.wav");
    play_to_wav(&source, &out, |config| config.output.preroll_ms = 100);
    let (written, _, _) = read_output(&out);
    let silence = RATE as usize / 10 * usize::from(CHANNELS) * 2;
    assert_eq!(written.len(), silence + data.len());
    assert!(written[..silence].iter().all(|b| *b == 0));
    assert!(
        written[silence..] == data[..],
        "the audio after the warm-up differs"
    );
}

/// The bit-perfect path through ALSA: plays 32-bit samples to the playback side of
/// the `snd-aloop` loopback card and records them from its capture side.
#[cfg(feature = "alsa")]
#[test]
fn alsa_loopback_carries_32_bit_samples_exactly() {
    use ::alsa::pcm::{Access, Format, HwParams, PCM};
    use ::alsa::{Direction, ValueOr};
    use helikodu_model::audio::OutputMode;

    let cards = fs::read_to_string("/proc/asound/cards").unwrap_or_default();
    if !cards.contains("Loopback") {
        eprintln!("SKIPPED: no snd-aloop loopback card; the ALSA loopback is not tested");
        return;
    }
    let capture = match PCM::new("hw:CARD=Loopback,DEV=1,SUBDEV=0", Direction::Capture, false) {
        Ok(pcm) => pcm,
        Err(error) => {
            eprintln!("SKIPPED: the loopback's capture side does not open: {error}");
            return;
        }
    };
    {
        let hw = HwParams::any(&capture).unwrap();
        hw.set_access(Access::RWInterleaved).unwrap();
        hw.set_format(Format::S32LE).unwrap();
        hw.set_channels(u32::from(CHANNELS)).unwrap();
        hw.set_rate(RATE, ValueOr::Nearest).unwrap();
        capture.hw_params(&hw).unwrap();
    }
    capture.start().unwrap();
    let wanted = FRAMES + RATE as usize;
    let recorder = std::thread::spawn(move || {
        let io = capture.io_i32().unwrap();
        let mut recorded = vec![0i32; wanted * usize::from(CHANNELS)];
        let mut done = 0;
        while done < wanted {
            match io.readi(&mut recorded[done * usize::from(CHANNELS)..]) {
                Ok(frames) => done += frames,
                Err(error) => {
                    if capture.try_recover(error, true).is_err() {
                        break;
                    }
                }
            }
        }
        recorded
    });

    let data = samples(Kind::S32, FRAMES);
    let source = write_wav("loopback-in.wav", Kind::S32, &data);
    let mut config = EngineConfig::default();
    config.output.backend = BackendSelection::Backend(OutputBackendKind::Alsa);
    config.output.device =
        DeviceSelection::Device(DeviceId::new("hw:CARD=Loopback,DEV=0").unwrap());
    config.output.mode = OutputMode::Exclusive;
    let paths = match play(config, &source) {
        Ok(paths) => paths,
        Err(error @ EngineError::DeviceUnavailable { .. }) => {
            eprintln!("SKIPPED: the loopback card is not free: {error}");
            return;
        }
        Err(error) => panic!("{error}"),
    };
    let recorded = recorder.join().unwrap();

    let expected: Vec<i32> = data
        .chunks(4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let start = recorded
        .iter()
        .position(|v| *v != 0)
        .expect("nothing was recorded");
    let got = &recorded[start..(start + expected.len()).min(recorded.len())];
    assert!(got == expected, "the recording differs from the source");
    assert_eq!(paths.last().unwrap().lamp, SignalLamp::BitPerfect);
}
