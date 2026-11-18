# Fuzzing

Every parser of untrusted input and the FFmpeg bridge have a fuzz target in [`fuzz/`](../../fuzz/README.md), and a regular test reads a corpus of damaged media files. This page says how they are run, how a finding is handled, and what the campaigns found.

## What is fuzzed

| Input | Target |
|---|---|
| Media files, as the player opens them (FFmpeg bridge: probe, metadata, pictures, decoding, seeking) | `ffmpeg_bridge_open` |
| Media files, as the scanner reads them (tags through lofty and FFmpeg, the embedded CUE sheet, chapters, covers) | `tags_read` |
| The scanner–worker protocol | `worker_message` |
| DSF and DSDIFF files (the own DSD reader) | `dsd_open` |
| Tracker modules, game music, SID tunes, MIDI files | `openmpt_open`, `gme_open`, `sid_open`, `midi_open` |
| Embedded and folder covers | `cover_decode` |
| CUE sheets | `cue_parse` |
| LRC lyrics | `lyrics_parse` |
| M3U, M3U8, PLS and XSPF playlists, by format and sniffed as the automatic import reads them | `playlist_m3u`, `playlist_pls`, `playlist_xspf`, `playlist_sniff` |
| The query language | `query_parse` |
| Naming patterns of the organizer, the converter and folder sync | `naming_pattern` |
| AutoEQ `ParametricEQ.txt` files | `autoeq_parse` |
| `settings.toml`, `ui-state.toml`, `export.toml` | `settings_toml`, `ui_state_toml`, `export_toml` |
| The manifest of a sync destination, `.helikodu-sync.json` | `sync_manifest` |
| The encoder's settings and samples | `ffmpeg_encode` |

The D-Bus interface's arguments arrive typed; a unit test feeds every method extreme and malformed values instead (`hostile_arguments_are_read_or_refused` in `crates/ui-gtk/src/remote_api.rs`). ASX and WPL playlists are not read yet, so they have no target.

## Running a campaign

Fuzzing needs a nightly toolchain. `fuzz/campaign.sh` builds the targets with the flags of `cargo fuzz` (libFuzzer, coverage, AddressSanitizer, overflow and debug assertions) and runs each for a fixed time, several at once:

```sh
fuzz/campaign.sh 900 8        # 15 minutes per target, 8 at a time
```

Run one before a release, after a change to a parser or to the FFmpeg bridge, and after an FFmpeg update. Keep the corpora in `fuzz/runs/` between campaigns: each run starts from what the last one found.

The system's C libraries are not instrumented. libFuzzer cannot steer inside them, so their coverage grows only through the Rust code around them; the sanitizer still catches every crash and abort inside them and what they do wrong to memory Rust owns. Leaks inside them that Helikodu cannot free are suppressed in `fuzz/lsan.supp`.

## Handling a finding

1. Reproduce: run the target's binary (`target/<host>/release/<target>`) with the crash file as its argument.
2. Shrink it: the same with `-minimize_crash=1 -max_total_time=60`.
3. Fix the code that Helikodu owns. When the fault is in a library (FFmpeg, LAME, lofty), refuse or clean the input before it reaches the library, or catch the panic around the call, and report the fault upstream.
4. Add the shrunk input as a regression test next to the code that broke, or to the malformed-media corpus.
5. Run the target again until it runs its full time without a finding.

## The malformed-media corpus

`cargo test -p helikodu-meta --test malformed` makes about 430 damaged files (2.3 MB) at test time and reads each one the way the player does: the scan worker's reader and the FFmpeg bridge for media files, and the parsers for CUE sheets, lyrics and playlists. Each must be read or refused within a time limit, without a panic. The valid files are written by the bridge's own encoder and tagged by lofty, so the test needs no external tools and no media file is committed. The damage: cuts at several places, flipped bytes in the header and in the body, a zeroed stretch, noise after the header, length fields set to 0, to the largest values and past the end, and a header written twice; plus hand-made files whose sizes lie (ID3v2, MP4 boxes, Ogg pages, DSF), hostile CUE sheets, lyrics and playlists (paths out of the folder, entity expansion, deep nesting, huge counts), and every input fuzzing found.

## Campaign of 18 November 2026

FFmpeg 9.0.2, lofty 0.25.5, nightly Rust 1.102. Each target ran on a machine shared with other builds, at most eight at a time.

The last clean run of each target, after the fixes below (a target that found something ran again until it ran its full time without a finding):

| Target | Clean run | Before that |
|---|---|---|
| `ffmpeg_bridge_open`, `tags_read`, `dsd_open`, `ffmpeg_encode` | 15 min | one finding each (two more for `ffmpeg_encode`) |
| `naming_pattern`, `settings_toml`, `ui_state_toml`, `export_toml`, `sync_manifest`, `worker_message`, `playlist_sniff` | 15 min (`export_toml` and `naming_pattern` again after their fixes) | nothing |
| `cue_parse`, `playlist_m3u`, `playlist_pls`, `playlist_xspf`, `query_parse`, `cover_decode`, `autoeq_parse`, `openmpt_open` | 10 min | nothing |
| `lyrics_parse`, `sid_open`, `gme_open`, `midi_open` | 10 min | one finding each (two for `gme_open` and `midi_open`) |

Findings, each fixed with a regression test:

| Where | What | Fix |
|---|---|---|
| FFmpeg 9.0 ADX demuxer | A header the ADX decoder refuses made the demuxer divide by zero (`SIGFPE`) while the bridge probed the file | The bridge refuses such headers before FFmpeg sees them (`demux/header_guard.rs`) |
| LAME (MP3 encoder) | Samples that are NaN or far beyond full scale abort the process (an assertion) | The encoder cleans float samples: NaN becomes silence, the rest is clipped to ±16 |
| FFmpeg 9.0 ALAC encoder | A last frame of one or two samples with four to six channels aborts the process | Such a last frame is padded to 16 samples of silence |
| lofty 0.25 (Opus) | An arithmetic overflow in the Opus header (a panic) | Every call into lofty is guarded; a panic refuses the file and FFmpeg reads it instead (reported upstream as a bug to fix) |
| Own DSDIFF reader | The end of the `PROP` chunk overflowed with a huge size | Saturating arithmetic |
| Own LRC parser | Word timings past the end of a line cut to the line limit | Timings are clamped to the cut line |
| libsidplayfp | A PSID data offset past the end of the file made it read outside its buffer | Headers are checked before the library sees them |
| libgme (NSF) | A play period of a few microseconds made emulation take many seconds per second of music | Play periods shorter than 1 ms are lengthened to 1 ms |
| libgme (HES) | A damaged HES file made the emulator read outside its memory (`SEGV`) | HES files are no longer played |
| rustysynth | A MIDI file without tracks panicked while measuring its length | The call is guarded; the file is refused |
| rustysynth | Damaged MIDI files also make its file reader panic (capacity overflow) | Already guarded; the file is refused |
| libvorbisenc | A few KiB leak when the encoder refuses six-channel settings | Not ours; suppressed in `fuzz/lsan.supp` |

Found by review while writing the targets, and fixed with tests: the sync manifest could name paths outside the destination (absolute or with `..`), which a sync would have removed; naming patterns nested without limit could exhaust the stack, and a folder name cut to length could end in a dot or a space; preset bit rates and identifiers near the integer limits overflowed.

Known and left: seeking in a MIDI file renders from the start, so a seek far into a file that claims to be very long takes seconds (about 5 s to the middle of a 74-minute file); the long-tail targets seek at most a minute in.
