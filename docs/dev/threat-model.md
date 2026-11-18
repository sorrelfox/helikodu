# Threat model

Helikodu has no network access. Whatever can hurt a user therefore arrives as a file or over the D-Bus session bus. This document lists those entry points, what could go wrong at each, and how the project defends against it. It is reviewed at every milestone and whenever a new entry point is added.

## What is protected

- The user's music files and their tags.
- The library database, with its ratings, statistics and playlists.
- Every other file the process can reach.
- The user's hearing and equipment.

## Entry points

| Entry point | What could go wrong | Mitigation | Verified by |
|---|---|---|---|
| Media files while scanning | Memory corruption, a hang or memory exhaustion in a C demuxer or decoder | Metadata is read in separate scan-worker processes with time and address-space limits. Landlock and seccomp confinement of the workers is planned after 1.0 | The broken-file corpus; fuzzing; robustness tests |
| Media files while playing | The same, inside the main process | The FFmpeg bridge and other decoders are fuzzed; formats are probed by content, never trusted by extension; a file that fails is skipped; a crash sentinel stops a crashing track from being replayed at start-up; the Flatpak sandbox | Fuzzing; a forced-crash test |
| Tags and embedded pictures | Oversized or malformed data | Size and dimension limits on tags and pictures; tag parsing and picture decoding in memory-safe Rust | The broken-file corpus; fuzzing |
| Playlists, CUE sheets, lyrics | Paths that leave the library, references to URLs, parser bugs | No network code exists, so URL entries are never fetched; Helikodu's own parsers are fuzzed; file operations are confined to the library folders | The offline audit; fuzzing |
| Sync destinations (a phone, a memory stick) | The list of files Helikodu put there (`.helikodu-sync.json`) lies on the destination, where anyone may change it, and a sync removes the files it lists | Entries that are not plain relative paths (absolute, with `..`, empty) are dropped when the list is read; a file is never removed through a folder that is a symbolic link; Helikodu removes only files the list names | Fuzzing; unit tests |
| Settings and state files | A hand-edited or damaged `settings.toml`, `ui-state.toml` or `export.toml` | Values out of range reset their group or are moved into range; a file that cannot be read gives the defaults; naming patterns nest at most 16 brackets deep | Fuzzing |
| File names | Bytes that are not text; control characters shown in the interface or passed to other programs | Paths are handled as bytes; display text is escaped and never executed | The file-name corpus |
| Imported presets, SoundFonts, ROM files | Parser bugs in the equalizer preset importer, the MIDI synthesizer or the SID emulator | Helikodu's own fuzzed preset parser; these files are only ever chosen by the user | Fuzzing |
| D-Bus (MPRIS and the application API) and the command-line tool | Another program in the session starts playback, changes the queue or writes files | The session bus is trusted as the user's own. The API offers no operation that moves, renames or deletes files; export writes only to a path the caller names | API review |
| Audio output | A sudden jump to full-scale output | Playback to a fixed-volume device (where software volume is bypassed) needs the user's explicit confirmation; anti-click gain ramps; the limiter when signal processing is on | Integration test |

## How "offline" is enforced

Offline is a property of how Helikodu is built, not a setting:

- `cargo deny` rejects every network, TLS and DNS crate (see `deny.toml`).
- FFmpeg only ever opens local files: custom I/O, a protocol whitelist, a format whitelist, and an `io_open` callback that refuses every nested open.
- The Flatpak manifest grants no network permission.
- CI runs the test suite inside a network namespace with no interfaces, under a seccomp filter that turns any attempt to create an `AF_INET` or `AF_INET6` socket into a test failure.

## Not defended against

- Another process running as the same user outside a sandbox.
- Physical access to the machine.

## Residual risk

For low latency, playback decoding runs inside the main process, and native packages have no sandbox around it. Moving playback decoding into a sandboxed helper process is a research item for after 1.0.

## Reporting

Security problems are reported privately; see [SECURITY.md](../../SECURITY.md). How severe a bug is, and how fast it is handled, is set out in [bug-severity.md](bug-severity.md).
