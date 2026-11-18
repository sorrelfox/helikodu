// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Invented signal paths, one for each lamp the engine reports, and playback
//! counters to go with them, in the engine's own wording. For the sample
//! player, the widget tests and the screenshots.

use helikodu_model::audio::{
    AudioSpec, ChannelLayout, DeviceId, DeviceSelection, Endianness, OutputBackendKind, OutputMode,
    SampleFormat, SampleRate,
};
use helikodu_model::player::{
    ItemId, PlaybackStats, SignalDevice, SignalLamp, SignalPath, SignalSource, SignalStage,
    StageKind, StageState,
};

fn rate(hz: u32) -> SampleRate {
    SampleRate::new(hz).unwrap_or(SampleRate::HZ_48000)
}

fn stage(kind: StageKind, state: StageState, detail: &str) -> SignalStage {
    SignalStage {
        kind,
        state,
        detail: detail.into(),
    }
}

fn device(id: &str) -> DeviceSelection {
    DeviceId::new(id).map_or(DeviceSelection::SystemDefault, DeviceSelection::Device)
}

/// A path the engine could report with `lamp`:
///
/// - bit-perfect: FLAC 96 kHz / 24-bit to a USB interface held exclusively
///   through ALSA, every stage idle;
/// - processed: the same file and device with ReplayGain, the equalizer, the
///   limiter and the volume at work;
/// - resampled: the same file into a shared PipeWire device at 48 kHz, with
///   six stages at work (the design's DSP screen);
/// - lossy: an MP3 decoded to float and sent as 16-bit to an HDMI output.
pub fn signal_path(lamp: SignalLamp) -> SignalPath {
    use StageKind as K;
    use StageState as S;
    let le = Endianness::Little;
    let flac = SignalSource {
        spec: AudioSpec::new(SampleFormat::S32(le), rate(96_000), ChannelLayout::STEREO),
        bits: 24,
        codec: Some("flac".into()),
    };
    let studio = SignalDevice {
        backend: OutputBackendKind::Alsa,
        device: device("hw:CARD=Studio,DEV=0"),
        description: Some("Studio Interface".into()),
        mode: OutputMode::Exclusive,
        format: flac.spec,
        device_rate: None,
        exact_bits: None,
        buffer_ms: 200,
    };
    match lamp {
        SignalLamp::BitPerfect => SignalPath {
            item: ItemId::new(1),
            source: flac,
            stages: vec![
                stage(K::Decoder, S::Bypassed, "flac"),
                stage(K::ChannelMix, S::NotNeeded, "stereo"),
                stage(K::Loudness, S::Off, ""),
                stage(K::Resampler, S::NotNeeded, "96 kHz"),
                stage(K::Crossfade, S::NotNeeded, ""),
                stage(K::Equalizer, S::Bypassed, ""),
                stage(K::Crossfeed, S::Bypassed, ""),
                stage(K::ChannelMixer, S::Bypassed, ""),
                stage(K::Limiter, S::Bypassed, ""),
                stage(K::Volume, S::External, "fixed 100 %"),
                stage(K::Dither, S::NotNeeded, ""),
                stage(K::Conversion, S::NotNeeded, "s32le → s32le"),
            ],
            device: studio,
            lamp,
        },
        SignalLamp::LosslessProcessed => SignalPath {
            item: ItemId::new(2),
            source: flac,
            stages: vec![
                stage(K::Decoder, S::Bypassed, "flac"),
                stage(K::ChannelMix, S::NotNeeded, "stereo"),
                stage(K::Loudness, S::Active, "ReplayGain -6.30 dB (album)"),
                stage(K::Resampler, S::NotNeeded, "96 kHz"),
                stage(K::Crossfade, S::NotNeeded, ""),
                stage(K::Equalizer, S::Active, "EQ 5 filters · preamp -4.5 dB"),
                stage(K::Crossfeed, S::Off, ""),
                stage(K::ChannelMixer, S::Off, ""),
                stage(K::Limiter, S::Active, "limiter -1.0 dBTP · lookahead 2 ms"),
                stage(K::Volume, S::Active, "volume 85 %"),
                stage(K::Dither, S::Active, "TPDF dither 24-bit"),
                stage(K::Conversion, S::NotNeeded, "s32le → s32le"),
            ],
            device: studio,
            lamp,
        },
        SignalLamp::Resampled => SignalPath {
            item: ItemId::new(3),
            source: flac,
            stages: vec![
                stage(K::Decoder, S::Bypassed, "flac"),
                stage(K::ChannelMix, S::NotNeeded, "stereo"),
                stage(K::Loudness, S::Active, "ReplayGain -6.30 dB (album)"),
                stage(K::Resampler, S::Active, "resample 96 → 48 kHz (high)"),
                stage(K::Crossfade, S::NotNeeded, ""),
                stage(K::Equalizer, S::Active, "EQ 5 filters · preamp -4.5 dB"),
                stage(K::Crossfeed, S::Bypassed, ""),
                stage(K::ChannelMixer, S::Bypassed, ""),
                stage(K::Limiter, S::Active, "limiter -1.0 dBTP · lookahead 2 ms"),
                stage(K::Volume, S::Active, "volume 85 %"),
                stage(K::Dither, S::Active, "TPDF dither 24-bit"),
                stage(K::Conversion, S::Bypassed, "s32le → f32le"),
            ],
            device: SignalDevice {
                backend: OutputBackendKind::PipeWire,
                device: device("alsa_output.pci-0000_00_1f.3.analog-stereo"),
                description: Some("Built-in Audio".into()),
                mode: OutputMode::Shared,
                format: AudioSpec::new(
                    SampleFormat::F32(le),
                    SampleRate::HZ_48000,
                    ChannelLayout::STEREO,
                ),
                device_rate: None,
                exact_bits: Some(24),
                buffer_ms: 200,
            },
            lamp,
        },
        SignalLamp::Lossy => SignalPath {
            item: ItemId::new(4),
            source: SignalSource {
                spec: AudioSpec::new(
                    SampleFormat::F32(le),
                    SampleRate::HZ_44100,
                    ChannelLayout::STEREO,
                ),
                bits: 32,
                codec: Some("mp3".into()),
            },
            stages: vec![
                stage(K::Decoder, S::Active, "mp3"),
                stage(K::ChannelMix, S::NotNeeded, "stereo"),
                stage(K::Loudness, S::Bypassed, "0 dB"),
                stage(K::Resampler, S::NotNeeded, "44.1 kHz"),
                stage(K::Crossfade, S::NotNeeded, ""),
                stage(K::Equalizer, S::Off, ""),
                stage(K::Crossfeed, S::Off, ""),
                stage(K::ChannelMixer, S::Off, ""),
                stage(K::Limiter, S::Off, ""),
                stage(K::Volume, S::Bypassed, "100 %"),
                stage(K::Dither, S::Active, "TPDF dither 16-bit"),
                stage(K::Conversion, S::Active, "f32le → s16le"),
            ],
            device: SignalDevice {
                backend: OutputBackendKind::Alsa,
                device: device("hw:CARD=HDMI,DEV=3"),
                description: Some("HDMI / DisplayPort 1".into()),
                mode: OutputMode::Exclusive,
                format: AudioSpec::new(
                    SampleFormat::S16(le),
                    SampleRate::HZ_44100,
                    ChannelLayout::STEREO,
                ),
                device_rate: None,
                exact_bits: None,
                buffer_ms: 200,
            },
            lamp,
        },
    }
}

/// Counters of a stream that plays well: 184 of 200 ms buffered, no underruns.
pub fn playback_stats() -> PlaybackStats {
    PlaybackStats {
        buffer_ms: 200,
        buffered_ms: 184,
        underruns: 0,
        frames_delivered: 96_000 * 102,
        silent_frames: 0,
        total_underruns: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{SignalState, output_bits};

    #[test]
    fn each_lamp_has_a_path_that_says_so() {
        for lamp in [
            SignalLamp::BitPerfect,
            SignalLamp::LosslessProcessed,
            SignalLamp::Resampled,
            SignalLamp::Lossy,
        ] {
            let path = signal_path(lamp);
            assert_eq!(path.lamp, lamp);
            assert_eq!(
                SignalState::of(&path) == SignalState::BitPerfect,
                lamp == SignalLamp::BitPerfect
            );
        }
        let bit_perfect = signal_path(SignalLamp::BitPerfect);
        assert_eq!(bit_perfect.active_stages().count(), 0);
        assert_eq!(output_bits(&bit_perfect), 24);
        let resampled = signal_path(SignalLamp::Resampled);
        assert_eq!(resampled.active_stages().count(), 6);
        assert_eq!(output_bits(&resampled), 32);
        assert_eq!(
            SignalState::of(&resampled),
            SignalState::Processed { dsp: true }
        );
        assert_eq!(
            SignalState::of(&signal_path(SignalLamp::Lossy)),
            SignalState::Processed { dsp: false }
        );
    }
}
