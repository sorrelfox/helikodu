# fuzz

`cargo fuzz` targets and their seed inputs for the parsers and the FFmpeg bridge.

This folder is outside the Cargo workspace because fuzzing needs a nightly toolchain.

## Targets

| Target | What it exercises |
|---|---|
| `ffmpeg_bridge_open` | `helikodu-ffmpeg-bridge`: opening arbitrary bytes with every offline lock on, metadata, chapters, attached pictures, decoding and seeking, also to the extreme positions a remote control can ask for |
| `cue_parse` | `helikodu-meta`'s CUE sheet parser: charset detection, parsing and resolution into virtual tracks, checking that no track's file leaves the sheet's folder and that every span is ordered |
| `playlist_m3u`, `playlist_pls`, `playlist_xspf` | `helikodu-meta`'s playlist parsers (M3U and M3U8 share a target): charset detection, parsing within the limits, writing the result back and parsing it again, checking that every local entry is an absolute path without `.` or `..` and that the entry and warning limits hold |
| `autoeq_parse` | `helikodu-engine`'s reader of AutoEQ `ParametricEQ.txt` files: parsing, checking that every filter it returns is valid in the settings and that there are at most 64, then writing the filters back and reading them again |
| `openmpt_open`, `gme_open`, `sid_open`, `midi_open` | The long-tail decoders of `helikodu-extra-decoders` (libopenmpt, libgme, libsidplayfp through its C++ shim, rustysynth with a SoundFont the target writes): the header probe, opening arbitrary bytes, every subsong (at most four), rendering, seeking to the middle and back to the start. The shared steps are in `fuzz_targets/song_check.rs` |
| `query_parse` | `helikodu-library`'s query language parser: the strict parser's error spans, the round trip of what parses through the printer, and the plain-text fallback of the lenient parser |
| `cover_decode` | `helikodu-meta`'s cover decoder: format sniffing, header probing, decoding within the size, dimension and pixel limits, and scaling into a thumbnail, checking that a decoded picture stays within the limits and agrees with its header |
| `lyrics_parse` | `helikodu-meta`'s LRC parser: time, metadata and word tags, the line bounds, time order, word offsets inside their line, and the current-line lookup at extreme positions and offsets |
| `naming_pattern` | `helikodu-meta`'s naming patterns (the organizer, the converter and folder sync): parsing, error spans, and rendering with arbitrary field values in every character set, checking that no name is empty, `.` or `..`, holds `/` or NUL, is too long for a file system, or breaks the Windows rules of the sets that keep them |
| `expression` | `helikodu-model`'s expression language: reading arbitrary text as a template and as a path template, error spans, evaluation with arbitrary field values of one or several values within the length limits, and the same fields and pieces as the naming patterns of before (`crates/model/src/expression/legacy.rs`) for every text those read |
| `playlist_sniff` | `helikodu-meta`'s playlist parsers as the automatic import uses them: the format sniffed from the bytes, then the checks of the other playlist targets |
| `tags_read` | The scan worker's reader (`helikodu-meta`): the quick hash, the tags through lofty and the FFmpeg fallback, the embedded CUE sheet, chapters and the embedded cover; the report must travel through the worker protocol unchanged |
| `worker_message` | `helikodu-meta`'s scanner–worker protocol: decoding arbitrary messages and frame streams; whatever decodes must encode back to the same bytes |
| `settings_toml` | `helikodu-service`'s reader of `settings.toml` (migration, validation, the reset of groups out of range): the result must validate and read back unchanged once written |
| `ui_state_toml` | The window state `ui-state.toml` (`helikodu-model`): moving values into range, and the round trip |
| `export_toml` | `export.toml` (`helikodu-service`): the converter's presets and the sync destinations, used as the converter and folder sync use them (size estimates at extreme lengths, the copy-as-is test, naming patterns) |
| `sync_manifest` | The manifest of a sync destination, `.helikodu-sync.json`, which anyone with the destination may have written: every entry left must be a plain relative path inside the destination |
| `dsd_open` | `helikodu-extra-decoders`' own DSF and DSDIFF reader: the raw stream, conversion to PCM and DoP packing, with seeks to the middle, past the end and back |
| `ffmpeg_encode` | The FFmpeg bridge's encoder: arbitrary settings (format, rate, channels, bits, quality) and arbitrary samples (NaN, infinities, values far out of range) in pieces that are not always whole frames |

The D-Bus interface has no fuzz target: its arguments arrive typed by D-Bus, and the unit test `hostile_arguments_are_read_or_refused` in `crates/ui-gtk/src/remote_api.rs` feeds every method extreme numbers, wrong types and strange text instead. The queries it takes go through the query parser, which `query_parse` covers. The scrobbler and ListenBrainz exports only write.

## Running

With `cargo-fuzz` installed:

```sh
cargo +nightly fuzz run ffmpeg_bridge_open
cargo +nightly fuzz run cue_parse
cargo +nightly fuzz run cover_decode
cargo +nightly fuzz run lyrics_parse
cargo +nightly fuzz run playlist_m3u
cargo +nightly fuzz run playlist_pls
cargo +nightly fuzz run playlist_xspf
cargo +nightly fuzz run autoeq_parse
cargo +nightly fuzz run openmpt_open
cargo +nightly fuzz run gme_open
cargo +nightly fuzz run sid_open
cargo +nightly fuzz run midi_open
cargo +nightly fuzz run query_parse
cargo +nightly fuzz run naming_pattern
cargo +nightly fuzz run expression       # and so on for every target above
```

Without it, `campaign.sh` builds every target with the same compiler flags (libFuzzer, coverage, AddressSanitizer, overflow checks) using only a nightly toolchain, and runs a campaign:

```sh
fuzz/campaign.sh 900 8                     # every target for 15 minutes, 8 at a time
fuzz/campaign.sh 300 2 tags_read dsd_open  # some targets
```

Seeds come from `fuzz/seeds/<target>/` when that folder exists; corpora, logs and crashes go to `fuzz/runs/<target>/`. Both folders are ignored by git. A target that finds a crash stops; the input is in `fuzz/runs/<target>/artifacts/`. Reproduce it by running the target's binary on that file, shrink it with `-minimize_crash=1`, fix the code, and add the shrunk input as a regression test next to the code it broke (or to the malformed-media corpus below). [docs/dev/fuzzing.md](../docs/dev/fuzzing.md) records each campaign and what it found.

Leaks inside system libraries that Helikodu cannot free are listed in `lsan.supp`, which `campaign.sh` passes to LeakSanitizer; with `cargo fuzz`, set `LSAN_OPTIONS=suppressions=fuzz/lsan.supp`.

The C libraries (FFmpeg, libopenmpt, libgme, libsidplayfp, LAME and the other encoders) come from the system and are not instrumented: AddressSanitizer catches what they do to memory Rust allocated, and every crash or abort inside them, but not every overflow within their own memory.

## Malformed-media corpus

`cargo test -p helikodu-meta --test malformed` writes a small corpus of damaged files (truncated, flipped, zeroed, noisy and lying-length FLAC, WAV, MP3, Ogg Vorbis, Opus, AAC and ALAC in MP4, DSF and DSDIFF files, CUE sheets, LRC lyrics and M3U, PLS and XSPF playlists, plus the inputs fuzzing found) into `target/tmp/meta-malformed/` and reads each one as the player does, with a time limit per file. Those files make good seeds for the media targets (prepend the extension byte for `tags_read`).

Good seed inputs are small files of every format the player opens; `cargo test -p helikodu-ffmpeg-bridge` leaves a set in `target/tmp/ffmpeg-bridge-media/`.

### CUE sheets

Seed `cue_parse` with real sheets: EAC and foobar2000 rips (single file with pregaps, one file per track), sheets in cp1251, Shift-JIS and UTF-16, and the sheets written by `cargo test -p helikodu-meta --test cue` into `target/tmp/meta-cue/`. The first byte of each input picks the legacy charset the parser is told the user chose, so the rest of the input is the sheet.

### Playlists

Seed the playlist targets with files other players write: Winamp and foobar2000 M3U in cp1251 or Shift-JIS with Windows paths, UTF-8 M3U8 with `#EXTINF` and a byte order mark, UTF-16 M3U, Winamp PLS, and VLC and Audacious XSPF; the tests of `cargo test -p helikodu-meta --test playlists` write such files into `target/tmp/meta-playlists/`. The first byte of each input picks the legacy charset (and, for `playlist_m3u`, M3U or M3U8 by its lowest bit), so prepend one byte to each seed. The shared checks are in `fuzz_targets/playlist_check.rs`.

### Long-tail files

Seed the long-tail targets with small files of each format: the corpus's `longtail/` set (`cargo xtask corpus`), and tracker modules, NSF, SPC, VGM, SID and MIDI files the project may redistribute.

### AutoEQ files

Seed `autoeq_parse` with `ParametricEQ.txt` files from AutoEQ's results (with and without shelf filters) and with EqualizerAPO configurations that include other commands (`Device:`, `Channel:`, `GraphicEQ:`), in UTF-8 with and without a byte order mark and with Windows line endings.

### Covers

Seed `cover_decode` with small JPEG (baseline, progressive, CMYK, with an Exif orientation), PNG (palette, grey, 16-bit, interlaced, with transparency) and WebP (lossy, lossless, animated) files. The first byte of each input is the thumbnail size, so prepend one byte to each seed.

### Naming patterns

Seed `naming_pattern` with patterns such as `{albumartist}/{album}/[{disc}-]{track} {title}`. The first byte sets the track and disc counts; the rest is split at the byte 0x01 into the pattern, the word for an empty field, and the values of title, artist, album artist, album, track, disc, year and genre.

### Media for the scan worker and the encoder

Seed `tags_read` with small tagged files of every format, each with one byte in front that picks the extension the file is given (0 `flac`, 1 `mp3`, 2 `ogg`, 3 `opus`, 4 `m4a`, 5 `wav`, 6 `aiff`, 7 `ape`, 8 `wv`, 9 `mka`, 10 `dsf`, 11 `mpc`; the top bit asks for the legacy charset `windows-1251`); the tests of `cargo test -p helikodu-meta --test tags` leave such files in `target/tmp/meta-tags-media/`. Seed `dsd_open` with DSF and DSDIFF files (`cargo test -p helikodu-extra-decoders --test dsd` writes some into `target/tmp/extra-decoders-dsd/`). `ffmpeg_encode` reads six bytes of settings (format, rate, channels, bits, quality kind and value) followed by pieces of samples, each a byte (length and float or integer) and its samples.

### Settings files

Seed `settings_toml`, `ui_state_toml` and `export_toml` with the files Helikodu writes in `~/.config/helikodu/` (copies, never the originals), and `sync_manifest` with a `.helikodu-sync.json` from a synced folder.

### Queries

Seed `query_parse` with the examples of the query syntax (`artist:"Pink Floyd" year:1970..1979 rating>=4`, `genre:jazz -genre:fusion added<30d`, `codec:flac samplerate>=88200 sort:added desc limit:50`, `title~"^(Intro|Outro)$" OR duration<30s`) and with plain search text in several scripts.
