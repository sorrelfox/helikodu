# Test corpus

Recipes for the generated test corpus: samples of every format the encoders on the machine can write, gapless sets, video files, broken files and file names that are not valid UTF-8. `cargo xtask corpus` writes the files and a checksum manifest to `target/corpus/`. Media files are never committed.

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
| `names/` | Names in Latin-1, cp1251 and Shift-JIS bytes, a folder whose name is not UTF-8, a name of exactly 255 bytes, names with a newline, a leading dash, leading and trailing spaces and characters Windows forbids, and the same name in Unicode NFC and NFD |

The summary line counts the distinct `format` labels in `formats/`. Each label is one codec in one container; variants such as another bit depth, channel layout or bit-rate mode share their label and do not count again.

Not generated yet, and to be added as recipes: the tag set (every tag format, legacy encodings, embedded pictures, lyrics, ReplayGain and R128, ratings, sort and classical fields), CUE sheets (single-file, multi-file, embedded, pregaps, non-UTF-8), titles in many scripts and locale-sensitive names, oversized tags and pictures, and playlists (nested, looping, referring to URLs). Synthetic libraries for benchmarks come from `cargo xtask synth-library` (see `xtask/README.md`). Formats without a Linux encoder (TAK, ATRAC, WMA Pro and Lossless, DTS-HD MA, DSD, MP1, many game formats) need a separate, licence-reviewed sample repository referenced by checksum.

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
| `set` | `formats`, `gapless`, `video`, `broken` or `names`; also the folder |
| `name` | Path inside the set's folder, percent-encoded (below) |
| `format` | Formats set only: the codec and container label |
| `source` | The source the tool reads |
| `from` | Instead of `source`: an earlier sample, as `set/name` |
| `parts` | `true` makes one file per part of a split source; `name` contains `{part}` |
| `tool` | A program (`ffmpeg`, `flac`, `lame`, `opusenc`, `wavpack`, `sox`, …) or a built-in step: `copy`, `truncate` (keeps the fraction `keep`), `garbage` (keeps `keep_bytes`, replaces the rest with noise from `seed`), `empty` |
| `args` | Arguments; `{input}` and `{output}` are replaced by paths |
| `needs` | FFmpeg features: `encoder:<name>`, `muxer:<name>` |
| `optional` | The tool may fail on builds that lack a feature `needs` cannot see; the recipe is then skipped |
| `extra_outputs` | Extensions of further files the tool writes next to the output (`wvc`) |
| `pad_name_to` | Pad the file name to this many bytes |
| `expect` | What `ffprobe` must report (below) |

For `ffmpeg`, `xtask` adds the source as the first `-i`, then the recipe's arguments, then `-map_metadata -1 -map_chapters -1 -fflags +bitexact -flags +bitexact -threads 1` and the output. Other programs get exactly `args`. Tools write to a scratch file with a plain name, which is then moved to its final name, so names that start with a dash or contain a newline never reach a command line.

A recipe is skipped with a notice when its program is not installed or FFmpeg lacks something in `needs`. The reference encoders `oggenc`, `mac` and `mpcenc` have recipes, so they are used where they are installed.

`expect` keys: `demuxer` (`ffprobe`'s `format_name`), `codec`, `sample_rate`, `channels` (these two default to the source's), `samples` (decoded frames of the first audio stream), `exact_length` (the decoded length equals the source's or the part's), `audio_streams`, `video_streams`. A file with `expect` is checked when it is made; a mismatch fails the recipe. Without `ffprobe` the checks are skipped with a notice (an error with `--strict`).

## Names

Recipes and the manifest are UTF-8, but some names are not. Names are therefore percent-encoded: `%XX` is the byte `0xXX`, any other character is its UTF-8 bytes, and a literal `%` is `%25`. In the manifest, control characters and bytes that are not valid UTF-8 are escaped; everything else is written as is.

## Manifest

`target/corpus/manifest.toml` lists every file with its set, format label, SHA-256 checksum, size, tool, expectations and a fingerprint (a hash of the recipe, the tool's version and the input's checksum). It also records the versions of the programs used, the checksum of `recipes.toml`, the number of formats and the skipped recipes with the reason. `--check` fails when a file is missing, changed or not listed, or when `recipes.toml` changed since the corpus was made.

## Determinism

With the same tool versions, a run gives the same bytes: the sources are computed exactly, FFmpeg runs with the bit-exact options above, Ogg serial numbers are fixed (`--serial`, `--serial-number`), SoX runs with `-R -D` (repeatable, no dither) and an empty comment, and broken files use seeded noise. Checksums are therefore stable for a pinned tool set, such as the CI container image, but differ between FFmpeg or encoder versions.
