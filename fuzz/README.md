# fuzz

`cargo fuzz` targets and their seed inputs for the parsers and the FFmpeg bridge.

This folder is outside the Cargo workspace because fuzzing needs a nightly toolchain.

## Targets

| Target | What it exercises |
|---|---|
| `ffmpeg_bridge_open` | `helikodu-ffmpeg-bridge`: opening arbitrary bytes with every offline lock on, metadata, chapters, attached pictures, decoding and seeking |
| `cue_parse` | `helikodu-meta`'s CUE sheet parser: charset detection, parsing and resolution into virtual tracks, checking that no track's file leaves the sheet's folder and that every span is ordered |
| `playlist_m3u`, `playlist_pls`, `playlist_xspf` | `helikodu-meta`'s playlist parsers (M3U and M3U8 share a target): charset detection, parsing within the limits, writing the result back and parsing it again, checking that every local entry is an absolute path without `.` or `..` and that the entry and warning limits hold |
| `autoeq_parse` | `helikodu-engine`'s reader of AutoEQ `ParametricEQ.txt` files: parsing, checking that every filter it returns is valid in the settings and that there are at most 64, then writing the filters back and reading them again |
| `openmpt_open`, `gme_open`, `sid_open`, `midi_open` | The long-tail decoders of `helikodu-extra-decoders` (libopenmpt, libgme, libsidplayfp through its C++ shim, rustysynth with a SoundFont the target writes): the header probe, opening arbitrary bytes, every subsong (at most four), rendering, seeking to the middle and back to the start. The shared steps are in `fuzz_targets/song_check.rs` |
| `query_parse` | `helikodu-library`'s query language parser: the strict parser's error spans, the round trip of what parses through the printer, and the plain-text fallback of the lenient parser |
| `cover_decode` | `helikodu-meta`'s cover decoder: format sniffing, header probing, decoding within the size, dimension and pixel limits, and scaling into a thumbnail, checking that a decoded picture stays within the limits and agrees with its header |

```sh
cargo +nightly fuzz run ffmpeg_bridge_open
cargo +nightly fuzz run cue_parse
cargo +nightly fuzz run cover_decode
cargo +nightly fuzz run playlist_m3u
cargo +nightly fuzz run playlist_pls
cargo +nightly fuzz run playlist_xspf
cargo +nightly fuzz run autoeq_parse
cargo +nightly fuzz run openmpt_open
cargo +nightly fuzz run gme_open
cargo +nightly fuzz run sid_open
cargo +nightly fuzz run midi_open
cargo +nightly fuzz run query_parse
```

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

### Queries

Seed `query_parse` with the examples of the query syntax (`artist:"Pink Floyd" year:1970..1979 rating>=4`, `genre:jazz -genre:fusion added<30d`, `codec:flac samplerate>=88200 sort:added desc limit:50`, `title~"^(Intro|Outro)$" OR duration<30s`) and with plain search text in several scripts.
