# Test corpus

Recipes for the generated test corpus: samples of every format the encoders on the machine can write, gapless sets, video files, broken files, file names that are not valid UTF-8, tags of every format, CUE sheets and text in many scripts. `cargo xtask corpus` writes the files and a checksum manifest to `target/corpus/`. Media files are never committed.

```sh
cargo xtask corpus            # make what is missing or out of date
cargo xtask corpus --check    # verify target/corpus against its manifest
cargo xtask corpus --strict   # fail instead of skipping when a tool is missing
```

A run makes only the files whose recipe, tool version or input changed; everything else is kept. Files in `target/corpus/` that no recipe makes any more are removed. A failing recipe does not stop the run, but the run fails at the end.

## Sets

| Folder | Content |
|---|---|
| `sources/` | The signals every encoder starts from, written by `xtask` itself: a linear sweep from 20 Hz with noise at -40 dBFS, each channel starting the sweep at a different point. Several rates, bit depths and layouts (mono to 5.1) |
| `formats/` | One sample per codec and container: PCM variants in WAV, RF64, Wave64, AIFF/AIFF-C, CAF, AU, VOC, IRCAM, 8SVX, NIST SPHERE, AVR and HCOM; FLAC (native, Ogg, Matroska, MP4, NUT), ALAC, WavPack (with a hybrid `.wv` + `.wvc` pair), TTA, MLP, TrueHD; MP3 (MPEG-1, 2 and 2.5), MP2, AAC (ADTS, M4A, LATM, MPEG-TS), Vorbis, Opus, Speex, AC-3, E-AC-3, DTS, WMA v1/v2, RealAudio 1.0, Nellymoser, Bluetooth codecs (SBC, aptX, aptX HD, LC3) and speech codecs (AMR-NB/WB, GSM, G.722, G.723.1, G.726, iLBC, Codec 2) |
| `gapless/` | Two references (44.1 kHz and 48 kHz), each split into four parts at points that are not frame boundaries; every part encoded with each available encoder (WAV, FLAC, WavPack, TTA, ALAC, MP3 from LAME and from FFmpeg, AAC from FFmpeg and FDK, AAC in ADTS without gapless data, Vorbis, Opus in Ogg and WebM). The references are `gapless/reference-*.wav` |
| `video/` | Two-second clips: H.264, H.265, VP9, AV1, MPEG-2, MPEG-4, Theora, H.263, WMV2, RealVideo and DV with AAC, Opus, AC-3, MP2, MP3, Vorbis, AMR-NB, WMA, RealAudio, FLAC or PCM audio, in MP4, Matroska, WebM, MPEG-PS, VOB, MPEG-TS, AVI, Ogg, FLV, 3GP, ASF, RM, DV, MXF and NUT; a Matroska file with three audio streams in different languages where the second is the default; a video-only file |
| `broken/` | Truncated files, zero-length files, valid headers followed by noise, pure noise with an audio extension, and a FLAC file named `.mp3` |
| `longtail/` | Files of the formats the engine plays with its own decoders, written byte by byte by `xtask` (`tool = "write"`, the writers in `crates/testkit/src/longtail.rs`): a ProTracker module with two subsongs, an NSF file with three tracks, a PSID tune with two songs, a MIDI file, the SoundFont the MIDI files play with, and copies of the module and the MIDI file under misleading names. The folder below `longtail/` names the engine backend that must claim the file. They are the project's own work under its licence, like everything generated here. The broken set has truncated copies |
| `tags/` | Tags written byte by byte (`tool = "tag"`) into untagged files (`tags/untagged/`, which read as untagged): Vorbis comments in FLAC, Ogg Vorbis, Opus and Speex; ID3v2.4, ID3v2.3, ID3v1 and all three with APEv2 on one MP3; ID3v2 in ADTS, WAV and AIFF; RIFF INFO; APEv2 in WavPack, MP3, Monkey's Audio, Musepack and TTA; MP4 atoms in M4A; Matroska and ASF tags written by FFmpeg. Several values of one field, sort fields, classical fields (composer, conductor, performer, work, movement name and number), ReplayGain and Opus R128 gains, lyrics (`USLT`, LRC in `LYRICS`, `©lyr`, APEv2 `Lyrics`), pictures of several types, ratings as players write them (`POPM` from Windows Media Player, MusicBee, Quod Libet and several players in one file, `RATING` in stars and out of 100, `FMPS_RATING`), and Windows-1251, Shift-JIS and GBK bytes in ID3v1, in ID3v2.3 Latin-1 frames and (Windows-1251) in RIFF INFO |
| `cue/` | CUE sheets (`tool = "text"`) beside 8 s of silent FLAC: a single-file sheet with a pregap (`INDEX 00`), hidden track one audio, a multi-file sheet with one file per track (with the gaps appended to the previous file, and without), a sheet in the `CUESHEET` Vorbis comment of the file it splits, and sheets in Windows-1251 (naming its file in Cyrillic, with the file name in Windows-1251 bytes on disk), Shift-JIS, Latin-1 and UTF-8 with a byte order mark |
| `scripts/` | Titles, artists and albums in Chinese, Japanese, Korean, Thai, Cyrillic, Greek, Arabic and Hebrew, in Vorbis comments, ID3v2.3, ID3v2.4 and MP4 atoms; in `locales/`, Swedish, German and Estonian artist names whose order differs between the root order, German, German phonebook, Swedish and Estonian |
| `names/` | Names in Latin-1, cp1251 and Shift-JIS bytes, a folder whose name is not UTF-8, a name of exactly 255 bytes, names with a newline, a leading dash, leading and trailing spaces and characters Windows forbids, and the same name in Unicode NFC and NFD |

**Unsupported by design:** the raw, headerless streams in `formats/` (Bluetooth SBC, aptX, aptX HD, DFPWM, G.722, G.723.1) have nothing in their bytes to recognise them by; FFmpeg identifies them only by file name extension, and the player never lets a file name decide what a file is. They are not supported as files. Their recipes stay, so the engine's corpus tests can check that such files are refused cleanly; the result tables list them as "unsupported by design", which is neither a gap nor a failure, also with `HELIKODU_CORPUS_STRICT=1`. One of them that starts to play fails the tests.

The summary line counts the distinct `format` labels in `formats/`. Each label is one codec in one container; variants such as another bit depth, channel layout or bit-rate mode share their label and do not count again.

Not generated yet, and to be added as recipes: oversized tags and pictures, and playlists (nested, looping, referring to URLs). Synthetic libraries for benchmarks come from `cargo xtask synth-library` (see `xtask/README.md`). Formats without a Linux encoder (TAK, ATRAC, WMA Pro and Lossless, DTS-HD MA, DSD, MP1, the game audio formats of vgmstream, and further tracker and game music formats) need a separate, licence-reviewed sample repository referenced by checksum.

## Recipes

`recipes.toml` holds two kinds of tables.

A `[[source]]` is a signal `xtask` computes and writes to `sources/<name>.wav`:

| Key | Meaning |
|---|---|
| `name` | ASCII letters, digits and dashes |
| `sample_rate`, `channels`, `frames` | Frames are samples per channel |
| `encoding` | `s16`, `s24`, `s32` or `f32` |
| `signal` | `sweep-noise` or `silence` |
| `seed` | Seed of the noise (default 1) |
| `split` | Frame positions; the parts are also written, as `sources/<name>.part<N>.wav` |

The signal uses basic floating-point arithmetic only, so the sources are bit-identical on every machine.

A `[[sample]]` is one corpus file, made in the order of the file:

| Key | Meaning |
|---|---|
| `set` | `formats`, `gapless`, `video`, `broken`, `names`, `longtail`, `tags`, `cue` or `scripts`; also the folder |
| `name` | Path inside the set's folder, percent-encoded (below) |
| `format` | Formats set only: the codec and container label |
| `source` | The source the tool reads |
| `from` | Instead of `source`: an earlier sample, as `set/name` |
| `parts` | `true` makes one file per part of a split source; `name` contains `{part}` |
| `tool` | A program (`ffmpeg`, `flac`, `lame`, `opusenc`, `wavpack`, `sox`, …) or a built-in step: `copy`, `truncate` (keeps the fraction `keep`), `garbage` (keeps `keep_bytes`, replaces the rest with noise from `seed`), `empty`, `write` (writes the file `content` names), `tag` (writes the `tag` tables into a copy of `from`), `text` (writes `text` in `charset`) |
| `content` | `write` only: `tracker-module`, `nsf`, `psid`, `midi` or `soundfont` |
| `tag` | `tag` only: the tags to write, as `[[sample.tag]]` tables (below) |
| `text`, `charset` | `text` only: the text, and its charset: `utf-8` (the default), `utf-8-bom`, `latin1` or a WHATWG label such as `windows-1251` or `shift_jis` |
| `args` | Arguments; `{input}` and `{output}` are replaced by paths |
| `needs` | FFmpeg features: `encoder:<name>`, `muxer:<name>` |
| `optional` | The tool may fail on builds that lack a feature `needs` cannot see; the recipe is then skipped |
| `extra_outputs` | Extensions of further files the tool writes next to the output (`wvc`) |
| `pad_name_to` | Pad the file name to this many bytes |
| `expect` | What `ffprobe` must report (below) |

For `ffmpeg`, `xtask` adds the source as the first `-i`, then the recipe's arguments, then `-map_metadata -1 -map_chapters -1 -fflags +bitexact -flags +bitexact -threads 1` and the output. Other programs get exactly `args`. Tools write to a scratch file with a plain name, which is then moved to its final name, so names that start with a dash or contain a newline never reach a command line.

A recipe is skipped with a notice when its program is not installed or FFmpeg lacks something in `needs`. The reference encoders `oggenc`, `mac` and `mpcenc` have recipes, so they are used where they are installed.

A `[[sample.tag]]` table is one tag: `format` (`vorbis`, `id3v1`, `id3v23`, `id3v24`, `ape`, `mp4` or `riff`), `fields` (each a list of the key and its values) and `pictures` (`{ type = 3, size = 16, description = "Front" }`: a square PNG in one colour, of an ID3v2 picture type). Keys are the format's own: Vorbis comment names; ID3v2 frame IDs, `TXXX:<description>`, `COMM`, `USLT`, `POPM:<email>` (the rating byte, optionally the play count) and `UFID:<owner>`; ID3v1 `TITLE`, `ARTIST`, `ALBUM`, `YEAR`, `COMMENT`, `TRACK` and `GENRE` (its number); APEv2 item keys; MP4 atom names (`©nam`, `trkn` as `N/M`, `----:<mean>:<name>`); RIFF INFO chunk IDs. Several values become one Vorbis comment each, or one NUL-separated ID3v2 frame or APEv2 item, or several MP4 `data` atoms. `charset` writes the text of ID3v1, ID3v2 (as Latin-1 frames), APEv2 and RIFF INFO in a legacy charset, as old taggers did. The step knows FLAC, Ogg Vorbis, Opus and Speex (`vorbis`), MP3 and ADTS (`id3v23`, `id3v24`, `ape`, `id3v1`), WavPack, Monkey's Audio, Musepack and TTA (`ape`, `id3v1`), MP4 (`mp4`, with the `moov` box after the media data), WAV (`riff`, `id3v23`, `id3v24`) and AIFF (`id3v23`, `id3v24`); tags already in the same place are replaced.

`expect` keys: `demuxer` (`ffprobe`'s `format_name`), `codec`, `sample_rate`, `channels` (these two default to the source's), `samples` (decoded frames of the first audio stream), `exact_length` (the decoded length equals the source's or the part's), `audio_streams`, `video_streams`. A file with any of these is checked when it is made; a mismatch fails the recipe. Without `ffprobe` the checks are skipped with a notice (an error with `--strict`).

Further `expect` keys record what the tests of the tag and CUE readers check; they are not checked with `ffprobe`:

| Key | Meaning |
|---|---|
| `tags` | Tag fields a reader must find, by the field's name in the tag record: `title`, `title_sort`, `artist`, `artists`, `artist_sort`, `album`, `album_sort`, `album_artist`, `album_artists`, `album_artist_sort`, `composer_sort`, `track_number`, `track_total`, `disc_number`, `disc_total`, `date`, `original_date`, `genres`, `composers`, `conductors`, `performers` (`name (instrument)`), `lyricists`, `writers`, `labels`, `catalog_numbers`, `comment`, `has_lyrics`, `rating` (half stars, 0 to 10), `compilation`, `grouping`, `work`, `movement`, `movement_number`, `movement_total`, `track_gain`, `track_peak`, `album_gain`, `album_peak` (numbers), `r128_track_gain`, `r128_album_gain` and `extra:<key>`. A value or a list; an empty list means the field is not set. For a CUE sheet: the album fields every track carries |
| `charset` | The legacy charset a reader must be told (`gbk`); unset when the charset is to be recognised |
| `pictures` | The embedded pictures in file order: `type` (unset for MP4) and `size` in pixels |
| `lyrics` | The embedded lyrics: `synced`, the number of `lines` and the `first_line` |
| `cue` | The tracks of a `.cue` file or of the sheet embedded in the file: `number`, `file` (relative to the sheet's folder, percent-encoded), `start`, `end` (unset for the end of the file) and `pregap` as CUE times `MM:SS:FF`, `title`, `artist` |
| `sort` | The file's position, from 1, when the files of its folder are sorted by artist in each locale (`und` is the root order) |

The tests that use them: `crates/meta/tests/corpus/` (tags, pictures, lyrics, CUE sheets) and `crates/library/tests/corpus.rs` (a scan of the three sets, and the locale order).

## Names

Recipes and the manifest are UTF-8, but some names are not. Names are therefore percent-encoded: `%XX` is the byte `0xXX`, any other character is its UTF-8 bytes, and a literal `%` is `%25`. In the manifest, control characters and bytes that are not valid UTF-8 are escaped; everything else is written as is.

## Manifest

`target/corpus/manifest.toml` lists every file with its set, format label, SHA-256 checksum, size, tool, expectations and a fingerprint (a hash of the recipe, the tool's version and the input's checksum). It also records the versions of the programs used, the checksum of `recipes.toml`, the number of formats and the skipped recipes with the reason. `--check` fails when a file is missing, changed or not listed, or when `recipes.toml` changed since the corpus was made.

## Determinism

With the same tool versions, a run gives the same bytes: the sources are computed exactly, FFmpeg runs with the bit-exact options above, Ogg serial numbers are fixed (`--serial`, `--serial-number`), SoX runs with `-R -D` (repeatable, no dither) and an empty comment, and broken files use seeded noise. Checksums are therefore stable for a pinned tool set, such as the CI container image, but differ between FFmpeg or encoder versions.
