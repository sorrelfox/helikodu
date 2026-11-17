# fuzz

`cargo fuzz` targets and their seed inputs for the parsers and the FFmpeg bridge.

This folder is outside the Cargo workspace because fuzzing needs a nightly toolchain.

## Targets

| Target | What it exercises |
|---|---|
| `ffmpeg_bridge_open` | `helikodu-ffmpeg-bridge`: opening arbitrary bytes with every offline lock on, metadata, chapters, attached pictures, decoding and seeking |
| `cue_parse` | `helikodu-meta`'s CUE sheet parser: charset detection, parsing and resolution into virtual tracks, checking that no track's file leaves the sheet's folder and that every span is ordered |
| `cover_decode` | `helikodu-meta`'s cover decoder: format sniffing, header probing, decoding within the size, dimension and pixel limits, and scaling into a thumbnail, checking that a decoded picture stays within the limits and agrees with its header |

```sh
cargo +nightly fuzz run ffmpeg_bridge_open
cargo +nightly fuzz run cue_parse
cargo +nightly fuzz run cover_decode
```

Good seed inputs are small files of every format the player opens; `cargo test -p helikodu-ffmpeg-bridge` leaves a set in `target/tmp/ffmpeg-bridge-media/`.

### CUE sheets

Seed `cue_parse` with real sheets: EAC and foobar2000 rips (single file with pregaps, one file per track), sheets in cp1251, Shift-JIS and UTF-16, and the sheets written by `cargo test -p helikodu-meta --test cue` into `target/tmp/meta-cue/`. The first byte of each input picks the legacy charset the parser is told the user chose, so the rest of the input is the sheet.

### Covers

Seed `cover_decode` with small JPEG (baseline, progressive, CMYK, with an Exif orientation), PNG (palette, grey, 16-bit, interlaced, with transparency) and WebP (lossy, lossless, animated) files. The first byte of each input is the thumbnail size, so prepend one byte to each seed.
