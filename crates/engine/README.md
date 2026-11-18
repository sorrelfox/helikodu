# helikodu-engine

Decoder registry and gapless trimming, the render thread and processing path (conversion, gain, DSP, crossfade, resampling), the output backends (PipeWire, PulseAudio, ALSA, JACK, null, WAV) and the transport state machine. The engine plays media sources; it knows nothing about the library.

- **May depend on:** `model`, `ffmpeg-bridge`, `extra-decoders`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Decoding

`src/decode.rs` defines the `Decoder` and `DecoderFactory` traits; `src/decode/` holds the registry, the backends and the trimming of sections.

```rust
use helikodu_engine::decode::{DecoderRegistry, PcmBlock};
use helikodu_model::library::MediaSource;

let registry = DecoderRegistry::with_default_backends();
let mut decoder = registry.open(&MediaSource::file(path))?;   // DecodeError → EngineError
let mut block = PcmBlock::new();
while decoder.read(&mut block)? {
    // block.spec(), block.position(), block.samples(), block.append_f32(&mut out)
}
```

| Type | What it is |
|---|---|
| `Decoder` | One source being decoded: `info()`, `spec()`, `duration()`, `position()`, `read()` into a `PcmBlock`, `seek()` to an exact `MediaTime` |
| `DecoderInfo` | Backend, container, codec, significant bits, the tags the backend reads with the audio, the file's audio streams and the one decoded, the subsong, whether the samples are DoP, whether the file was read into memory |
| `DecoderFactory` | A backend: `name()`, `probe()` returning a `ProbeScore`, `open()` a path (and a subsong); `open_input()` a `SourceInput` (path, and the bytes when buffered) with an `OpenRequest` (subsong, audio stream) |
| `Probe`, `ProbeScore` | The path and first 4 KiB of the file; `NONE`, `FALLBACK`, `EXTENSION`, `SIGNATURE` |
| `DecoderRegistry` | The backends and the choice of one for a `MediaSource`; `open_stream()` with an audio stream; `configure()` the decoder settings, `set_dop()`, `set_memory_limit()` |
| `DecodeOptions`, `SharedOptions` | The decoder settings, DoP and the buffering limit, shared between the registry and its backends |
| `SongFactory`, `SongLibrary` | The synthesizing backends over `extra-decoders` |
| `PcmBlock`, `BlockSamples` | One block of interleaved samples with its `AudioSpec` and position |
| `DecodeError` | Everything that can fail; `to_engine_error()` and `From` map it to the model's `EngineError` |

**Choosing a backend.** The registry reads the first 4 KiB of the file and asks each backend for a score. Backends are tried from the highest score down, on ties in registration order, and the first that opens the file wins; when none does, `DecodeError::NoDecoder` lists each backend's reason. File names never decide: FFmpeg probes the content itself and scores every file `FALLBACK`; the DSD backend claims files with the DSF or DSDIFF signature, libopenmpt and libgme files their own header probes accept, the SID backend `PSID`/`RSID` and the MIDI backend `MThd` or RIFF `RMID` (all `SIGNATURE`). vgmstream is the exception: most of its formats have no signature, so it scores the files vgmstream would try by name `EXTENSION`, checks the content when it opens them, and leaves what it does not recognise to FFmpeg. An unreadable file stops the search at once.

| Backend | Cargo feature | Files | Subsongs |
|---|---|---|---|
| `dsd` | `dsd` (default) | DSF and DFF, decoded to PCM by the reader in `extra-decoders`, or packed into DoP (below). DST-compressed DFF is refused there and falls through to FFmpeg | No |
| `openmpt` | `openmpt` (default) | Tracker modules through libopenmpt | The module's |
| `gme` | `gme` (default) | Game music through libgme | Tracks of multi-track files |
| `sidplayfp` | `sid` (default) | C64 SID tunes through libsidplayfp | The tune's songs |
| `rustysynth` | `midi` (default) | MIDI files with a SoundFont | No |
| `vgmstream` | `vgmstream` (off) | Game audio streams; needs `libvgmstream` (see `extra-decoders`) | Streams of a bank |
| `ffmpeg` | always built | Everything else FFmpeg decodes, through `ffmpeg-bridge` | No; several audio streams instead |

The synthesizing backends (`openmpt`, `gme`, `sidplayfp`, `rustysynth`, `vgmstream`) give `f32` stereo blocks at 44.1 kHz (vgmstream at the stream's rate) and put the file's title, artist, album, date and comment in `DecoderInfo::tags`; lengths, loops, fades and the SoundFont come from the `[playback.decoders]` settings (`DecoderRegistry::configure`, the `SetDecoderSettings` command). See the `extra-decoders` README for each library.

**Whole-file buffering.** With `load_into_memory` in the decoder settings (off by default), the registry reads a file of at most `DecodeOptions::memory_limit` bytes (512 MiB, `MEMORY_BUFFER_LIMIT`, which the settings never exceed) into memory before any backend opens it; FFmpeg then reads it through `MediaFile::open_bytes` and the synthesizing backends take the bytes. Larger files, DSD files and vgmstream (which opens companion files itself) read from the drive. `DecoderInfo::in_memory` says which happened.

**Audio streams.** Of a file with several audio streams FFmpeg decodes the one the item names (`PlaybackItem::audio_stream`, an index in the file) or else the default one, and lists them all in `DecoderInfo::streams`.

**Block format.** Blocks hold interleaved samples in WAVE channel order, in the format the source decodes to: 8-, 16- and 32-bit integers keep their exact values (24-bit sources sit in the top bits of 32-bit words, `DecoderInfo::bits_per_sample` says 24), floating-point sources are `f32`. The render side converts to the `f32` of the output ring buffer with `PcmBlock::append_f32`, which divides by 2^(bits − 1) and is exact for 8-, 16- and 24-bit values. Keeping integers until that point leaves room for a bit-perfect path that hands them to the device untouched.

**Gapless playback.** Encoder delay and padding are trimmed by the backend: FFmpeg, through the bridge, for MP3 (LAME/Info tags), AAC and Opus in MP4, Opus and Vorbis in Ogg and Matroska. Positions count frames from the first frame after the delay. `seek()` lands on the exact frame.

**Sections.** A `SourceSection::Span` (a CUE track, for example) is applied by the registry on top of any backend. Its times are converted to frames at the decoder's rate, rounding down, which is exact for CUE times; the decoder seeks to the start and blocks are cut at both ends, so consecutive spans of one file join without a gap or an overlap. Positions, `duration()` and `seek()` are relative to the span. The first block is decoded when opening, so a span that starts after the end of the file fails with `InvalidSection` instead of trusting the container's stated length, which is an estimate for some formats. `SourceSection::Subsong` is passed to the backend: the synthesizing backends open that subsong (one the file does not have fails to open), FFmpeg and the DSD backend fail with `SubsongUnsupported`.

**DSD.** DSF and DFF play as `f32` PCM at 176.4 kHz (DSD64 to DSD512; 192 kHz for the 48 kHz family), raised by 6 dB because the DSD 0 dB reference corresponds to −6 dBFS. See the `extra-decoders` README for the filter.

**DoP.** With `[output] dsd = "dop"` and the device in exclusive mode, the transport asks the registry for DoP (`set_dop`), and DSD files decode to DSD over PCM: 24-bit words with the alternating `0x05`/`0xFA` markers, in `i32` blocks with `bits_per_sample` 24, at the DSD rate divided by 16. A DoP source plays only on a path that leaves its samples untouched: no gain, channel mixing, resampling, crossfade or DSP, fixed or hardware volume, and a device format of 24 bits or more (`transport/formats.rs`, `keeps_dop`). On any other path the transport replaces it, before anything is rendered, by the same file converted to PCM at the same position (`Decoder::pcm_fallback`). `tests/long_tail.rs` checks every word through the WAV sink, and the fallback with software volume.

**Errors.** Everything that keeps a source from opening (`Io`, `Open`, `NoDecoder`, `SubsongUnsupported`, `InvalidSection`) becomes `EngineError::Unplayable`; a failure after opening (`Decode`) becomes `EngineError::Decode`.

Building the crate needs the FFmpeg development files (7.1 or newer), as for `ffmpeg-bridge`, and with the default features those of libopenmpt, libgme and libsidplayfp (pkg-config names `libopenmpt`, `libgme`, `libsidplayfp`) and a C++ compiler; `--no-default-features` with the features wanted leaves any of them out.

Not done yet in the decoding layer:

- **A per-format backend preference** (for example FFmpeg over vgmstream for ADX). The registry order is fixed.
- **DoP silence and DoP-aware fades.** During pauses and between tracks the device gets PCM silence, not the DoP idle pattern, and the anti-click fades of a pause or seek scale DoP words like PCM; a DAC may drop out of DSD mode for a moment there. A DoP stream that starts at the beginning of a track is not faded.
- **Native DSD output** (ALSA `DSD_U8`/`DSD_U16`/`DSD_U32`).
- **Stream choice that persists.** `SelectAudioStream` applies to the playing item; the choice is not stored for the file.

## Output

`src/output.rs` defines the `OutputBackend` and `OutputStream` traits; each backend has its own file in `src/output/`. Opening a stream negotiates the device format and returns a `SampleWriter`, the producer side of a lock-free ring buffer (`rtrb`) of interleaved `f32` samples. The backend's real-time side copies out of the ring buffer, runs the device-side stage (below) and converts to the device format; underruns are counted in atomics. When the negotiated format is 32-bit integer, the ring holds `i32` samples instead of floats (a float carries only 24 bits exactly): `SampleWriter::write_i32` takes integers exactly, `write` converts floats exactly as the encoder would, and while the device-side stage leaves the samples alone the integers reach the device without passing through a float (`SampleWriter::carries_integers`). Device changes, lost devices and failures arrive as `OutputEvent`s on a channel.

**The device-side stage** (`src/output/device_stage.rs`) applies software volume, the anti-click fades (the pipeline's `Fader`) and dither after the ring buffer, so that volume, pause, seek and stop are heard within one device period rather than after the whole buffer. The render thread controls it through the writer: `SampleWriter::volume()` is a `SharedVolume` (two atomics), and `SampleWriter::schedule(at, DeviceAction)` queues a fade, an open or shut fader, a change of dither, or a `Discard` of buffered frames on a second lock-free queue, each to apply at a ring position (frames written since the stream opened), so a fade starts on exactly the frame it is meant for. `is_silent()` and `silent_since()` tell the render thread when a fade out has finished and where. A `Discard` with `hold` drops the buffered audio and plays silence, without counting underruns, until the next fade in, so the buffer can be refilled after a seek. Until told otherwise the fader is open and the volume full, so samples pass unchanged. `StreamTiming` counts the discarded frames, and `heard_position()` gives the ring position being heard.

| Backend | Cargo feature | Device names |
|---|---|---|
| PipeWire | `pipewire` (default) | Node name, for example `alsa_output.usb-Topping_D10s-00.analog-stereo` |
| ALSA | `alsa` (default) | `hw:CARD=<card ID>,DEV=<n>`, and PCMs of the ALSA configuration such as `pipewire` |
| PulseAudio, through cpal | `pulseaudio` (default) | Sink name, for example `alsa_output.usb-Topping_D10s-00.analog-stereo` |
| JACK, through cpal | `jack` (default) | Name of the client whose input ports receive the audio, for example `system` |
| Null | always built | `null` |
| WAV file | always built | The file name |

**The default device.** Each backend marks the device "System default" plays to with `DeviceInfo::is_default`. PipeWire follows `default.audio.sink` in the session manager's `default` metadata; the backend reports itself connected only after a second round trip to the daemon, so the mark is there in the first device list. ALSA opens the `default` PCM without starting it: when it leads to a card (directly or through `plug`/`dmix`), that card's `hw:` device is the default; when it is a sound server's plugin, the server's PCM (`pipewire`, `pulse`) is; when it is busy, the first card's first device.

Building the native backends needs the PipeWire and ALSA development files (`pkg-config` names `libpipewire-0.3` and `alsa`) and `libclang` for the PipeWire bindings; the JACK backend needs `jack.pc` (Fedora: `pipewire-jack-audio-connection-kit-devel`, Debian and Ubuntu: `libjack-jackd2-dev`, Arch: `pipewire-jack`), although `libjack` is only loaded at run time. Without the package, a `jack.pc` of your own pointing at the installed `libjack.so` (through `PKG_CONFIG_PATH`) is enough to build. The PulseAudio backend speaks the protocol itself and needs no library. `--no-default-features` builds the crate without any of them.

**PulseAudio and JACK** (`src/output/pulse.rs`, `src/output/jack.rs`) go through cpal 0.18; `src/output/cpal_stream.rs` holds their common stream (the callback copies through the device-side stage, converts, and records the delay cpal predicts until the first frame is heard). cpal has no stream volume, no exclusive mode and no device notifications: the engine applies software volume, exclusive mode opens the device as in shared mode, and a `DeviceWatcher` (`src/output/device_watch.rs`) lists the devices every two seconds (every 250 ms while a stream plays to a chosen device), sends `DeviceAdded`, `DeviceRemoved` and `DefaultDeviceChanged`, and marks a stream lost when its device is no longer listed; from then on the stream writes silence.

- *PulseAudio:* the server converts rate, channels and format, so the request is opened as it is (formats cpal cannot pass become the nearest one it can). Every stream has a fixed buffer of about 20 ms: with the server's default, its first requests are almost two seconds long and empty the ring buffer. "System default" opens the default sink as it is when the stream opens. A sink that goes away is noticed within 250 ms; the server may have moved the stream to another sink by then, which then gets silence.
- *JACK:* devices are the clients with audio input ports; "System default" is `system`, or the first client with physical ports. Streams run at the server's rate with float samples (the engine resamples), with as many channels as the request and the device both have. cpal connects a new stream to `system:playback_*` when it builds it, and its option against that does not work; the backend removes those links before the stream starts (it plays only silence until then) and connects the ports to the chosen client with a JACK client of its own (`helikodu-ports`). On PipeWire, `PIPEWIRE_PROPS='{ jack.self-connect-mode = ignore-all }'` in the program's environment makes the server ignore cpal's links, so not even silence reaches the default device; the backend's own links are not affected. A client that goes away loses the stream; JACK has removed its links, so it is silent.

**Automatic backend:** PipeWire when its socket is there, otherwise PulseAudio when its socket is there, otherwise ALSA. JACK is only used when chosen.

**Backends of the program's own.** `EngineConfig::backend_connector` takes a `BackendConnector` that connects backends in place of the built-in ones of the same kind; the device tests use it to script devices that go away.

The `alsa` crate is pinned to the version cpal uses (0.11 with cpal 0.18): two versions of `alsa-sys` cannot be linked into one program, so both are upgraded together.

**Device capabilities.** Every device in the list carries the sample rates, formats and channel counts it takes, as far as the backend can tell: ALSA probes the hardware parameters, PipeWire reads each sink node's `EnumFormat` parameters (for a sound card, the formats of its ALSA device; a range of rates becomes the common rates within it) and reports a device again when they arrive after it was listed, PulseAudio and JACK report what their servers take.

**Sample rate and format.** The stream is asked for the rate of the sample-rate policy (`stream_config`): the track's rate for "match the track", a fixed rate (the engine resamples to it), or, for "device default", no rate at all. PipeWire asks the graph for the rate with `node.rate` in shared mode, which switches it only within the user's allowed rates, and with `node.force-rate` in exclusive mode, which switches it; the stream's format is always 32-bit float, and the rate the graph really runs at is reported (`StreamTiming::device_rate`), so the signal path shows the server's resampling. ALSA opens `hw:` devices in exclusive mode without alsa-lib's rate converter, at the track's rate when the device has it and at its nearest rate otherwise (the engine resamples, and the signal path says so); "device default" takes 48 kHz or 44.1 kHz when the device has them. Each track whose rate or format differs reopens the device (see Gapless joins). The format follows the format setting (`s16`, `s24`, `s24_3`, `s32`, `f32`, or the source's own for `auto`); ALSA falls back to the widest format the device has.

**Device reservation.** A sound server usually holds every card, so a `hw:` device is busy. The ALSA backend takes the card through a `DeviceReservation` (`src/output/reservation.rs`) that the program running the engine injects, with `EngineConfig::device_reservation`, `AlsaBackend::with_reservation` or `connect_backend_with`. This crate does not speak D-Bus: `helikodu-desktop` implements the protocol (`org.freedesktop.ReserveDevice1`). In exclusive mode the backend takes the card before opening a `hw:` or `plughw:` device; in shared mode only when the device is busy. After taking it, it retries a busy device for up to one second while the previous holder closes it. The stream keeps the `ReservationGuard` until it is stopped or dropped (which the transport does on stop and after `exclusive_release_s` of pause) and drops it after closing the device. A holder that refuses gives `OutputError::DeviceBusy` naming it; without a reservation (the default, `NoReservation`) or when reservation fails, the device is opened anyway and a busy one is reported busy, naming the program that holds it. The guard reports whether someone has asked for the card back (`release_requested`); the engine does not act on it yet.

Not done yet in the output layer:

- **Real-time priority for the ALSA writer thread** (rtkit or the Realtime portal). PipeWire's process callback already runs on PipeWire's real-time thread.
- **ALSA hot-plug through udev.** The ALSA backend polls `/proc/asound/cards` every two seconds instead.
- **A headphone jack on a card that stays.** Unplugging headphones from a sound card whose device stays (only its port changes) is not noticed; pausing on unplug covers devices that go away (USB, Bluetooth).
- **Following the default sink on PulseAudio.** A stream on "System default" stays on the sink that was the default when it opened, until it is opened again.

## Processing

`src/pipeline.rs` and `src/pipeline/` hold the processing path between the decoders and the output backends. For each track a `PathPlan` decides, from the source format, its loudness information, the settings and the format the device was opened with, which stages run. When none is needed (no gain, no channel mixing, no resampling, no DSP, no crossfade, the volume at 100 % or outside the engine, and a device format that carries the samples exactly) the plan is bit-perfect and the samples reach the device unchanged. The plan is plain data and prints as a signal path, for example `96 kHz/24-bit 5.1 → downmix 5.1 → stereo → ReplayGain -6.30 dB (track) → resample 96 → 48 kHz (high) → volume 85 % → TPDF dither 16-bit → s16le 48 kHz stereo`.

| Stage | Type | Notes |
|---|---|---|
| Conversion to float | `to_float` | Integers as `n / 2^(bits-1)`: exact up to 24 bits |
| Channel mapping, downmix, upmix | `MixMatrix` | ITU-R BS.775 coefficients (centre and surrounds at −3 dB), LFE dropped unless included, downmix scaled so it cannot clip |
| Loudness gain | `resolve_gain`, `resolve_track_gain`, `GainStage` | ReplayGain 2.0 reference of −18 LUFS; Opus R128 tags become `r128 / 256 + 5` dB; track, album and automatic modes, preamps for tagged and untagged files, the track's own volume adjustment, clipping prevention by peak |
| Resampling | `ResampleStage` | rubato behind the crate's own `Resampler` trait: FFT for rate pairs with a small common divisor, windowed sinc otherwise; four quality presets; start-up delay removed, `n` frames become `ceil(n × to / from)` |
| Software volume | `SoftwareVolume`, `SharedVolume` | `a = p³`, 20 ms linear ramps, mute |
| Anti-click fades | `Fader`, `FadeDurations` | Raised-cosine fades on start, pause, resume, seek, stop, skip and device switch: 10 ms (5 ms around a seek) by default, the lengths of `[playback.transitions]` through `FadeDurations::from_settings` |
| Crossfade | `equal_power` | Outgoing gain `cos(πt/2)`, incoming `sin(πt/2)`, so the squares add up to one; mixed by the transport (see Transport) |
| Dither | `TpdfDither` | Triangular ±1 LSB to 8, 16 or 24 bits, only while the samples are processed |
| Device format | `Encoder` | Run by the output backends; 32-bit integer samples pass to a 32-bit format exactly (`encode_s32`) |

`SourceChain` (mixing, gain, resampling; one per source) and `OutputChain` (volume, fades, dither; on the mixed stream) build the stages from a plan. Samples are interleaved 32-bit floats in canonical channel order; gains and filter state are 64-bit. Every stage allocates when it is built and never while processing; `tests/pipeline_allocations.rs` checks this with a counting allocator.

`rubato` is pinned to an exact version because its interface changes often; only `src/pipeline/resample/rubato_backend.rs` uses it.

Not done yet in the processing path:

- **`OutputChain` is not used by the transport**: its stages run on the device side of the ring buffer instead (see Output).
- **True peaks from a scan.** The loudness values come with the `PlaybackItem` (its `Loudness`, filled by the service from the file's tags); tags carry sample peaks, so `true_peak` is always off until the loudness scanner exists.

## DSP chain

`src/dsp.rs` and `src/dsp/` hold the DSP chain, which runs on the render thread on the joined stream, at the device's rate and channels, before the ring buffer. The order is fixed: equalizer, crossfeed, channel mixer, limiter; dither stays on the device side (above) and runs whenever the chain changes the samples.

| Stage | Type | Notes |
|---|---|---|
| Equalizer | `Equalizer` | Preamp (automatic: the largest boost as a cut), then biquads with the RBJ cookbook coefficients (`Coefficients` and `graphic` live in `helikodu_model::eq`, which the user interface draws its response with; `Cascade`; 64-bit state, transposed direct form II). Graphic: 10 octave or 31 third-octave bands at the ISO centres, constant Q, gains corrected for the overlap of neighbouring bands (`graphic::corrected_gains`, Newton's method on the exact responses) so that the response meets each slider within 0.05 dB. Parametric: up to 64 peaking, shelving and pass filters. Filters above 0.475 of the rate are left out |
| Crossfeed | `Crossfeed` | Bauer stereophonic-to-binaural (as in bs2b): low pass of the opposite channel, matching high boost of the direct one; presets Default (700 Hz, 4.5 dB), Chu Moy (700 Hz, 6 dB), Jan Meier (650 Hz, 9.5 dB) or custom. Stereo only |
| Channel mixer | `ChannelMixer` | Mono, left/right swap and balance on the front pair |
| Limiter | `Limiter` | True-peak lookahead limiter, linked channels: peaks between samples from an 8× Kaiser-windowed-sinc interpolator, sliding minimum, exponential release (60 ms) and a moving average as long as the lookahead (1 to 5 ms), with the audio delayed so that the gain has settled before a peak and holds around it. Aims 0.2 dB under the ceiling. `LimiterActivity` gives the reduction now, the largest and how often it limited |

`DspChain` builds the stages from the model's `DspSettings` for one rate and channel count. A stage that is off, or that changes nothing (a flat equalizer, centred stereo), is not built, and with no stage left `process` does not touch the samples, so the path stays bit-perfect. `set_settings` builds new stages, runs them over the last 100 ms of the chain's input so that their filters and delay line are settled, and crossfades to them over 20 ms (raised cosine); changes that arrive during a crossfade wait for it, the latest one winning. A guard at the end of the chain turns NaN, infinities and subnormals into zero. Building allocates; processing never does. `DspChain::plan()` gives a `DspPlan`, which `PathPlan::with_dsp` puts into the plan, so the stages appear in the signal path (`EQ 5 filters · preamp -4.5 dB → crossfeed 700 Hz · 4.5 dB → limiter -1.0 dBTP · lookahead 2 ms`).

`dsp::autoeq` reads the EqualizerAPO `ParametricEQ.txt` syntax AutoEQ publishes (`PK`/`PEQ`, `LS`/`LSC`, `HS`/`HSC`, `LP`/`LPQ`, `HP`/`HPQ`, `BW Oct`, several `Preamp` lines) into the settings' filters, clamping values into range and skipping other lines with warnings, and writes filters back in the same syntax. `fuzz/` has a target for it. `dsp::processing_settings` takes the resampler quality (`[dsp] resampler`) and the LFE option of downmixes from the settings into the `ProcessingSettings`.

In the transport the chain lives as long as the stream format (a gapless join keeps it, a new rate builds a new one), is reset on every new start (seek, resume, load), and plays out the limiter's lookahead at the end of the audio, so nothing is lost.

## Transport

`src/transport.rs` and `src/transport/` join decoding, processing and output into a playing engine. `Engine::spawn(EngineConfig)` starts the engine thread and returns the handle and the receiver of `EngineEvent`s; `Engine::send` (or a cloned `EngineSender`) takes the model's `EngineCommand`s. There is no async runtime: one thread does control and rendering, and a second one forwards the backends' events to it.

```rust
use helikodu_engine::transport::{Engine, EngineConfig};

let (engine, events) = Engine::spawn(EngineConfig::default())?;
engine.send(EngineCommand::Load { item, start: MediaTime::ZERO, paused: false })?;
for event in events.iter() {
    if let EngineEvent::NearEnd { .. } = event {
        engine.send(EngineCommand::SetNext(next_item()))?;
    }
}
```

| File | What it holds |
|---|---|
| `transport.rs` | `Engine`, `EngineSender`, the state and event rules |
| `transport/config.rs` | `EngineConfig`: output settings, processing settings, start volume, the pacing of the test sinks, `NearEnd` lead time (5 s), position interval (100 ms), the device reservation |
| `transport/worker.rs` | The engine thread's loop, state and events |
| `transport/commands.rs` | Load, play, pause, stop, seek, next item, volume, output |
| `transport/settings.rs` | Output and loudness settings changed while running |
| `transport/render.rs` | Filling the ring buffer, joins, draining, the fades and refills around transitions |
| `transport/recovery.rs` | Settling after a transition; a failed output or a lost device |
| `transport/source.rs` | One item: decoder and `SourceChain` |
| `transport/crossfade.rs` | Crossfades between tracks and on skips, their policies, and the fall and rise when the next track needs another stream |
| `transport/timeline.rs` | Which item each part of the ring buffer belongs to, and the heard position |
| `transport/device.rs` | The connected backend, the open stream, and opening one for an item |
| `transport/signal.rs` | The signal path of the heard item and the playback counters, as events |
| `transport/ab_loop.rs` | The A-B loop of the loaded item |

**How it plays.** While playing, the thread wakes every few milliseconds (a sixteenth of the buffer, 2 to 10 ms), writes at most one buffer length, follows the heard position and sends what is due. While stopped or paused it sleeps until a command arrives. Each block of work is bounded, and rendering reuses its buffers, so the steady state does not allocate; opening an item, a join and a transition do.

**Transitions.** Pause, seek, stop, a device switch and a `Load` that replaces playing audio (a skip) fade out on the device side (for the lengths, see Crossfades and fades), wait until the device side reports silence, and take the ring position where it began as the point heard last. Pause continues from exactly that point; seek, resume and a device switch drop the buffer (`Discard` with `hold`), seek the decoder, refill about 50 ms and fade in at the first new frame. A track that starts at its beginning is not faded in. Resuming after a pause re-decodes from the paused position instead of playing the stale buffer.

**Settings while running.** `UpdateSettings { output, loudness }` replaces the `[output]` and `[loudness]` settings (the backend, device and mode in `output` are ignored; `SetOutput` chooses them). A new sample rate policy, sample format, buffer length or volume control needs another stream: while playing, the engine reopens it at the heard position the way a device switch does (`Reconfiguring`, fade out, fade in); while paused it closes the stream and `Play` opens the new one. A loudness change (mode, preamps, clipping prevention) keeps the stream and takes effect at once: the current item's gain ramps over 20 ms to its new value from the next frame rendered (a chain without a gain stage gets one, starting at unity), so it is heard after the audio already buffered, at most the buffer length; the resampler's state is kept. Later items, and the current one after a seek or resume, are planned with the new settings. The device-lost policy and the release time of a paused exclusive device apply from then on. `UpdateSettings` also carries the `[dsp]` settings: the chain crossfades to its new stages on the audio rendered from then on, and the current item's plan follows (its signal path, and dither on the device side); the resampler quality and the LFE option apply to items planned later.

**Gapless joins.** `SetNext` opens the next item at once (errors are reported then). When the current item ends, the next one continues in the same render iteration if the settings would open the same stream for it (`stream_config` gives the same rate, format and channels); the samples join exactly. Otherwise the buffer plays out and the device is reopened (`Reconfiguring`). The timeline records where each item starts in the ring buffer, so `TrackEnded` and `TrackStarted` are sent when the join is heard. A `SetNext` that replaces an item already written after the heard one drops that part of the buffer with a `Discard` at its start; when that item was being crossfaded in, the heard item is rendered again from the start of the crossfade (or, once the crossfade is heard, the jump gets a 10 ms dip).

**Long-tail formats.** `SetDecoderSettings` replaces the `[playback.decoders]` settings (loops, fades, SoundFont, SID song lengths and ROMs, tracker rendering, whole-file buffering) for the items opened from then on; `Engine::spawn_with_registry(config, DecoderRegistry::with_settings(..))` starts with them. `SelectAudioStream(Some(index))` plays another audio stream of the loaded item's file from the heard position, as a seek does (while paused, the item is opened with the stream at once, so a stream that does not open is reported now). When an item whose file has several audio streams starts, `TrackStarted` is followed by `AudioStreams { item, streams, playing }`, and again after `SelectAudioStream`. `transport/formats.rs` holds these and the DoP check.

**Crossfades and fades.** `EngineConfig::transitions` and `SetTransitions` take the model's `TransitionSettings` (`[playback.transitions]`); `EngineConfig::with_transitions` also puts its fade lengths into `processing.fades`: pause and resume, stop, seek and skip (100, 200, 50 and 100 ms in the settings' defaults; the engine's own default stays 10 ms, 5 ms around a seek). The fade in at a start position and the fades around a device switch stay 10 ms. Crossfades are off by default.

- *Between tracks.* The render side mixes the next item into the last frames of the current one, after each item's gain and resampling and before the DSP chain, with equal-power gains. The window has the setting's length, shortened to half of either track for short tracks and to what is left when the next item comes late; it begins where the current item has that many frames left, which `Source::remaining_frames` gives exactly from its length (resampling makes `n` frames `ceil(n × to / from)`). `NearEnd` comes earlier by the crossfade's length, and a freewheeling sink waits at the window for the answer. `TrackEnded` (`Finished`) and `TrackStarted` are sent when the middle of the crossfade is heard; the incoming item's positions count from its first frame, at the start of the window. Dither runs during the crossfade.
- *Policies.* No crossfade between consecutive items of one album (`PlaybackItem::album`, the same key) while `keep_albums_gapless` is set, on a bit-perfect path in exclusive mode, or for an item of unknown length: they join gaplessly, bit-exact. When the next item needs another stream, the current one falls over the window instead (equal-power, to silence), the stream is reopened, and the next one fades in as long on the device side. The integer path of 32-bit sources stays off while a crossfade can begin.
- *Skips.* A `Load` while playing is a skip. With the skip crossfade on and the same stream for the new item, the engine crossfades from the end of the audio already buffered (at most the buffer length) over the skip length; `TrackEnded` (`Interrupted`) and `TrackStarted` come at once, without `Loading`, and the new item's position counts from its first frame. Otherwise the playing item fades out over the skip fade.
- A pause, seek, stop or device switch ends a crossfade: playback continues with the item heard, and the crossfade into the next one begins again, shortened, if there is time left.

**A-B loop.** `SetLoop { item, range }` loops a section of the loaded item (the one heard, or paused at), or ends the loop with `None`; it is ignored for any other item or an empty range. The source cuts each decoded block that starts before the loop's end and reaches it, and once the chain has taken that block it moves the decoder back to the start without resetting the chain, so the start follows the end as the next frame, without a gap; the end of the source inside the loop moves back too (unless nothing was decoded since the last move). Each move back is a rewind of the item's timeline segment, so positions stay inside the loop. When a loop is set, changed or ended while playing, the audio already rendered is compared with it: from the first ring position where they part (the end passed, a move back that no longer applies, the next item already joined or crossfaded in) the buffer is dropped with a `Discard`, the next item goes back to being next, and the item is rendered again from there; a heard position at or past the end jumps to the start at once (while paused, the position moves there). A seek past the end leaves the loop: blocks after the end are not cut. While the loop holds, `NearEnd` is not sent for the item and no crossfade is planned into the next one. The loop ends with the item, and is given again to the item's source after a pause, seek or device switch.

**Volume for this track.** `PlaybackItem::track_volume_db` is added to the loudness gain before clipping prevention (`resolve_track_gain`); with loudness normalization off it is the gain on its own (`track volume -3.00 dB` in the signal path). Zero adds nothing, so a bit-perfect path stays bit-perfect.

**Position.** The heard ring position is the frames the device side has taken (played or dropped) minus the delay the backend reports; the timeline turns it into the item and its position, at the source rate.

**Device profiles.** Each device can have a profile in `[output.profiles."<backend>:<device>"]` (and "System default" one under `<backend>:default`); `OutputSettings::for_device` applies it. The engine opens each stream with the profile of its device: mode, sample rate policy, sample format, buffer length, volume control and the release time of a paused exclusive device. Switching to another device, falling back to the default one, and a changed profile (through `UpdateSettings`) apply the other profile; the stream is reopened when it changes the stream. A fixed volume is only applied to a device whose profile records the user's confirmation; otherwise the volume is applied in software.

**Bit-perfect playback.** A plan that changes nothing (see Processing) plays bit-perfect. 16- and 24-bit integer and 32-bit float samples pass the float ring exactly. A 32-bit integer source whose plan changes nothing on the render side (no gain, channel mixing, resampling, crossfade or DSP) goes from the decoder into the ring as integers (`Source::render_exact`); a 32-bit device format's ring carries them to the device exactly. Both paths take from the same decoded block, so a change of settings moves playback between them at the next frame.

**DAC warm-up.** A stream that is opened anew (the first item, a new rate or format, another device, resuming after a released pause) starts with `preroll_ms` of digital silence; the item starts after it, so positions are not shifted.

**Exclusive devices.** A stream in exclusive mode is closed on stop and after `exclusive_release_s` seconds of pause (30 by default, 0 for never), which gives the card back to the sound server (the ALSA backend drops its `ReservationGuard` after closing the device); `Play` opens it again, with the warm-up.

**Signal path and statistics.** The engine sends `EngineEvent::SignalPath` with the path of the item being heard whenever it changes: another item heard (at a gapless join, when the join is heard), a new plan (gain, DSP settings, crossfade), another stream or device format, another volume, or a sound server's graph moving to another rate. The model's `SignalPath` has the source (format, significant bits, codec), every stage in order (decoder, channel mixing, loudness, resampler, crossfade, equalizer, crossfeed, channel mixer, limiter, volume, dither, conversion), each `Active`, `Bypassed`, `Off`, `NotNeeded` or `External` with its values, the device (backend, description, mode, the negotiated format, the device's own rate when a server resamples, the bits a server carries exactly) and a lamp: `BitPerfect`, `LosslessProcessed`, `Resampled` or `Lossy` (the device gets fewer bits than the source has), the worst that happens. Only exclusive PipeWire streams, ALSA `hw:` devices and the file sinks can be bit-perfect; on PipeWire up to 24 bits. `SignalPath(None)` comes before `Stopped`. While playing, `EngineEvent::PlaybackStats` reports the buffer length and fill, underruns, delivered and silent frames of the open stream four times a second. Both are built in `transport/signal.rs` from inputs compared without allocating.

**Unplugged devices.** A stream that follows "System default" is moved by the sound server when its device goes away. When the device it plays to is removed (headphones, a USB or Bluetooth device unplugged), playback pauses if `pause_on_unplug` is set (the default), instead of going on through the next default device. A stream on a chosen device that goes away is lost instead, and `on_device_lost` applies; every backend reports it (PipeWire through the stream state, ALSA through its errors, PulseAudio and JACK through the device watcher).

**Errors.** An item that does not open (on `Load` or `SetNext`) sends `Error` and `TrackEnded` (`Failed`) at once; for `SetNext`, `NearEnd` follows again so the service can name the next item. A decoding error midway ends the item there; its `Error` and `TrackEnded` (`Failed`) come when that point is heard, and playback continues with the next item. A lost or failed stream applies `on_device_lost`: pause (the default), continue on the default device, or stop. A device missing at start-up falls back to the default one under `fallback-default`; otherwise the engine reports the error and pauses (or stops).

**Freewheeling sinks.** With `EngineConfig::sink_pacing` set to `Pacing::Freewheel`, the null and WAV sinks take audio as fast as it is rendered. There is no deadline then, so at the end of an item the engine waits (up to `next_wait`, 5 s) for the answer to `NearEnd` instead of draining at once; tests and rendering to a file get the same joins as real playback.

`examples/play.rs` plays one file to a chosen backend and device and prints the events; lines on standard input pause, resume, seek, change the volume and switch the output:

```sh
cargo run -p helikodu-engine --example play -- song.flac --backend pipewire --device <node name>
cargo run -p helikodu-engine --example play -- --list-devices --backend alsa
```

Not done yet in the transport:

- **Opening the next item happens on the engine thread.** It takes milliseconds from local storage; slow storage would delay the buffer by as much.
- **The engine thread's priority** is not raised yet (it needs the same rtkit or portal route as the ALSA writer).
- **A hardware mixer** (`volume = "hardware:…"`) is not driven; the engine applies software volume instead.
- **DSP details not done yet:** the limiter's activity is not reported as events (`DspChain::limiter_activity()` holds it); the user's graphic presets are stored but not imported or exported as files; per-device DSP presets; the positions reported do not subtract the limiter's few milliseconds of delay.
- **`NearEnd` for items of unknown length** comes only when the item has been decoded to its end, about one buffer length before it ends.
- **`Engine::stats()`** gives the output counters (streams opened, frames delivered, underruns, silent frames) summed over every stream, lock-free, for diagnostics and tests.

## Analysis

`src/convert.rs` converts a source for the converter: `transcode` decodes it through the registry (CUE sections and DSD included), downmixes with `MixMatrix` when the target has fewer channels, resamples with the playback resampler when the target rate differs (Opus always at 48 kHz) and encodes through the bridge's `AudioEncoder`; lossless targets of integer sources that need neither stay exact.

`src/analysis.rs` and `src/analysis/` measure whole sources away from playback. The loudness scanner (`analysis::loudness`) follows EBU R128 and ITU-R BS.1770-4 with the `ebur128` crate: K-weighting, 400 ms gating blocks with an absolute gate at −70 LUFS and a relative gate at −10 LU, loudness range from 3 s blocks (EBU Tech 3342), channel weights of 1.0 for the fronts and 1.41 for the surrounds, the LFE left out, mono counted as dual mono, and true peak by 4× oversampling.

```rust
use helikodu_engine::analysis::{album_loudness, measure_source};

let registry = DecoderRegistry::with_default_backends();
let track = measure_source(&registry, &source, &|| cancelled())?;  // a file or a CUE span
track.loudness.integrated_lufs;   // LUFS, None for silence
track.loudness.gain_db();         // ReplayGain 2.0: −18 LUFS − integrated
let album = album_loudness(&tracks);   // the tracks as one programme
```

| Type | What it is |
|---|---|
| `LoudnessMeter` | Takes interleaved `f32` or decoded `PcmBlock`s (following changes of rate or channels) and `finish`es into a `TrackMeasurement` |
| `TrackLoudness` | Integrated loudness, loudness range, sample and true peak, frames and rate; `gain_db()` |
| `AlbumLoudness` | The same for an album measured as one programme: the gating blocks of all its tracks gated together, not the mean of the track gains |
| `MEASUREMENT_VERSION` | Changes when the results change, so stored results of an older version are measured again |

A finished `TrackMeasurement` keeps only the meter's histograms (its audio buffer is shrunk), so the tracks of a large album can wait for the album measurement without holding their audio.

## Tests

```sh
cargo test -p helikodu-engine
```

The null and WAV sinks are tested end to end, including bit-exact WAV output.

The processing tests (`tests/pipeline.rs` and the unit tests in `src/pipeline/`) check the gain arithmetic including the R128 conversion, the mixing matrices, that volume changes and fades never step more than the signal itself, that a resampled sine keeps its frequency, level, timing and length, the statistics of the dither noise, and that a bit-perfect plan passes samples through bit for bit.

The transport tests in `tests/transport/` play generated media through the engine to the WAV and null sinks, freewheeling, with the test acting as the service (answering `NearEnd` from a queue and skipping failed items). They check gapless joins of a signal cut into three files (WAV and FLAC bit-exact, MP3 and Opus with the exact total length), the reopening of the device for a change of rate, pause and resume continuing at exactly the reported position, seeks landing on the exact frame (the shape of the fade in shows a landing that is off by one), seeking while paused, stop, a start position, a device switch continuing at the same frame on the second WAV file, settings changed while playing (a new sample format reopening the stream at the same frame, also while paused; a loudness change ramping the gain on the open stream without a click; policies applied without a new stream), software volume, the order of the events and rising positions, a broken file in the queue being skipped with an exact join around it, and outputs that cannot be opened. `tests/transport/crossfade.rs` checks crossfades on constant levels in float output: the exact length (also resampled to 48 kHz), the equal-power gains frame by frame, the skip crossfade, a crossfade switched on while playing, the fall and rise around a reopened stream, consecutive album tracks staying bit-exact (as with crossfades off), crossfaded sines without clicks, and the lengths of the pause, resume, seek, stop and skip fades of the settings. Every transition is checked for clicks: no step between samples above 1.5 times the largest step of the test sine.

`tests/transport/devices.rs` plays in real time through a scripted backend (a `BackendConnector` over the null sink) that stands for PipeWire, ALSA, PulseAudio and JACK in turn: a lost device under each of the three policies on every backend, pausing when the followed default device is unplugged (and not when the setting is off or another device goes), and a device profile applied when its device opens (mode, rate, format, buffer, the release of a paused exclusive device), the general settings on a device without one, and a changed profile reopening the stream.

`tests/bit_perfect.rs` plays generated files of 16-, 24- and 32-bit integer and 32-bit float samples (every bit used) to the WAV sink and compares the written bytes with the source, bit for bit, and checks that the signal path says bit-perfect; 24-bit samples widened into a 32-bit format, a 32-bit source into a 24-bit format (reported lossy), and the DAC warm-up's silence. With the `snd-aloop` module loaded it plays 32-bit samples to the loopback card through ALSA in exclusive mode and compares what its capture side records; otherwise it prints `SKIPPED:` (it never loads the module). `tests/exclusive_release.rs` plays in real time through a scripted card that has to be reserved: the card is given back on stop, after the pause timeout (and taken again on play), never with a timeout of 0, and not taken at all in shared mode. The signal paths of several plans (bit-perfect on ALSA `hw:`, PipeWire exclusive and shared, resampling by the engine and the server, downmix, gain, volume and dither into a narrower device, float sources and files) are unit tests in `src/transport/signal.rs`.

The DSP tests (`tests/dsp.rs` and the unit tests in `src/dsp/`) check the biquads' magnitudes against the cookbook's analog prototypes through the bilinear transform, that the overlap-corrected graphic equalizer meets random, preset, all-raised and zig-zag slider settings at 44.1, 48 and 96 kHz, the gains of sines through the equalizers, the crossfeed's low-frequency separation (equal to its level) and high-frequency separation, the limiter's ceiling between the samples (measured by 16× interpolation with a long sinc, on sines near Nyquist, a quarter-rate sine, band-limited noise, a square wave and bursts, in blocks of uneven size), that a chain that is off is bit-identical, that settings changed every 256 frames never step more than the signal itself, and AutoEQ and EqualizerAPO files. `tests/transport/dsp.rs` plays through the engine: neutral stages stay bit-exact, a change while playing crossfades without a click, and the limiter delays without losing audio. `tests/pipeline_allocations.rs` also runs the chain, crossfades included, under the counting allocator.

The loudness tests (`tests/loudness.rs`) generate their signals: cases 1 to 5 of EBU Tech 3341 (stereo 1 kHz sines, with both gates) and cases 1 to 4 of Tech 3342 (loudness range) within ±0.1 LU, the true peak of a sine at a quarter of the sample rate whose samples miss its peak by 3 dB, the album values of a 3-track album, and a WAV file and its two sections measured through the decoder registry.

The decoding tests in `tests/decoding/` generate their media at test time: a reference signal written as WAV by the tests, encoded with the `ffmpeg` tool into Cargo's temporary directory, and DSF/DFF files of a sigma-delta modulated sine. A test whose tool or encoder is missing prints `SKIPPED:` and passes. They check the backend choice (by content, also for misnamed files), exact lengths for WAV, FLAC, MP3, Opus, Vorbis and AAC, bit-exact sections and joins of consecutive sections, exact seeks in files and sections, DSD level and length, and the mapping of failures to `EngineError`.

The corpus tests in `tests/corpus/` run the engine against the generated test corpus (`cargo xtask corpus`, see `tests/corpus/README.md`). Without `target/corpus/` (or `$HELIKODU_CORPUS`) they print `SKIPPED:` and pass; with it they take a few seconds and run as part of `cargo test` and `cargo xtask ci`. Each prints a table of results with `-- --nocapture` and writes it to `target/tmp/corpus-report/`:

| Test | What it checks |
|---|---|
| `formats` | Every sample of the formats set opens through the decoder registry and decodes to the rate and channels of its manifest entry, and to the exact length where the manifest states one (otherwise within 2 % plus 0.1 s of the source) |
| `gapless` | Each encoder's split set plays as a queue through the transport into the WAV sink, freewheeling, with one `TrackStarted`/`TrackEnded` pair per part and no reopened device: lossless sets bit-exact against the reference, lossy sets with its exact length. AAC in ADTS has no gapless information; its difference is reported, not checked |
| `seeking` | Seeks to five positions (forwards and backwards) in every format: lossless formats land on the exact frame; lossy ones aligned (after the decoder settles, the output matches a straight decode best at the target frame). The table shows each lossy decoder's warm-up after a seek |
| `video` | Every video file plays its audio only (the default stream when there are several) through the registry and the transport; a video-only file is unplayable. A child process decodes the set with FFmpeg's log on: every video stream's decoder is refused by the bridge's codec whitelist, and the whitelist names none of the build's video decoders (from `ffmpeg -decoders`) |
| `broken` | Truncated, empty, garbage and misnamed files never panic; each plays what it has or fails with an error event, and the good files queued between them all play to their end |
| `names` | Files whose names are not UTF-8, 255 bytes long, or contain a newline, a leading dash or characters Windows forbids play through the transport, bit-exact |
| `longtail` | The project's own tracker module, NSF, PSID tune and MIDI files (also under misleading names) are claimed by their own backends by content, decode to the length they state, are not silent, and every subsong opens; MIDI plays with the corpus's own SoundFont |

Formats and seeks the engine does not handle yet are listed in `tests/corpus/formats.rs`, `seeking.rs` and `video.rs` with the reason, and are reported as gaps rather than failures; `HELIKODU_CORPUS_STRICT=1` makes them fail.

`tests/long_tail.rs` covers the long tail of formats with the files `helikodu-testkit` writes: recognition by content whatever the name, subsongs through `SourceSection::Subsong` (and a span of a subsong file), the SoundFont setting, whole-file buffering (off by default, used when on, not above the limit), the audio stream chosen by index in a Matroska file with two streams (muxed by the `ffmpeg` program; `SKIPPED:` without it) both in the registry and through `SelectAudioStream` while paused, a subsong played through the transport, DoP checked word for word through the WAV sink, and its fallback to PCM.

`tests/stress.rs` is the underrun stress test, ignored by default: an endless queue of near-silent material plays in real time while one busy thread per processor competes for the CPU, and any underrun (counted by `Engine::stats()`), error or reopened device fails it. `HELIKODU_STRESS_SECS` sets the length (60 s by default), `HELIKODU_STRESS_BACKEND` the output (`null`, paced like a device, or `pipewire`, a temporary null sink loaded with `pactl`), `HELIKODU_STRESS_LOAD` the number of busy threads. `cargo xtask stress` runs it with optimisations; the extended checks run it for an hour.

```sh
cargo xtask corpus && cargo test -p helikodu-engine --test corpus -- --nocapture
cargo xtask stress --secs 600 --backend pipewire
```

Tests against real sound servers and hardware are ignored by default. They never make a sound:

```sh
# PipeWire: needs a running PipeWire session with pipewire-pulse (`pactl`). Each test
# loads a temporary null sink, plays digital silence into it by name (or reads its
# capabilities), and unloads it.
cargo test -p helikodu-engine --test pipewire_output -- --ignored --test-threads 1

# PulseAudio and JACK: needs PipeWire with pipewire-pulse and pipewire-jack. Lists the
# devices, plays silence into a temporary null sink through each, checks where the
# JACK ports are connected, unloads the sink while playing (the stream is lost).
PIPEWIRE_PROPS='{ jack.self-connect-mode = ignore-all }' \
    cargo test -p helikodu-engine --test cpal_output -- --ignored --test-threads 1

# ALSA: needs sound cards. Lists devices and the default one, opens each free hw:
# device, negotiates a format and closes it again, without starting a stream or
# writing a sample.
cargo test -p helikodu-engine --test alsa_output -- --ignored
```

The PipeWire tests set the stream volume; the session manager remembers the last volume of the application "Helikodu".
