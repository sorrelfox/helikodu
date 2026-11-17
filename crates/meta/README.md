# helikodu-meta

Tag reading and writing (with an FFmpeg fallback for formats the tag library does not cover), legacy charset repair, cover extraction, CUE and LRC parsing, playlist formats, naming templates, and the messages exchanged with scan workers.

- **May depend on:** `model`, `ffmpeg-bridge`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Reading tags

```rust
use helikodu_meta::tags::{ReadOptions, read_tags};

let file = read_tags(path, &ReadOptions::default())?;
println!("{:?} by {:?}", file.tags.title, file.tags.artists);
println!("{:?} at {:?} Hz", file.audio.duration, file.audio.sample_rate);
```

| Type | What it is |
|---|---|
| `read_tags`, `read_tags_from_file` | Read one file: lofty first, FFmpeg when lofty does not know the format or cannot parse the file |
| `ReadOptions` | Charset repair on or off, and the legacy charset the user chose (a WHATWG label such as `windows-1251`) |
| `FileTags` | The tags, the audio properties, which reader produced them (`TagSource`), why FFmpeg was used, and whether text was repaired |
| `TrackTags` | The format-independent record: titles, artists and album artists (credited string and list), sort names, numbers and totals, dates, genres, credits with roles (`Credit`, `Role`), labels, catalogue numbers, release types, MusicBrainz IDs, comment, lyrics present, ReplayGain and R128 values, rating, compilation flag, grouping, work and movement, and every other field in `extra` |
| `AudioProperties` | Duration, sample rate, channels, bit depth, bitrate, codec and container |
| `Rating` | 0 to 10 in half stars; `None` in `TrackTags::rating` means not rated |
| `TagReader`, `LoftyReader`, `FfmpegReader` | The reader seam; each reader on its own |

lofty reads MP3 (ID3v2, APEv2, ID3v1), FLAC, Ogg Vorbis, Opus, Speex, MP4, Monkey's Audio, Musepack, WavPack, WAV (RIFF INFO, ID3v2), AIFF and ADTS AAC. FFmpeg reads the rest (Matroska, ASF/WMA, DSF, DFF, TAK, TTA); those tags are read-only. Cover art is not read here.

### Field mapping

`src/tags/mapping/` holds one table per tag format, following MusicBrainz Picard's tag mapping: ID3v2 frames and `TXXX` descriptions, Vorbis comments, APEv2 items, MP4 atoms and freeform names, RIFF INFO, and FFmpeg's dictionary (generic keys, ASF attributes, Matroska names). APEv2, `TXXX`, freeform atoms and FFmpeg fall back to the Vorbis names. A file with several tags (ID3v2, APEv2 and ID3v1 on one MP3) is read in order of preference: a single-valued field takes the first value found, and a multi-valued field comes entirely from the first tag that has it.

Multiple values are NUL-separated values in ID3v2 and APEv2, repeated Vorbis comments, and repeated MP4 data atoms. Genres are also split at `;`, and ID3 genre references such as `(17)` are resolved. Slash-joined lists are only split for MusicBrainz IDs.

### Charsets

`src/tags/charset.rs` repairs text written in legacy code pages (cp1251, Shift-JIS, GBK, …) and read as Latin-1, and UTF-8 bytes in frames declared Latin-1. Text is only touched when it can be turned back into its bytes and looks like mojibake, and the repair must decode cleanly into a non-Latin script; Western European text is left as it is. The charset is guessed per tag with `chardetng`, unless the user chose one.

### Ratings

`POPM` (by player e-mail: range table, linear for Quod Libet and Picard, MusicBee's half-star values), `FMPS_RATING`, `RATING` (0–5 or 0–100) and ASF `WM/SharedUserRating` are read onto the half-star scale. The table is in `src/tags/rating.rs`.

## Tests

```sh
cargo test -p helikodu-meta
```

The integration tests in `tests/tags/` generate their media at test time in Cargo's temporary directory: a WAV file written by the tests, encoded with the `ffmpeg` tool with `-metadata`, then edited with lofty where a test needs frames FFmpeg cannot write (`POPM`, `UFID`, `TMCL`, `TIPL`, multi-valued fields, Latin-1 frames holding cp1251, Shift-JIS and GBK bytes). A test whose tool or encoder is missing prints `SKIPPED:` and passes.
