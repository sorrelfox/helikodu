# helikodu-cli

Builds `helikodu-cli`: remote control of the running application over D-Bus, and offline tools (play, list-outputs, scan, search, info, stats, loudness, analyze, similar, duplicates, managed-folder; later verify and convert) that use the core directly. It turns arguments into calls on the core and prints the results.

- **May depend on:** `model`, `service`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

The offline commands are in `src/offline/`; the commands for the running application are in `src/remote/`. They use its D-Bus interface (`docs/dev/dbus-api.md`) through zbus, with the interface's names written out here, since this program may not depend on `desktop`; `tests/remote.rs` checks them against the interface's introspection data.

## Usage

```sh
helikodu-cli play FILE... [--backend auto|pipewire|alsa|null|wav] [--device ID]
                          [--mode shared|exclusive] [--volume 0..100] [--wav-out PATH]
                          [--replaygain off|track|album|auto] [--preamp DB]
helikodu-cli list-outputs [--backend auto|pipewire|alsa|null] [--json]
helikodu-cli scan [FOLDER...] [--exclude PATTERN]... [--workers N] [--worker PATH]
                  [--json] [--database PATH]
helikodu-cli search TEXT... [--limit N] [--offset N] [--json] [--database PATH]
helikodu-cli info FILE|TRACK-ID [--json] [--database PATH]
helikodu-cli stats [--top N] [--json] [--database PATH]
helikodu-cli loudness [--album|--library] [--write-tags] [--threads N] [--worker PATH]
                      [--json] [--database PATH] [FILE|TRACK-ID...]
helikodu-cli analyze [--no-loudness] [--json] [--database PATH]
helikodu-cli similar FILE|TRACK-ID [--album|--artist] [--limit N] [--json] [--database PATH]
helikodu-cli duplicates [--json] [--database PATH]
helikodu-cli managed-folder show|set|off|import|consolidate|resume|roll-back
                            [--settings PATH] [--database PATH] ...
helikodu-cli status|queue|playlists [--json]
helikodu-cli play-pause|pause|resume|stop|next|previous
helikodu-cli seek POSITION            # 83, 1:23, +10, -5
helikodu-cli volume [LEVEL]           # 0..100, +5, -5
helikodu-cli shuffle off|tracks|albums|folders|groupings|within-album
helikodu-cli repeat off|one|album|all
helikodu-cli enqueue [--next] FILE...|--query QUERY
helikodu-cli play-query [--shuffle] QUERY...
helikodu-cli rate STARS|none [--track ID]
helikodu-cli export-playlist PLAYLIST FILE
helikodu-cli --version
```

### play

Plays the files in the order given. Files of the same format join without a gap. A file that cannot be opened or decoded is reported and skipped.

| Option | Meaning |
|---|---|
| `--backend` | `auto` (the default: PipeWire if it is running, otherwise ALSA), `pipewire`, `alsa`, `null` (discards the audio) or `wav` |
| `--device` | The device's stable name, as `list-outputs` shows it: a PipeWire node name such as `alsa_output.usb-Topping_D10s-00.analog-stereo`, or an ALSA PCM such as `hw:CARD=D10s,DEV=0`. The default is the backend's default device |
| `--mode` | `shared` (the default) or `exclusive` |
| `--volume` | The position of the volume control, from 0 to 100 (the default). The control follows the cubic curve of desktop mixers |
| `--wav-out` | The file the `wav` backend writes; required with `--backend wav` and refused otherwise. The file is written as fast as the files decode |
| `--replaygain` | Loudness normalization by the files' ReplayGain tags, or Opus `R128_TRACK_GAIN`/`R128_ALBUM_GAIN` tags (converted to the ReplayGain 2.0 reference of −18 LUFS): `off` (the default), `track`, `album` (for files given in album order), or `auto` (the album gain for a file whose neighbour in the list, before or after it, has the same album title and album artists; the track gain otherwise). A file without the chosen gain uses the other one; a file without either plays unchanged. A positive gain is lowered so that the peak in the tags stays at or below full scale |
| `--preamp` | Added to the gain of files with loudness tags, in dB, from −24 to 24 (the default 0) |

When standard input is a terminal, these keys control playback, and a status line on standard error shows the file, position, volume and output:

| Key | Action |
|---|---|
| Space | Pause or resume |
| `n`, `p` | Next file; previous file (restarts the current one when more than three seconds of it have played) |
| ←, → | Seek back or forward 5 seconds |
| `+`, `-` | Volume up or down by 5 |
| `o` | Move playback to the next output device, through the devices of PipeWire and then of ALSA, without stopping |
| `q`, Esc, Ctrl+C | Quit |

Otherwise the files play to the end without reading input, and a lost device ends playback instead of pausing it.

Messages (the files as they start, skipped files, errors) go to standard error; `play` writes nothing to standard output.

### list-outputs

Lists the devices of every backend that plays sound (PipeWire and ALSA), or of the one given with `--backend`, on standard output. The default device is marked with `*`. With `--json` the output is one JSON array with an object per device: `backend`, `id`, `description`, `is_default` and `capabilities` (`sample_rates`, `formats`, `channel_counts`; empty when the backend cannot tell). A backend that cannot be listed is reported on standard error.

```sh
helikodu-cli list-outputs --backend pipewire --json | jq -r '.[].id'
```

### The library commands

`scan`, `search`, `info` and `stats` work on the library database, `$XDG_DATA_HOME/helikodu/library.db` (`~/.local/share/helikodu/library.db` when `XDG_DATA_HOME` is not set), or the file given with `--database`. `scan` creates it; the other commands need it to exist, except `info` on a file. Sorting follows the locale of `LC_ALL`, `LC_COLLATE` or `LANG`.

Paths in JSON are given as `"path"`, text in which bytes that are not UTF-8 become U+FFFD, and, only when the path is not valid UTF-8, also as `"path_bytes"`, the exact bytes as an array of numbers. Times are seconds since the Unix epoch, lengths are milliseconds, and a value that is not known is `null`. Objects may gain keys in later versions; existing keys keep their meaning.

#### scan

Adds the folders to the library and reads their new and changed files with `helikodu-scan-worker` processes; files that disappeared are marked missing. Without folders, every folder already in the library is scanned again.

| Option | Meaning |
|---|---|
| `--exclude` | Leaves out every file and folder whose name matches the pattern (`*` and `?` are wildcards, case matters), or, when the pattern contains a `/`, the folder or file at that path (relative to the current folder unless absolute). Can be given more than once |
| `--workers` | The number of worker processes, 1 to 64. The default is one per processor, at most 8 |
| `--worker` | The worker program. The default is `helikodu-scan-worker` beside `helikodu-cli`, or in `../libexec/helikodu/` from it |

Progress goes to standard error (a line that updates itself on a terminal, otherwise the steps and every file that could not be read); the summary goes to standard output. With `--json` the summary is one object:

```json
{
  "roots": 1, "unavailable_roots": 0, "found": 120, "unchanged": 0, "read": 120,
  "added": 118, "returned": 0, "updated": 0, "moved": 0, "missing": 0, "back": 0,
  "failed": 2, "skipped_unreadable": 0, "worker_restarts": 1, "cancelled": false,
  "elapsed_ms": 4211,
  "failures": [
    {"path": "/music/broken.flac", "kind": "worker_crashed", "message": "…"}
  ]
}
```

`kind` is one of `io`, `unreadable`, `reader_failed`, `worker_crashed`, `timed_out`, `write_failed` and `other`.

#### search

Finds the tracks whose title, artists, album, album artists, genres, composers or path contain every word of `TEXT` (case, accents and character width do not matter), best matches first. `--limit` (1 to 1000, default 50) and `--offset` (default 0) page through them. With `--json`:

```json
{
  "query": "morning", "total": 1, "offset": 0, "limit": 50,
  "hits": [
    {"track_id": 7, "rank": "prefix", "title": "Morning Light", "artist": "Aurora",
     "album": "Dawn", "duration_ms": 241000, "path": "/music/Aurora/Dawn/01.flac"}
  ]
}
```

`rank` is how the whole query matched the title, an artist or the album: `exact`, `prefix`, `word_prefix`, `substring`, or `terms` (only word by word). `artist` lists the track artists separated by commas.

#### info

Shows a file or a library track. A file is read directly, so it need not be in the library; when it is, its library entry and statistics are shown too. A number that is not the name of an existing file is a track number, as `search` prints it; the track is shown from the library, with the loudness tags of its file if the file can be read. With `--json`:

```json
{
  "path": "/music/Aurora/Dawn/01.flac",
  "sub_index": 0,
  "origin": "file",
  "tags": {"title": "Morning Light", "title_sort": null, "artists": ["Aurora"],
           "album_artists": ["Aurora"], "composers": [], "album": "Dawn",
           "genres": ["Ambient"], "track_number": 1, "disc_number": null,
           "date": "2024", "year": 2024},
  "audio": {"duration_ms": 241000, "sample_rate": 44100, "channels": 2,
            "bit_depth": 16, "bitrate": 912000, "codec": "flac", "container": "flac"},
  "loudness": {"track_gain_db": -4.5, "track_peak": 0.25,
               "album_gain_db": null, "album_peak": null},
  "file_error": null,
  "library": {"track_id": 7, "identity_id": 7, "kind": "music",
              "added_at": 1795442700, "missing_since": null,
              "statistics": {"play_count": 2, "skip_count": 0, "first_played": 1795443000,
                             "last_played": 1795450000, "last_skipped": null,
                             "rating": null, "resume_position_ms": null}}
}
```

`origin` is `file` when the tags and audio properties were read from the file now, `library` when they are what the library stored. `sub_index` is the CUE track or subsong number, 0 for a whole file. `loudness` holds gains in dB relative to −18 LUFS (Opus R128 gains converted) and linear peaks; it is `null` when the file could not be read, and `file_error` then says why. `library` is `null` for a file that is not in the library. `kind` is `music`, `audiobook`, `podcast`, `audio_from_video` or `other`; `rating` is in half stars, 0 to 10.

#### stats

Shows the library folders, the totals and the most played tracks (`--top`, 0 to 1000, default 10; one track for each identity). With `--json`:

```json
{
  "roots": [{"path": "/music"}],
  "totals": {"roots": 1, "files": 118, "tracks": 130, "albums": 12, "artists": 9,
             "genres": 5, "duration_ms": 31800000, "size_bytes": 2400000000,
             "missing_files": 0, "unreadable_files": 2, "plays": 41,
             "played_tracks": 17, "rated_tracks": 3},
  "most_played": [
    {"track_id": 7, "title": "Morning Light", "artist": "Aurora", "album": "Dawn",
     "play_count": 6, "last_played": 1795450000}
  ]
}
```

`files` counts media files with tracks (not CUE sheets); `artists` counts track and album artists.

#### duplicates

Lists the groups of copies in the library: byte-identical files, the main copy first. It only lists; nothing is moved or deleted (the application moves copies to the trash). Text output marks each path `main`, `copy` or `same file` (another path to the same file on the disk, never a copy). With `--json`:

```json
{"groups": [
  {"main_file_id": 12,
   "files": [
     {"file_id": 12, "path": "/music/Dawn/01.flac", "main": true, "missing": false,
      "same_file_as": null, "format": "flac", "size_bytes": 24000000,
      "added": 1795450000, "root": "/music", "track_id": 40},
     {"file_id": 913, "path": "/backup/Dawn/01.flac", "main": false, "missing": false,
      "same_file_as": null, "format": "flac", "size_bytes": 24000000,
      "added": 1795460000, "root": "/backup", "track_id": null}
   ]}
]}
```

`track_id` is `null` for a copy without tracks of its own (with `on_scan = "skip"` in `[library.duplicates]`); `path_bytes` is added for a path that is not UTF-8, as in the other commands.

#### loudness

Measures library tracks, given as files or track numbers, with the EBU R128 / ReplayGain 2.0 scanner and stores the results in the library; playback prefers them over the tags. `--album` measures the whole albums of the tracks with the album gain (the album as one programme); a track given alone gets track values only. `--library` measures every track without a current measurement, with the rest of its album. `--write-tags` also writes `REPLAYGAIN_*` tags into the files (not for CUE sheet tracks); `--threads` sets the number of measuring threads (default half the processors). With `--json`:

```json
{
  "total": 2, "measured": 2, "failed": 0, "tags_written": 0, "cancelled": false,
  "tracks": [
    {"track_id": 7, "path": "/music/Dawn/01.flac", "album_id": 3,
     "loudness": {"integrated_lufs": -11.7, "loudness_range_lu": 5.2,
                  "sample_peak": 0.97, "true_peak": 1.02, "track_gain_db": -6.3,
                  "version": 1,
                  "album": {"integrated_lufs": -12.1, "loudness_range_lu": 6.0,
                            "sample_peak": 0.99, "true_peak": 1.04, "gain_db": -5.9}},
     "error": null, "tags": "not_asked", "tags_reason": null}
  ]
}
```

`integrated_lufs` and the gains are `null` for silence; peaks are linear. `album` is `null` when the track was measured without its whole album. `tags` is `not_asked`, `written`, `skipped` or `failed`, with `tags_reason`. A track that cannot be measured has `loudness` `null` and `error`.

#### analyze

Analyses every library track that has no current analysis, one at a time, as the application does in the background: where its sound starts and ends, its silent stretches and its loudness envelope (see `docs/dev/track-analysis.md`). The same pass measures the loudness of tracks without any loudness values (no tags, no measurement), an album as one programme; `--no-loudness` leaves that out. Each track's result is stored at once, so an interrupted run loses at most one track; a file that cannot be decoded is recorded and not tried again until it changes. Progress goes to standard error. With `--json`:

```json
{
  "analyzed": 120, "failed": 1, "loudness_measured": 118, "cancelled": false,
  "library": {"tracks": 5000, "analyzed": 4999, "failed": 1, "remaining": 0}
}
```

The exit code is 1 when a track could not be analysed.

#### similar

Lists the library tracks that sound most like a track (a library file or a track number), from the sound features of the track analysis: most alike first, with the track number, the similarity from 0 to 1, artist, title (or file name), album and length. Copies of the same recording are left out. `--album` compares with the mean of the track's album and leaves out the album's tracks; `--artist` does the same for its first artist. `--limit` (default 20) sets how many. With `--json`:

```json
{
  "seed": {"track_id": 12, "album_id": 3},
  "tracks": [{"track_id": 40, "similarity": 0.83, "title": "…", "artist": "…", "album": "…", "path": "/music/…"}]
}
```

The exit code is 1 when nothing was found: the track is not analysed yet (run `analyze`) or is silent.

#### managed-folder

The managed media folder (`[library.managed]` in the settings file; see the user guide's file tools page). `show [--json]` prints the setting and a run that was cut short; `set [FOLDER] [--pattern P] [--mode copy|move] [--characters portable|as-is|ascii]` turns it on or changes it (a folder outside every library folder is added to `[library] folders`); `off` turns it off. `import PATH... [--move|--copy] [--dry-run] [--json]` and `consolidate [--move|--copy] [--dry-run] [--json]` plan first and, without `--dry-run`, carry the plan out; the files are read by `helikodu-scan-worker` and the library reads only the folders the files went to. `resume` and `roll-back` settle a run that was cut short (no new run starts before). With `--dry-run --json` the plan is:

```json
{
  "operation": "import", "move": false, "folder": {"path": "/home/me/Music/Library"},
  "items": [
    {"kind": "audio", "action": "copy", "from": {"path": "/media/stick/01.flac"},
     "to": {"path": "/home/me/Music/Library/Okto/Live/01 One.flac"},
     "size_bytes": 23456789, "renamed": false, "rewritten": false, "file_id": null}
  ],
  "to_place": 1, "renamed": 0, "skipped": 0,
  "total_bytes": 23456789, "bytes_needed": 23456789, "free_bytes": 1000000000
}
```

`action` is `copy`, `move`, `copy (source is read-only)`, `skip (identical)` or `skip` (with `reason`). The exit code is 1 when a file could not be placed or a run was not finished.

### The commands for the running application

`status`, `queue`, `playlists`, the transport, `seek`, `volume`, `shuffle`, `repeat`, `enqueue`, `play-query`, `rate` and `export-playlist` call the interface `io.github.sorrelfox.Helikodu.Control1` of the running application on the session bus. They first ask the bus whether the application owns its name and never start it: when it does not, or there is no session bus, they print `Helikodu is not running` and exit with 4. An error the application returns (no such playlist, no library track playing) is printed and exits with 1. `--json` prints the application's JSON as it comes; the text form is for people.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. For `play`, at least one file played; skipped files are reported but do not change the code. For `scan`, files that could not be read are reported but do not change the code. `search` with no match is a success |
| 1 | A runtime error: the output could not be opened or failed, no backend could be listed, there is no library yet, the library or a file could not be read, a scan could not start or a folder given to it could not be read, or `info` found no such track; for `loudness`, also a track that could not be measured or whose tags could not be written |
| 2 | A usage error: an unknown option or value, or options that do not go together |
| 3 | `play` could not play any of the files |
| 4 | A command for the running application found it not running, or no session bus |

## Tests

```sh
cargo test -p helikodu-cli
```

- `tests/cli.rs` runs the built program against generated WAV files with the `null` and `wav` backends; they make no sound.
- `tests/replaygain.rs` plays FLAC files with ReplayGain tags and an Opus file with R128 tags, made by the `ffmpeg` tool, to the `wav` backend and checks the level of the output in every mode: the track and album gains, the preamp, the automatic choice by album neighbours, clipping prevention by the peak, and untagged files.
- `tests/loudness.rs` scans two FLAC files made by `ffmpeg` as one album and parses the JSON of `loudness`: one track measured without album values and the file untouched, the rest of the library with the album gain, nothing left after that, the album of a track number with `--write-tags` (the tags read back by `info`), and the usage errors.
- `tests/library.rs` makes a small library with `ffmpeg` (including a file name that is not UTF-8 and an excluded folder), scans it with real worker processes into a temporary database, and parses the JSON of `scan`, `search`, `info`, `stats` and `duplicates`, and checks the errors and exit codes. It builds `helikodu-scan-worker` into the same target folder, beside `helikodu-cli`. It gives one track play counts with the `sqlite3` tool when that is installed.

- `tests/remote.rs` starts a private `dbus-daemon`, serves a stand-in for the application's interface on it and runs every command for the running application against it, checking the calls, the text and JSON output and the exit codes (4 without the application).

Tests whose tool or encoder is missing print `SKIPPED:` and pass.
