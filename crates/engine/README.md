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
| `DecoderInfo` | Backend, container, codec, significant bits and the tags the backend reads with the audio |
| `DecoderFactory` | A backend: `name()`, `probe()` returning a `ProbeScore`, `open()` a path (and a subsong) |
| `Probe`, `ProbeScore` | The path and first 4 KiB of the file; `NONE`, `FALLBACK`, `EXTENSION`, `SIGNATURE` |
| `DecoderRegistry` | The backends and the choice of one for a `MediaSource` |
| `PcmBlock`, `BlockSamples` | One block of interleaved samples with its `AudioSpec` and position |
| `DecodeError` | Everything that can fail; `to_engine_error()` and `From` map it to the model's `EngineError` |

**Choosing a backend.** The registry reads the first 4 KiB of the file and asks each backend for a score. Backends are tried from the highest score down, on ties in registration order, and the first that opens the file wins; when none does, `DecodeError::NoDecoder` lists each backend's reason. File names never decide: FFmpeg probes the content itself and scores every file `FALLBACK`, the DSD backend claims files with the DSF or DSDIFF signature (`SIGNATURE`). An unreadable file stops the search at once.

| Backend | Cargo feature | Files | Subsongs |
|---|---|---|---|
| `dsd` | `dsd` (default) | DSF and DFF, decoded to PCM by the reader in `extra-decoders`. DST-compressed DFF is refused there and falls through to FFmpeg | No |
| `ffmpeg` | always built | Everything else FFmpeg decodes, through `ffmpeg-bridge` | No |

**Block format.** Blocks hold interleaved samples in WAVE channel order, in the format the source decodes to: 8-, 16- and 32-bit integers keep their exact values (24-bit sources sit in the top bits of 32-bit words, `DecoderInfo::bits_per_sample` says 24), floating-point sources are `f32`. The render side converts to the `f32` of the output ring buffer with `PcmBlock::append_f32`, which divides by 2^(bits − 1) and is exact for 8-, 16- and 24-bit values. Keeping integers until that point leaves room for a bit-perfect path that hands them to the device untouched.

**Gapless playback.** Encoder delay and padding are trimmed by the backend: FFmpeg, through the bridge, for MP3 (LAME/Info tags), AAC and Opus in MP4, Opus and Vorbis in Ogg and Matroska. Positions count frames from the first frame after the delay. `seek()` lands on the exact frame.

**Sections.** A `SourceSection::Span` (a CUE track, for example) is applied by the registry on top of any backend. Its times are converted to frames at the decoder's rate, rounding down, which is exact for CUE times; the decoder seeks to the start and blocks are cut at both ends, so consecutive spans of one file join without a gap or an overlap. Positions, `duration()` and `seek()` are relative to the span. The first block is decoded when opening, so a span that starts after the end of the file fails with `InvalidSection` instead of trusting the container's stated length, which is an estimate for some formats. `SourceSection::Subsong` is passed to the backend; neither current backend has subsongs, so it fails with `SubsongUnsupported`.

**DSD.** DSF and DFF play as `f32` PCM at 176.4 kHz (DSD64 to DSD512; 192 kHz for the 48 kHz family), raised by 6 dB because the DSD 0 dB reference corresponds to −6 dBFS. See the `extra-decoders` README for the filter.

**Errors.** Everything that keeps a source from opening (`Io`, `Open`, `NoDecoder`, `SubsongUnsupported`, `InvalidSection`) becomes `EngineError::Unplayable`; a failure after opening (`Decode`) becomes `EngineError::Decode`.

Building the crate needs the FFmpeg development files (7.1 or newer), as for `ffmpeg-bridge`.

Not done yet in the decoding layer:

- **A per-format backend preference** (for example the DSF reader over FFmpeg only when DoP is wanted). The registry order is fixed.
- **The long-tail backends** (libopenmpt, libgme, vgmstream, libsidplayfp, MIDI) and with them subsongs.
- **DoP and native DSD output.** The DSD reader already yields the raw stream in time order for it.
- **Stream choice.** FFmpeg decodes the default or best audio stream; a per-file choice is not passed through yet.

## Output

`src/output.rs` defines the `OutputBackend` and `OutputStream` traits; each backend has its own file in `src/output/`. Opening a stream negotiates the device format and returns a `SampleWriter`, the producer side of a lock-free ring buffer (`rtrb`) of interleaved `f32` samples. The backend's real-time side copies out of the ring buffer, runs the device-side stage (below) and converts to the device format; underruns are counted in atomics. Device changes, lost devices and failures arrive as `OutputEvent`s on a channel.

**The device-side stage** (`src/output/device_stage.rs`) applies software volume, the anti-click fades (the pipeline's `Fader`) and dither after the ring buffer, so that volume, pause, seek and stop are heard within one device period rather than after the whole buffer. The render thread controls it through the writer: `SampleWriter::volume()` is a `SharedVolume` (two atomics), and `SampleWriter::schedule(at, DeviceAction)` queues a fade, an open or shut fader, a change of dither, or a `Discard` of buffered frames on a second lock-free queue, each to apply at a ring position (frames written since the stream opened), so a fade starts on exactly the frame it is meant for. `is_silent()` and `silent_since()` tell the render thread when a fade out has finished and where. A `Discard` with `hold` drops the buffered audio and plays silence, without counting underruns, until the next fade in, so the buffer can be refilled after a seek. Until told otherwise the fader is open and the volume full, so samples pass unchanged. `StreamTiming` counts the discarded frames, and `heard_position()` gives the ring position being heard.

| Backend | Cargo feature | Device names |
|---|---|---|
| PipeWire | `pipewire` (default) | Node name, for example `alsa_output.usb-Topping_D10s-00.analog-stereo` |
| ALSA | `alsa` (default) | `hw:CARD=<card ID>,DEV=<n>`, and PCMs of the ALSA configuration such as `pipewire` |
| Null | always built | `null` |
| WAV file | always built | The file name |

Building the native backends needs the PipeWire and ALSA development files (`pkg-config` names `libpipewire-0.3` and `alsa`) and `libclang` for the PipeWire bindings. `--no-default-features` builds the crate without them.

The `alsa` crate is pinned to the version cpal uses (0.11 with cpal 0.18): two versions of `alsa-sys` cannot be linked into one program, so both are upgraded together.

**Device reservation.** A sound server usually holds every card, so a `hw:` device is busy. The ALSA backend takes the card through a `DeviceReservation` (`src/output/reservation.rs`) that the program running the engine injects, with `EngineConfig::device_reservation`, `AlsaBackend::with_reservation` or `connect_backend_with`. This crate does not speak D-Bus: `helikodu-desktop` implements the protocol (`org.freedesktop.ReserveDevice1`). In exclusive mode the backend takes the card before opening a `hw:` or `plughw:` device; in shared mode only when the device is busy. After taking it, it retries a busy device for up to one second while the previous holder closes it. The stream keeps the `ReservationGuard` until it is stopped or dropped (which the transport does on stop and after `exclusive_release_s` of pause) and drops it after closing the device. A holder that refuses gives `OutputError::DeviceBusy` naming it; without a reservation (the default, `NoReservation`) or when reservation fails, the device is opened anyway and a busy one is reported busy, naming the program that holds it. The guard reports whether someone has asked for the card back (`release_requested`); the engine does not act on it yet.

Not done yet in the output layer:

- **Real-time priority for the ALSA writer thread** (rtkit or the Realtime portal). PipeWire's process callback already runs on PipeWire's real-time thread.
- **ALSA hot-plug through udev.** The ALSA backend polls `/proc/asound/cards` every two seconds instead.
- **Device capabilities on PipeWire.** The device list carries none yet; ALSA probes them from the hardware parameters.
- **PulseAudio and JACK** backends.

## Processing

`src/pipeline.rs` and `src/pipeline/` hold the processing path between the decoders and the output backends. For each track a `PathPlan` decides, from the source format, its loudness information, the settings and the format the device was opened with, which stages run. When none is needed (no gain, no channel mixing, no resampling, no DSP, no crossfade, the volume at 100 % or outside the engine, and a device format that carries the samples exactly) the plan is bit-perfect and the samples reach the device unchanged. The plan is plain data and prints as a signal path, for example `96 kHz/24-bit 5.1 → downmix 5.1 → stereo → ReplayGain -6.30 dB (track) → resample 96 → 48 kHz (high) → volume 85 % → TPDF dither 16-bit → s16le 48 kHz stereo`.

| Stage | Type | Notes |
|---|---|---|
| Conversion to float | `to_float` | Integers as `n / 2^(bits-1)`: exact up to 24 bits |
| Channel mapping, downmix, upmix | `MixMatrix` | ITU-R BS.775 coefficients (centre and surrounds at −3 dB), LFE dropped unless included, downmix scaled so it cannot clip |
| Loudness gain | `resolve_gain`, `GainStage` | ReplayGain 2.0 reference of −18 LUFS; Opus R128 tags become `r128 / 256 + 5` dB; track, album and automatic modes, preamps for tagged and untagged files, clipping prevention by peak |
| Resampling | `ResampleStage` | rubato behind the crate's own `Resampler` trait: FFT for rate pairs with a small common divisor, windowed sinc otherwise; four quality presets; start-up delay removed, `n` frames become `ceil(n × to / from)` |
| Software volume | `SoftwareVolume`, `SharedVolume` | `a = p³`, 20 ms linear ramps, mute |
| Anti-click fades | `Fader`, `FadeDurations` | Raised-cosine fades of 10 ms (5 ms around a seek) on start, pause, resume, seek, stop and device switch |
| Dither | `TpdfDither` | Triangular ±1 LSB to 8, 16 or 24 bits, only while the samples are processed |
| Device format | `Encoder` | Run by the output backends |

`SourceChain` (mixing, gain, resampling; one per source) and `OutputChain` (volume, fades, dither; on the mixed stream) build the stages from a plan. Samples are interleaved 32-bit floats in canonical channel order; gains and filter state are 64-bit. Every stage allocates when it is built and never while processing; `tests/pipeline_allocations.rs` checks this with a counting allocator.

`rubato` is pinned to an exact version because its interface changes often; only `src/pipeline/resample/rubato_backend.rs` uses it.

Not done yet in the processing path:

- **Crossfades and the DSP chain** (the plan already has their switches).
- **`OutputChain` is not used by the transport**: its stages run on the device side of the ring buffer instead (see Output).
- **Bit-perfect 32-bit integer sources.** The ring buffer holds floats, which carry 24 bits exactly; such sources always go the processed way.
- **Reading loudness tags.** `parse_gain_db`, `parse_peak` and `LoudnessInfo::from_r128` take the values; the tag reader that finds them comes with the metadata layer.

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
| `transport/render.rs` | Filling the ring buffer, joins, draining, the fades and refills around transitions |
| `transport/recovery.rs` | Settling after a transition; a failed output or a lost device |
| `transport/source.rs` | One item: decoder and `SourceChain` |
| `transport/timeline.rs` | Which item each part of the ring buffer belongs to, and the heard position |
| `transport/device.rs` | The connected backend, the open stream, and opening one for an item |

**How it plays.** While playing, the thread wakes every few milliseconds (a sixteenth of the buffer, 2 to 10 ms), writes at most one buffer length, follows the heard position and sends what is due. While stopped or paused it sleeps until a command arrives. Each block of work is bounded, and rendering reuses its buffers, so the steady state does not allocate; opening an item, a join and a transition do.

**Transitions.** Pause, seek, stop, a device switch and a `Load` that replaces playing audio fade out on the device side (10 ms, 5 ms for a seek), wait until the device side reports silence, and take the ring position where it began as the point heard last. Pause continues from exactly that point; seek, resume and a device switch drop the buffer (`Discard` with `hold`), seek the decoder, refill about 50 ms and fade in at the first new frame. A track that starts at its beginning is not faded in. Resuming after a pause re-decodes from the paused position instead of playing the stale buffer.

**Gapless joins.** `SetNext` opens the next item at once (errors are reported then). When the current item ends, the next one continues in the same render iteration if the settings would open the same stream for it (`stream_config` gives the same rate, format and channels); the samples join exactly. Otherwise the buffer plays out and the device is reopened (`Reconfiguring`). The timeline records where each item starts in the ring buffer, so `TrackEnded` and `TrackStarted` are sent when the join is heard. A `SetNext` that replaces an item already written after the heard one drops that part of the buffer with a `Discard` at its start.

**Position.** The heard ring position is the frames the device side has taken (played or dropped) minus the delay the backend reports; the timeline turns it into the item and its position, at the source rate.

**Errors.** An item that does not open (on `Load` or `SetNext`) sends `Error` and `TrackEnded` (`Failed`) at once; for `SetNext`, `NearEnd` follows again so the service can name the next item. A decoding error midway ends the item there; its `Error` and `TrackEnded` (`Failed`) come when that point is heard, and playback continues with the next item. A lost or failed stream applies `on_device_lost`: pause (the default), continue on the default device, or stop. A device missing at start-up falls back to the default one under `fallback-default`; otherwise the engine reports the error and pauses (or stops).

**Freewheeling sinks.** With `EngineConfig::sink_pacing` set to `Pacing::Freewheel`, the null and WAV sinks take audio as fast as it is rendered. There is no deadline then, so at the end of an item the engine waits (up to `next_wait`, 5 s) for the answer to `NearEnd` instead of draining at once; tests and rendering to a file get the same joins as real playback.

`examples/play.rs` plays one file to a chosen backend and device and prints the events; lines on standard input pause, resume, seek, change the volume and switch the output:

```sh
cargo run -p helikodu-engine --example play -- song.flac --backend pipewire --device <node name>
cargo run -p helikodu-engine --example play -- --list-devices --backend alsa
```

Not done yet in the transport:

- **Loudness tags** are not passed to the path plan yet (no gain is applied); the metadata layer reads them.
- **Opening the next item happens on the engine thread.** It takes milliseconds from local storage; slow storage would delay the buffer by as much.
- **The engine thread's priority** is not raised yet (it needs the same rtkit or portal route as the ALSA writer).
- **A hardware mixer** (`volume = "hardware:…"`) is not driven; the engine applies software volume instead.
- **DAC warm-up** (`preroll_ms`) is not applied, and settings other than the output selection cannot be changed while the engine runs.
- **`NearEnd` for items of unknown length** comes only when the item has been decoded to its end, about one buffer length before it ends.
- **Underruns and the signal path** are not reported as events; the model has no events for them yet.

## Tests

```sh
cargo test -p helikodu-engine
```

The null and WAV sinks are tested end to end, including bit-exact WAV output.

The processing tests (`tests/pipeline.rs` and the unit tests in `src/pipeline/`) check the gain arithmetic including the R128 conversion, the mixing matrices, that volume changes and fades never step more than the signal itself, that a resampled sine keeps its frequency, level, timing and length, the statistics of the dither noise, and that a bit-perfect plan passes samples through bit for bit.

The transport tests in `tests/transport/` play generated media through the engine to the WAV and null sinks, freewheeling, with the test acting as the service (answering `NearEnd` from a queue and skipping failed items). They check gapless joins of a signal cut into three files (WAV and FLAC bit-exact, MP3 and Opus with the exact total length), the reopening of the device for a change of rate, pause and resume continuing at exactly the reported position, seeks landing on the exact frame (the shape of the fade in shows a landing that is off by one), seeking while paused, stop, a start position, a device switch continuing at the same frame on the second WAV file, software volume, the order of the events and rising positions, a broken file in the queue being skipped with an exact join around it, and outputs that cannot be opened. Every transition is checked for clicks: no step between samples above 1.5 times the largest step of the test sine.

The decoding tests in `tests/decoding/` generate their media at test time: a reference signal written as WAV by the tests, encoded with the `ffmpeg` tool into Cargo's temporary directory, and DSF/DFF files of a sigma-delta modulated sine. A test whose tool or encoder is missing prints `SKIPPED:` and passes. They check the backend choice (by content, also for misnamed files), exact lengths for WAV, FLAC, MP3, Opus, Vorbis and AAC, bit-exact sections and joins of consecutive sections, exact seeks in files and sections, DSD level and length, and the mapping of failures to `EngineError`.

Tests against real sound servers and hardware are ignored by default. They never make a sound:

```sh
# PipeWire: needs a running PipeWire session with pipewire-pulse (`pactl`). Each test
# loads a temporary null sink, plays digital silence into it by name, and unloads it.
cargo test -p helikodu-engine --test pipewire_output -- --ignored

# ALSA: needs sound cards. Lists devices, opens each free hw: device, negotiates a
# format and closes it again, without starting a stream or writing a sample.
cargo test -p helikodu-engine --test alsa_output -- --ignored
```

The PipeWire tests set the stream volume; the session manager remembers the last volume of the application "Helikodu".
