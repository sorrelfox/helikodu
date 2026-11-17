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

`src/output.rs` defines the `OutputBackend` and `OutputStream` traits; each backend has its own file in `src/output/`. Opening a stream negotiates the device format and returns a `SampleWriter`, the producer side of a lock-free ring buffer (`rtrb`) of interleaved `f32` samples. The backend's real-time side only copies out of the ring buffer and converts to the device format; underruns are counted in atomics. Device changes, lost devices and failures arrive as `OutputEvent`s on a channel.

| Backend | Cargo feature | Device names |
|---|---|---|
| PipeWire | `pipewire` (default) | Node name, for example `alsa_output.usb-Topping_D10s-00.analog-stereo` |
| ALSA | `alsa` (default) | `hw:CARD=<card ID>,DEV=<n>`, and PCMs of the ALSA configuration such as `pipewire` |
| Null | always built | `null` |
| WAV file | always built | The file name |

Building the native backends needs the PipeWire and ALSA development files (`pkg-config` names `libpipewire-0.3` and `alsa`) and `libclang` for the PipeWire bindings. `--no-default-features` builds the crate without them.

The `alsa` crate is pinned to the version cpal uses (0.11 with cpal 0.18): two versions of `alsa-sys` cannot be linked into one program, so both are upgraded together.

Not done yet in the output layer:

- **ALSA exclusive access while PipeWire holds the card.** Taking the card through device reservation (`org.freedesktop.ReserveDevice1`, over D-Bus) is left for later, so this crate does not depend on a D-Bus library. Until then, a `hw:` device that the sound server holds fails with `OutputError::DeviceBusy`, naming the program that holds it.
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

- **The render thread and the transport** that drive the chains, and with them crossfades and the DSP chain (the plan already has their switches).
- **Software volume in the real-time callback.** The output backends only copy and convert, so `OutputChain` runs before the ring buffer for now, and a volume change is heard after the buffer length.
- **Bit-perfect 32-bit integer sources.** The ring buffer holds floats, which carry 24 bits exactly; such sources always go the processed way.
- **Reading loudness tags.** `parse_gain_db`, `parse_peak` and `LoudnessInfo::from_r128` take the values; the tag reader that finds them comes with the metadata layer.

## Tests

```sh
cargo test -p helikodu-engine
```

The null and WAV sinks are tested end to end, including bit-exact WAV output.

The processing tests (`tests/pipeline.rs` and the unit tests in `src/pipeline/`) check the gain arithmetic including the R128 conversion, the mixing matrices, that volume changes and fades never step more than the signal itself, that a resampled sine keeps its frequency, level, timing and length, the statistics of the dither noise, and that a bit-perfect plan passes samples through bit for bit.

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
