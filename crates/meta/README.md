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

lofty reads MP3 (ID3v2, APEv2, ID3v1), FLAC, Ogg Vorbis, Opus, Speex, MP4, Monkey's Audio, Musepack, WavPack, WAV (RIFF INFO, ID3v2), AIFF and ADTS AAC. FFmpeg reads the rest (Matroska, ASF/WMA, DSF, DFF, TAK, TTA); those tags are read-only. Cover art is not read here; see [Covers](#covers).

### Field mapping

`src/tags/mapping/` holds one table per tag format, following MusicBrainz Picard's tag mapping: ID3v2 frames and `TXXX` descriptions, Vorbis comments, APEv2 items, MP4 atoms and freeform names, RIFF INFO, and FFmpeg's dictionary (generic keys, ASF attributes, Matroska names). APEv2, `TXXX`, freeform atoms and FFmpeg fall back to the Vorbis names. A file with several tags (ID3v2, APEv2 and ID3v1 on one MP3) is read in order of preference: a single-valued field takes the first value found, and a multi-valued field comes entirely from the first tag that has it.

Multiple values are NUL-separated values in ID3v2 and APEv2, repeated Vorbis comments, and repeated MP4 data atoms. Genres are also split at `;`, and ID3 genre references such as `(17)` are resolved. Slash-joined lists are only split for MusicBrainz IDs.

### Charsets

`src/tags/charset.rs` repairs text written in legacy code pages (cp1251, Shift-JIS, GBK, …) and read as Latin-1, and UTF-8 bytes in frames declared Latin-1. Text is only touched when it can be turned back into its bytes and looks like mojibake, and the repair must decode cleanly into a non-Latin script; Western European text is left as it is. The charset is guessed per tag with `chardetng`, unless the user chose one.

### Ratings

`POPM` (by player e-mail: range table, linear for Quod Libet and Picard, MusicBee's half-star values), `FMPS_RATING`, `RATING` (0–5 or 0–100) and ASF `WM/SharedUserRating` are read onto the half-star scale. The table is in `src/tags/rating.rs`.

## Writing tags

```rust
use helikodu_meta::tags::writing::{
    BatchItem, SelectionView, TagChanges, TagField, WriteOptions, load_for_editing, write_batch,
};

let files: Vec<_> = paths.iter().map(|p| load_for_editing(p, &ReadOptions::default())).collect::<Result<_, _>>()?;
let view = SelectionView::of(&files);             // per field: Shared::Same(values) or Shared::Mixed(counts)
let mut changes = TagChanges::new();
changes.set_one(TagField::Album, "Kid A").set(TagField::Genre, ["Electronic", "Art Rock"]);
let items: Vec<_> = files.iter().map(|f| BatchItem::for_file(f, changes.clone())).collect();
let report = write_batch(&items, &WriteOptions::default(), &mut |progress| {}, &|| false);
```

| Type | What it is |
|---|---|
| `write_tags` | Write a `TagChanges` into one file, refusing it if its `EditStamp` no longer matches; returns a `WriteOutcome` (`WriteMode`, the new stamp, the ID3v2 version written, changes the tag could not take) or a `WriteError` |
| `TagChanges` | Per field `FieldChange::Set(values)` or `Clear` (`TagField` by Picard name, or `FieldKey::Custom` for "Add field"), `CoverChange` (`SetFront` with a JPEG or PNG, `RemoveFront`, `RemoveAll`), lyrics (set or remove) and rating (set or remove). Values are checked before the file is touched: numbers, dates (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`, optional time), the compilation flag, one value for a single-valued field, custom names a format can carry |
| `WriteOptions` | The ID3v2 version of new tags (`Id3Version::V2_4` by default; an existing tag keeps its version) and the size above which files are changed in place (2 GiB by default) |
| `load_for_editing`, `EditableFile` | A file as the editor opens it: stamp, tags, audio, whether it can be written, its tag formats (`ID3v2.4`, `APEv2`, …), lyrics and front cover hash |
| `SelectionView`, `Shared` | The values a selection shares, field by field, cover, lyrics and rating: `Same(value)` or `Mixed` with each value and how many files have it |
| `write_batch`, `BatchItem`, `BatchProgress`, `BatchReport` | Many files in turn: one result each, progress after each file, cancellation between files (`WriteError::Cancelled` for the rest); a failure does not stop the others |
| `field_values`, `read_lyrics`, `planned_mode`, `is_writable` | A field's values as the editor shows them; the lyrics of a file; whether a write would replace the file or change it in place; whether a file's tags can be written |
| `remove_stale_temporaries`, `is_temporary_name` | Clean-up of the temporary copies a crash left behind (hidden files ending in `.helikodu-tmp`) |

**Formats.** The main tag gets every change and is created when missing: ID3v2 in MP3, ADTS AAC, WAV and AIFF; Vorbis comments in FLAC, Ogg Vorbis, Opus and Speex; `ilst` in MP4; APEv2 in WavPack, Monkey's Audio and Musepack. The other tags a file already has (APEv2 and ID3v1 on MP3, RIFF `INFO` in WAV, AIFF text chunks) get the changes they can hold, so no stale value in them reads back. Matroska, ASF, DSF, DFF, TAK, TTA and files lofty cannot parse but FFmpeg can are `WriteError::ReadOnlyFormat`. Unknown frames, keys, atoms and pictures stay as they are.

**Keys.** Each field is written under the key Picard writes (`src/tags/writing/` has one module per format: `id3v2.rs`, `vorbis.rs`, `ape.rs`, `mp4.rs`, `secondary.rs`), and every key the reading tables map to the same field is removed first, so `ALBUM ARTIST` and `ALBUMARTIST` never disagree. A unit test checks each written key against the reading table. Several values are NUL-separated in ID3v2 and APEv2, repeated comments in Vorbis, several data atoms in MP4, and `; `-joined where a tag holds one string (RIFF, AIFF, ID3v1). Track, disc and movement numbers are written with their totals (`TRCK 3/12`, `trkn`, `TRACKNUMBER` and `TRACKTOTAL`). Dates go to ID3v2 timestamp frames. Performers with an instrument go to `TMCL`; arrangers, producers, engineers, mixers and DJ mixers to `TIPL`; the MusicBrainz recording to `UFID`. In ID3v2.3, which lacks them, `TSOT`, `TSOP`, `TSOA`, `TSST`, `TMOO`, `TDRL` and `TMCL` become `TXXX` frames under the Vorbis names, and several genres are written as one `; ` list.

**Ratings.** A rating is written as ID3v2 `POPM` with the e-mail field `Helikodu` (1, 64, 128, 196 and 255 for one to five stars, a half star rounded up; no frame for zero stars) and `TXXX:FMPS_Rating`; as `FMPS_RATING` (0.0 to 1.0) and `RATING` (0 to 100; none for zero stars) in Vorbis comments and APEv2; and as freeform `FMPS_Rating` and `RATING` atoms in MP4. Removing a rating removes these and leaves other players' `POPM` frames alone. The reader prefers `FMPS_RATING`, so the exact half-star value reads back.

**Safe saving.** The file is copied to a hidden temporary file in its folder (same permissions and, where allowed, owner), edited there, read back and checked (the same sample rate, channels, codec and length; every change as written; the cover by its hash), flushed, checked against the stamp once more, and renamed over the original; then the folder is flushed. Any failure removes the copy and leaves the original as it was. A symbolic link is followed and stays a link. Files above the in-place limit, and files with several hard links, are changed in place (`WriteMode::InPlace`), reported so that a player can wait for the end of a playing track; a check that fails then says `in_place: true`. A write always gives the file a new modification time, as any change does, so a rescan sees it.

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

## Covers

```rust
use helikodu_meta::covers::{CoverLimits, CoverOptions, find_cover, make_thumbnail};

let options = CoverOptions::default();
if let Some(cover) = find_cover(path, &options) {
    let hash = cover.hash()?;                 // SHA-256 of the encoded bytes
    let thumb = make_thumbnail(cover.bytes()?, 256, &options.limits)?; // RGBA, fits 256 × 256
    let file = if thumb.is_opaque() { thumb.encode_jpeg(85)? } else { thumb.encode_png()? };
}
```

| Type | What it is |
|---|---|
| `read_embedded_covers` | A file's embedded pictures: ID3v2 `APIC`, APEv2 `Cover Art (…)`, FLAC `PICTURE`, Vorbis `METADATA_BLOCK_PICTURE` and MP4 `covr` through lofty; FFmpeg's attached pictures and Matroska image attachments when lofty does not know the format or cannot parse the file. Pictures over a limit or in a format that is not decoded are listed as skipped |
| `find_folder_covers` | The image files beside a track, best first, without reading them |
| `find_cover`, `choose_front` | One cover for a track: the best embedded picture, then the first folder image that is a JPEG, PNG or WebP (or the other way round, `CoverPreference`) |
| `Cover` | A `CoverSource` (embedded: tag kind, picture type, position, description; or a folder file's path) and the encoded bytes, read from a folder file only when first needed |
| `CoverHash` | SHA-256 of the encoded bytes; equal pictures have equal hashes wherever they were found |
| `PictureFormat` | The format sniffed from the magic bytes (JPEG, PNG, WebP decoded; GIF, BMP, TIFF, AVIF, HEIF, JPEG XL recognised); declared media types and extensions are ignored |
| `probe_picture`, `decode_picture`, `make_thumbnail` | Header-only format and dimensions; decoding into `RgbaPicture`; decoding and scaling to fit a square |
| `RgbaPicture` | 8-bit RGBA pixels; `scale_to_fit` (Lanczos 3, keeps the aspect ratio, never enlarges), `encode_png`, `encode_jpeg` |
| `CoverLimits` | 32 MiB per picture, 64 Mi pixels (8192 × 8192), 16 pictures per file |

**Choosing the embedded picture.** The picture type decides: the front cover, then "other" or untyped pictures (MP4 has no types), then any other type, then the back cover, then file icons. Among pictures of the same rank the largest in bytes wins, then the first.

**Folder images.** The track's folder is searched, then the parent of a disc folder (`CD1`, `Disc 2`, `disk_03 - Bonus`, …); nothing else, and not recursively. Names are compared ignoring case, with the extension `jpg`, `jpeg`, `png` or `webp`. The order: folder (the track's own first); name (`cover`, `folder`, `front`, `albumart`, `album`, `artwork`, `art`, or the configured list; then Windows Media Player's `AlbumArt_{…}_Large`; then the album title; then `AlbumArt_{…}_Small`; then `AlbumArtSmall`); extension in the order above; then the file name's bytes. Paths are kept as bytes.

**Hostile pictures.** A picture over the byte limit is not copied. Decoding checks the byte size, sniffs the format, parses the header and checks the declared width × height against the pixel limit before any pixel buffer exists; the `image` crate's own allocation limit applies while decoding. Only JPEG, PNG and WebP are decoded, by memory-safe Rust decoders; malformed or truncated input is an error, never a panic. `fuzz/` has a target for the decoder.

## Playlists

```rust
use helikodu_meta::playlists::{ParseOptions, PlaylistKind, WriteOptions, read_playlist, write_playlist};

let parsed = read_playlist(path, &ParseOptions::default())?;   // kind from the extension, else sniffed
for entry in &parsed.playlist.entries {
    println!("{:?} {:?}", entry.location, entry.hints.title);
}
let written = write_playlist(&parsed.playlist, PlaylistKind::M3u8, &WriteOptions::default());
```

| Type | What it is |
|---|---|
| `PlaylistFormat` | The seam a format plugs into: `parse` (bytes and `ParseOptions` → `ParsedPlaylist`) and `write` (`Playlist` and `WriteOptions` → `WrittenPlaylist`). `M3u`, `M3u8`, `Pls` and `Xspf` implement it, one file each in `src/playlists/` |
| `PlaylistKind` | The formats; `format`, `from_path` (by extension), `sniff` (XML is XSPF, `[playlist]` is PLS, anything else M3U), `is_playlist_path`. M3U8 is the default for writing |
| `read_playlist`, `parse_playlist`, `write_playlist` | Read a file (relative entries resolve against its folder), parse bytes, write bytes |
| `Playlist`, `PlaylistEntry` | The title and the entries; an entry's `EntryLocation`, its `undecoded_path` (see charsets), its `EntryHints` (title, artist, album, length, track number) and its line |
| `EntryLocation` | `Local` (a path as bytes), `Foreign` (a drive letter, a network share, or a path rooted at the current drive, with `/` separators) or `Remote` (a URL; kept, never fetched) |
| `ParseOptions`, `PlaylistLimits` | The playlist's folder, the legacy charset the user chose, and the bounds: 16 MiB per file, 100 000 entries, 32 KiB per line |
| `WriteOptions`, `PathStyle` | Relative paths (to the playlist's folder, when they share more than the root) or absolute ones |
| `PlaylistWarning`, `WriteWarning` | What was skipped or repaired, with its line; what a format could not carry |

**Reading.** M3U: one location per line, blank lines and `#` comments skipped; `#EXTINF:<seconds>[ attributes],<artist> - <title>`, `#EXTART:` and `#EXTALB:` describe the next entry, `#PLAYLIST:` names the playlist, `#EXTM3U` is optional. PLS: the `[playlist]` section's `File<n>`, `Title<n>` (split like `#EXTINF`), `Length<n>` and `NumberOfEntries`, in any case and in the order of the numbers. XSPF (`quick-xml`): `playlist/title` and each track's `location` (the first local one), `title`, `creator`, `album`, `duration` and `trackNum`, with namespace prefixes and case ignored. The parsers are tolerant (LF, CRLF or CR; malformed lines and elements skipped with a line-numbered warning, at most 256 warnings) and bounded, and never panic. XSPF expands only the predefined entities and character references, never a document type's entities, nests at most 64 levels, and keeps the tracks before malformed XML. `fuzz/` has a target for each parser.

**Locations.** A URL with a scheme of two letters or more is `Remote`; a `file:` URL on this host is percent-decoded into bytes, and on another host it is a network share. A drive letter, a leading `\\`, or `\` as the only separator marks a Windows path: `\` becomes `/`, and a path with a drive or a share, or rooted at the current drive, is `Foreign`. Other paths are `Local`: relative ones are joined to the playlist's folder, and `.` and `..` resolve lexically. Entries naming other playlists (M3U, M3U8, PLS, XSPF, JSPF, ASX, WPL) are skipped with a warning, not followed, so nested and looping playlists end at once.

**Charsets.** A byte order mark decides (UTF-8, UTF-16); then UTF-16 without a mark. M3U8 and XSPF are UTF-8: bytes that are not are replaced in the text, and an M3U8 path keeps its bytes. M3U and PLS are UTF-8 when valid, and otherwise in the charset the user chose (`ParseOptions::legacy_charset`) or the one `chardetng` guesses from the whole file; a path's decoded form is the location and its original bytes are `undecoded_path`, since old collections may still have names in that charset.

**Writing.** M3U and M3U8 with `#EXTM3U`, `#PLAYLIST:`, `#EXTINF` and `#EXTALB`; PLS version 2; XSPF in UTF-8. M3U and PLS write a path's own bytes; M3U8 writes a path that is not UTF-8 as decoded text and warns that it may not resolve; XSPF percent-encodes every byte outside the unreserved set in `file:` URIs (or relative references), so any name survives. A path holding a line break cannot be written to M3U or PLS and is left out with a warning; a name starting with `#` is written as `./#…`.

## Scan workers

The `worker` module is the protocol between the library scanner and the `helikodu-scan-worker` processes, which read one file at a time so that a decoder that crashes or hangs on a broken file cannot take the application down.

- Messages are binary frames: a little-endian 32-bit length, then the payload, at most 64 MiB. A worker greets with a magic value, the protocol version and its process ID; the scanner refuses another version.
- A request names one file (its path as bytes) and the reading options. The answer is a report with the file's size, a quick hash, its tags, a CUE sheet or chapters if it has them, or a failure with its kind.
- `quick_hash` is SHA-256 over the file size and its first and last 64 KiB, truncated to 16 bytes. It lets the library recognise a moved file without reading all of it.
- `serve` runs the worker side over any reader and writer; `handle` answers a single request.

## Tests

```sh
cargo test -p helikodu-meta
```

The integration tests in `tests/cue/` write sheets shaped like those found in the wild (EAC single-file rips with pregaps and `REM` lines, multi-file rips, CRLF, byte order marks, UTF-16, cp1251, Shift-JIS, malformed lines, hostile paths) and generate audio with the `ffmpeg` tool: FLAC and WavPack files with a `CUESHEET` tag, a FLAC file given a cue sheet block by `metaflac`, and Matroska and M4B files with chapters from an FFMETADATA file. A test whose tool is missing prints `SKIPPED:` and passes.

The integration tests in `tests/tags/` generate their media at test time in Cargo's temporary directory: a WAV file written by the tests, encoded with the `ffmpeg` tool with `-metadata`, then edited with lofty where a test needs frames FFmpeg cannot write (`POPM`, `UFID`, `TMCL`, `TIPL`, multi-valued fields, Latin-1 frames holding cp1251, Shift-JIS and GBK bytes). A test whose tool or encoder is missing prints `SKIPPED:` and passes.

The integration tests in `tests/writing/` encode a file per format with the `ffmpeg` tool (MP3, ADTS AAC, FLAC, Ogg Vorbis, Opus, MP4, WavPack, WAV, AIFF, and Matroska as a read-only format) and write and read back fields with several values, custom fields, numbers, dates, credits, lyrics, covers and ratings, then clear them; they check the ID3v2 version of new and existing tags, the refusal of a file changed since it was read, in-place writes above a small limit, hard and symbolic links, the rating frames, and a batch over a selection with mixed values, a stale file and a cancellation. Unit tests in `src/tags/writing/tests.rs` check that a failed verification leaves the original untouched and no temporary file behind.

The integration tests in `tests/covers/` encode pictures with the `image` crate and write hostile ones byte by byte (a PNG header claiming 100000 × 100000 pixels, a JPEG claiming 65535 × 65535, a 4096 × 4096 PNG of a few kilobytes against a smaller limit, truncated and corrupted JPEG, PNG and WebP). Audio files get their pictures from the `ffmpeg` tool (`-disposition attached_pic` for MP3, FLAC and M4A; `-attach` for Matroska) or from lofty (several `APIC` types, two FLAC `PICTURE` blocks, an Opus `METADATA_BLOCK_PICTURE`, APEv2 cover items in WavPack). Folder tests build album folders with disc subfolders, mixed-case names and non-UTF-8 names.

The integration tests in `tests/playlists/` write playlist files shaped like those other players write (Windows M3U with a byte order mark, CRLF and backslashes, drive letters and network shares; cp1251 M3U read with a guessed and a chosen charset, and the same bytes as M3U8; UTF-16 M3U; relative, absolute and `file://` entries; URLs kept and never fetched; playlists naming each other; files over the size, entry and line limits; a VLC XSPF) and write and read back every format with relative and absolute paths, including a name that is not UTF-8.
