# helikodu-extra-decoders

Safe decoders over the binding crates (libopenmpt, libgme, vgmstream, libsidplayfp), the pure-Rust MIDI synthesizer (rustysynth) and the DSF/DFF reader with its DoP packer. Each decoder sits behind its own Cargo feature, so packagers can leave a library out.

- **May depend on:** `model` and the `-sys` binding crates. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound. The `unsafe` code is in the module of the library it calls (`openmpt.rs`, `gme.rs`, `sid.rs`, `vgmstream.rs`); the DSD reader, the MIDI decoder and `song` have none. No panic crosses into C: the decoders pass no Rust callbacks (libopenmpt gets its own silent log and error functions), and the SID shim catches every C++ exception.

| Module | Cargo feature | What it is |
|---|---|---|
| `dsd` | `dsd` (default) | DSF and DSDIFF (DFF) reader, DSD to PCM conversion and DoP packing. Pure Rust |
| `openmpt` | `openmpt` (default) | Tracker modules (MOD, S3M, XM, IT, MPTM and about 45 more) through libopenmpt 0.6 or newer |
| `gme` | `gme` (default) | Game music (AY, GBS, GYM, HES, KSS, NSF/NSFE, SAP, SPC, VGM/VGZ) through libgme 0.6.2 or newer |
| `sid` | `sid` (default) | C64 SID tunes (PSID, RSID) through libsidplayfp 2.x and a C++ shim |
| `midi` | `midi` (default) | MIDI files (MID, KAR, RMI) through rustysynth and a SoundFont 2 file. Pure Rust |
| `vgmstream` | `vgmstream` (off) | Game audio streams (ADX, HCA, BRSTM, WEM, FSB, ...) through vgmstream's `libvgmstream` API 1.1 |
| `song` | with any of the five above | What they share: `SongDecoder`, `SongInfo` and `SubsongInfo`, `SongOptions`, the end-of-tune `Envelope`, `SongError` |

## Tracker modules, game music, SID, MIDI and game audio streams

```rust
use helikodu_extra_decoders::gme::{GmeDecoder, probe};
use helikodu_extra_decoders::song::{SongDecoder, SongOptions};

let data = std::fs::read(path)?;
if probe(&data) {
    let options = SongOptions::from_settings(&settings.playback.decoders);
    let mut decoder = GmeDecoder::open(&data, Some(2), &options)?;   // the third track
    let mut pcm = Vec::new();
    while decoder.read(4096, &mut pcm)? > 0 {
        // interleaved f32 stereo at decoder.rate() (44.1 kHz)
    }
}
```

Every decoder renders interleaved `f32` at `SongOptions::rate` (44.1 kHz; vgmstream at the stream's own rate), positions count frames from the start of the subsong, and `seek()` lands on the exact frame (see the notes below for modules and SID). `info()` gives the file's text (title, artist, album, date, comment) and its subsongs with their lengths; `None` as the subsong opens the file's default one (the first, or a SID tune's start song).

| Decoder | Recognised by | Subsongs | Length and end | Seeking |
|---|---|---|---|---|
| `OpenMptDecoder` | libopenmpt's header probe (`openmpt::probe`) | The module's subsongs (orders not reached from the start of the song) | Played once, as libopenmpt renders it, up to where the song ends or would jump back; no fade | libopenmpt moves to the row for the time, then renders to the frame |
| `GmeDecoder` | libgme's four-byte header check (`gme::probe`) | The tracks of multi-track files | The stated length, then the stated fade or `fade`; a stated loop plays the intro and `loops` passes, then `fade`; otherwise `default_length`, then `fade`. libgme's own silence detection and fade are off | Start over and render to the frame (exact) |
| `SidDecoder` | `PSID`/`RSID` signature | The tune's songs; the default is its start song | The HVSC song-length database (`sid_song_lengths`) when it lists the tune; otherwise `default_length`, then `fade` | Start over in a new emulator and render to the frame |
| `MidiDecoder` | `MThd`, or a RIFF `RMID` form | One | The file's length plus 1.5 s in which the last notes fade out | Start over and render to the frame (exact) |
| `VgmstreamDecoder` | vgmstream's own check of the name (most of its formats have no signature), then the content when it opens the file | The streams of a bank | vgmstream's: `loops` passes of a loop, then `fade` | vgmstream's own (exact) |

**Fades** fall along a raised cosine to silence at the end (`Envelope`).

**SID tunes.** With libsidplayfp 2.14 and newer the shim takes each chip's samples (`play(cycles)` and `buffers`) and mixes them itself, without the random dither of the library's mixer; older versions use `play(buffer, count)`. The power-on delay is fixed. The emulation is still not bit-for-bit repeatable (libsidplayfp seeds a random generator from the clock), so the samples after a seek match a straight run to within a few steps of the 16-bit output. With `sid_roms` naming a folder, the files whose names start with `kernal`, `basic` and `chargen` (or `characters`) are installed when they have the right size (8192, 8192 and 4096 bytes); without them libsidplayfp's built-in replacement KERNAL plays most PSID tunes. The song-length database is read once and kept while the file stays the same; `sid::tune_md5` gives a tune's key in it.

**MIDI files** need a SoundFont: `SongOptions::soundfont`, or else one installed on the system (`midi::system_soundfont`: `/usr/share/soundfonts/default.sf2`, `/usr/share/sounds/sf2/default-GM.sf2`, `FluidR3_GM.sf2`, `TimGM6mb.sf2`, `/app/share/soundfonts/default.sf2`). Without one, opening fails with `SongError::NeedsSoundFont` ("MIDI files need a SoundFont: choose one in the settings"). The last SoundFont loaded is kept for the next file. `midi::read_info` reads a file's text and length without a SoundFont, for the library scanner. The title is the first track's name, the date its copyright notice. rustysynth's parsers run under `catch_unwind`, so a damaged file is an error, not a panic.

**vgmstream** is off by default because few distributions package it. With the feature on, `helikodu-vgmstream-sys` looks for `libvgmstream` through pkg-config or in `VGMSTREAM_LIB_DIR`; when it is not found the build goes on with a warning and stub functions, and `vgmstream::available()` is false (with `HELIKODU_REQUIRE_VGMSTREAM=1` the build fails instead). vgmstream reads the file itself, and companion files beside it.

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

## DoP

`DopDecoder` packs the raw stream into DSD over PCM 1.1: each 24-bit sample carries the marker `0x05` or `0xFA` (alternating from frame to frame, the same for every channel of a frame; frame 0 has `0x05`, so seeks keep the alternation) in its top byte and 16 DSD bits of its channel below it, the earlier byte first. The words are left-justified in `i32`; the PCM rate is the DSD rate divided by 16 (DSD64: 176.4 kHz). `dop_silence(frame)` is the DoP word of the idle pattern.

## Tests

```sh
cargo test -p helikodu-extra-decoders
```

`tests/songs.rs` runs every synthesizing decoder on the project's own files from `helikodu-testkit` (a two-subsong ProTracker module, a three-track NSF, a two-song PSID tune, a MIDI file and a one-preset SoundFont, all written byte by byte by the test kit): recognition by content, subsongs, text, lengths with the default length and the fade, the song-length database, and seeks that give what a straight run gives. Nothing is played.

The tests in `tests/dsd.rs` write DSF and DFF files of a sine modulated by a second-order sigma-delta modulator at test time, and check the decoded PCM: rate, exact length, level (+6 dB, within 0.05 dB), noise and distortion, the absence of a filter delay, identical output from DSF and DFF, channel reordering, the raw byte order, exact seeks, DSD128, truncated and damaged files, and the refusal of DST compression. Unit tests check the filter's frequency response.
