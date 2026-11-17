# helikodu-extra-decoders

Safe decoders over the binding crates (libopenmpt, libgme, vgmstream, libsidplayfp), the pure-Rust MIDI synthesizer and the DSF/DFF reader used for DoP output. Each decoder sits behind its own Cargo feature, so packagers can leave a library out.

- **May depend on:** `model` and the `-sys` binding crates. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound. The DSD reader has none (`#![forbid(unsafe_code)]` on its module).

| Module | Cargo feature | What it is |
|---|---|---|
| `dsd` | `dsd` (default) | DSF and DSDIFF (DFF) reader, and DSD to PCM conversion. Pure Rust |

## DSD

```rust
use helikodu_extra_decoders::dsd::{DsdPcmDecoder, DsdReader};

// The raw 1-bit stream, for DoP and native DSD output.
let mut reader = DsdReader::open(path)?;
let mut bytes = vec![Vec::new(); reader.info().channels()];
reader.read(4096, &mut bytes)?;

// PCM, for every other output.
let mut decoder = DsdPcmDecoder::open(path)?;
let mut pcm = Vec::new();
while decoder.read(4096, &mut pcm)? > 0 {
    // interleaved f32 at decoder.spec().rate
}
```

| Type | What it is |
|---|---|
| `DsdReader` | Parses the container and reads the raw stream: per channel, bytes in time order with the earliest bit most significant, channels in WAVE order. `seek()` to a byte |
| `DsdInfo`, `DsdContainer` | Container, DSD rate, channel layout and length in bits per channel |
| `DsdPcmDecoder` | The stream decoded to interleaved `f32` PCM: `spec()`, `frames()`, `position()`, `read()`, `seek()` to an exact frame |
| `DsdError` | `Io`, `NotDsd`, `Unsupported` (DST compression, odd rates) and `Malformed` |
| `sniff()` | Recognises the DSF or DFF signature in the first bytes of a file |

**Containers.** DSF stores each channel in blocks (4096 bytes in practice), one block of each channel in turn, each byte least significant bit first; DFF stores one byte of each channel in turn, most significant bit first. The reader turns both into the same canonical form and reorders DFF channels (`SLFT`, `SRGT`, `C   `, `LFE `, `LS  `, `RS  `, ...) to WAVE order. A file shorter than its header says plays what is there. DST-compressed DFF files are refused with `DsdError::Unsupported`; the engine leaves them to FFmpeg.

**Conversion to PCM.** One linear-phase FIR filter decimates by the smallest power of two from 16 up that brings the rate to 192 kHz or below: DSD64, DSD128, DSD256 and DSD512 become 176.4 kHz, the 48 kHz family 192 kHz. The filter is a Kaiser-windowed sinc (β = 8, −6 dB at 40 kHz) of 32 taps per bit of decimation (512 at DSD64), so it is flat within 0.1 dB to 20 kHz and attenuates by more than 70 dB from 60 kHz up, below the output's Nyquist frequency, where the modulator's ultrasonic noise would otherwise fold into the audio band. Since every input bit is ±1, the filter runs a byte at a time from a table of the 256 possible sums for each byte of the filter. The filter is centred on the instant of each PCM frame, so the output has no delay; before the start and after the end the stream is extended with the DSD idle pattern (`0x69`). A stream of `n` bits per channel gives `n / factor` frames. The output is raised by 6 dB (`DSD_GAIN`): the DSD 0 dB reference (50 % modulation) corresponds to −6 dBFS in PCM.

## Tests

```sh
cargo test -p helikodu-extra-decoders
```

The tests in `tests/dsd.rs` write DSF and DFF files of a sine modulated by a second-order sigma-delta modulator at test time, and check the decoded PCM: rate, exact length, level (+6 dB, within 0.05 dB), noise and distortion, the absence of a filter delay, identical output from DSF and DFF, channel reordering, the raw byte order, exact seeks, DSD128, truncated and damaged files, and the refusal of DST compression. Unit tests check the filter's frequency response.
