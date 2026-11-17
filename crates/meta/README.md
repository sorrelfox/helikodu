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

## CUE sheets and chapters

```rust
use helikodu_meta::cue::{CueOptions, read_cue, read_embedded_cue, read_chapters};

let album = read_cue("/music/Album/Album.cue", &CueOptions::default())?;
for track in &album.tracks {
    println!("{} {:?} {:?}..{:?}", track.number, track.tags.title, track.start(), track.end());
}
let embedded = read_embedded_cue("/music/Album.flac", &CueOptions::default())?; // Option<CueTracks>
let chapters = read_chapters("/books/Book.m4b")?; // Vec<VirtualTrack>
```

| Type | What it is |
|---|---|
| `read_cue` | Read a `.cue` file and resolve its tracks against the files beside it |
| `read_embedded_cue` | The sheet inside an audio file: the `CUESHEET` tag (Vorbis comment, APEv2) through the tag reader, then FFmpeg, then the FLAC `CUESHEET` block. Every `FILE` stands for the audio file; the file's tags fill the album fields the sheet lacks |
| `read_chapters`, `chapters_to_tracks` | Container chapters (MP4/M4B, Matroska, Ogg, ID3v2 `CHAP`) through FFmpeg, in the same form |
| `CueSheet` | The parsed sheet: `CATALOG`, `CDTEXTFILE`, `TITLE`, `PERFORMER`, `SONGWRITER`, `REM` lines, `FILE`s and `TRACK`s with `INDEX`, `PREGAP`, `POSTGAP`, `FLAGS`, `ISRC`; `parse` takes bytes, `parse_text` text, `resolve` gives the tracks without touching the disk |
| `VirtualTrack` | A track number, a `MediaSource` with a `SourceSection::Span`, `TrackTags`, the `INDEX 00` position and the pre-emphasis flag |
| `CueTracks` | The tracks, where they came from (`CueOrigin`), the sheet and every warning |
| `CueWarning` | What was skipped or repaired, with its line |
| `CueTime` | `mm:ss:ff` at 75 frames per second; converts exactly to `MediaTime` |

The parser is tolerant: commands in any case, LF, CRLF or CR line ends, quoted or unquoted values, quotes inside quoted values, unknown commands and malformed lines skipped with a line-numbered warning. It is bounded: 1 MiB per sheet, 8 KiB per line, 1000 files, 1000 tracks, 100 indexes per track, 256 remarks per track or album, 256 warnings. It never panics; `fuzz/` has a target for it.

**Charsets.** A byte order mark decides (UTF-8, UTF-16); then UTF-16 without a mark; then valid UTF-8; then the legacy charset the user chose (`CueOptions::legacy_charset`), or else the one `chardetng` guesses from the whole sheet (cp1251, Shift-JIS, GBK, …).

**Paths.** `FILE` names are kept as bytes. Absolute paths, drive letters, network shares and URLs are rejected, `\` is a separator, and `..` may not leave the sheet's folder. A name in a legacy charset is tried both as UTF-8 and as its original bytes. A missing file is looked for in another case and with another audio extension (`Album.wav` beside `Album.flac`); tracks of a file found nowhere are kept with a warning.

**Pregaps.** A track starts at `INDEX 01` (without one: its lowest index above 01, else `INDEX 00`, with a warning) and ends at the next track's `INDEX 01` in the same file, so a pregap plays at the end of the previous track as on a CD. The last track of a file is open-ended. `INDEX 00` in the same file is `VirtualTrack::pregap_start`. `PREGAP` and `POSTGAP` are silence not stored in the file and stay on the parsed track.

**Tags.** Album `TITLE` and `PERFORMER` become the album and album artist; the track's `PERFORMER` (or else the album's) the artist; `SONGWRITER` a writer credit; `CATALOG` and `ISRC` the `barcode` and `isrc` extras; `REM DATE`, `GENRE`, `COMMENT`, `DISCNUMBER`, `TOTALDISCS`, `REPLAYGAIN_*`, `COMPOSER`, `LABEL` and the `MUSICBRAINZ_*` IDs their fields; other `REM` keys (`DISCID`, …) go to `extra` in lower case. A track's `REM` lines override the album's. The table is in `src/cue/fields.rs`.

## Tests

```sh
cargo test -p helikodu-meta
```

The integration tests in `tests/cue/` write sheets shaped like those found in the wild (EAC single-file rips with pregaps and `REM` lines, multi-file rips, CRLF, byte order marks, UTF-16, cp1251, Shift-JIS, malformed lines, hostile paths) and generate audio with the `ffmpeg` tool: FLAC and WavPack files with a `CUESHEET` tag, a FLAC file given a cue sheet block by `metaflac`, and Matroska and M4B files with chapters from an FFMETADATA file. A test whose tool is missing prints `SKIPPED:` and passes.

The integration tests in `tests/tags/` generate their media at test time in Cargo's temporary directory: a WAV file written by the tests, encoded with the `ffmpeg` tool with `-metadata`, then edited with lofty where a test needs frames FFmpeg cannot write (`POPM`, `UFID`, `TMCL`, `TIPL`, multi-valued fields, Latin-1 frames holding cp1251, Shift-JIS and GBK bytes). A test whose tool or encoder is missing prints `SKIPPED:` and passes.
