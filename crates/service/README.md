# helikodu-service

The player service (queue, play modes, session, statistics recording), the library facade the front ends query, the settings store, background jobs and the event bus. The GUI, the CLI and the D-Bus interfaces all go through this crate.

- **May depend on:** `model`, `engine`, `library`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

The Cargo features `pipewire`, `alsa` and `dsd` (all on by default) pass on to the engine's features of the same names; `bundled-sqlite` (off by default) passes on to the library's.

## Overview

| Module | What it holds |
|---|---|
| `service.rs` | `Service::start(ServiceConfig)`: settings, library, player and bus kept in step (playback settings reconfigure the player, the output device follows both ways) |
| `bus.rs`, `event.rs` | `EventBus`, `Subscription` (derefs to a `Receiver`), `Waker` for main loops; `Event` (`Player`, `Settings`, `Library`), `SubscribeOptions` (kinds of events, position throttling) |
| `clock.rs` | `Clock`, `SystemClock`, `FakeClock` for tests |
| `files.rs` | `AppDirs` (XDG config, state, data, cache), `write_atomic` |
| `formats.rs` | `supported_formats()`: the decoders this build has (from the engine's decoder registry) and the families of formats each reads, for the Supported formats page; `sqlite_version()` |
| `settings.rs`, `settings/migrate.rs` | `SettingsStore` (`settings.toml`), `migrate` and `MIGRATIONS`, `applies`, `UiStateStore` (`ui-state.toml`) |
| `player.rs`, `player/` | The player service |
| `library.rs`, `library/` | `MusicLibrary` (blocking, typed queries) and `LibraryService` (worker thread, thumbnails, scans, watcher) |
| `jobs.rs`, `jobs/` | Background jobs: the loudness scanner (`jobs::loudness`) |
| `convert.rs`, `convert/` | The converter and folder sync: presets, naming patterns (`convert::naming`, self-contained), the plan of a conversion, `ConvertJob` (several files at a time, written under a temporary name and renamed when complete, tags and cover copied), sync destinations with their manifest `.helikodu-sync.json`, and `export.toml` (`ExportStore`) |
| `cover_manager.rs` | The cover manager: albums by where their covers are, pictures found in album folders, and putting a picture into the files or beside them |

## Player

`Player::spawn(PlayerConfig)` starts the service actor, a thread that owns the engine, the queue and the session, and returns the handle with a subscription to its events. `Player::send` (or a cloned `CommandSender`) takes `PlayerCommand`s; `Player::queue()` and `Player::status()` read the latest state without waiting for the actor; `Player::sync()` waits until earlier commands are handled and the session is saved.

| Command | What it does |
|---|---|
| `PlayFiles`, `PlayTracks { context, tracks, start }` | Replaces the queue (or appends, with `PlayerPrefs::append_contexts`) and plays |
| `PlayContext { context: ContextRequest, title, start }` | An album, artist, genre or search, looked up in the library |
| `Enqueue { tracks, at: Next \| End }`, `PlayEntry`, `Remove`, `Move`, `Clear` | Edit the queue; `Clear` keeps the playing entry |
| `PlayPause`, `Play`, `Pause`, `Stop`, `Next`, `Previous`, `Seek*` | Transport; `Previous` restarts after `PlayerPrefs::previous_restart` |
| `SetVolume`, `AdjustVolume`, `SetOutput`, `NextOutput` | Volume and output, switched while playing |
| `SetShuffle`, `SetRepeat`, `SetStopAfter`, `Configure` | Play modes and preferences: shuffle by tracks, albums, folders, groupings or within albums; repeat one, the album or all; stop after the track, the album or the queue |
| `SetConsume`, `SetPlayScope`, `SetFollowCursor`, `SetCursor` | Consume, "Play only" (everything, the artist, the album), "Continue from the selected track" and the selected entry |
| `NextAlbum`, `PreviousAlbum`, `RandomAlbum` | Album navigation within the queue |
| `LoopStartHere`, `LoopEndHere`, `SetLoop` | The A-B loop of the playing track |
| `StartSleepTimer`, `ExtendSleepTimer`, `CancelSleepTimer` | The sleep timer: at once (a 30 s fade, then pause) or after the track |
| `SetTrackFlags` | Per-track flags (leave out when shuffling, play only when started directly, volume for this track), stored in the library |
| `ConfigureAudio { output, loudness, dsp }` | The other `[output]` settings, `[loudness]` and `[dsp]` while playing: a new rate policy, format, buffer or volume control reopens the stream at the same position, loudness ramps the gain, a DSP change crossfades in |
| `ConfigureTransitions(TransitionSettings)` | `[playback.transitions]` while playing: crossfades between tracks and on skips, the album policy, and the fades of pause, stop, seek and skip |

Events (`PlayerEvent`): `State { playback, transport }`, `TrackChanged` (now playing), `Position`, `QueueChanged { version }`, `ModesChanged`, `StopAfterChanged`, `VolumeChanged`, `OutputChanged`, `DevicesChanged`, `SignalPath` (the engine's signal path of what is heard, `None` when nothing plays; also in `PlayerStatus::signal_path`), `PlaybackStats` (buffer, underruns, delivered frames; four times a second while playing, also in `PlayerStatus::stats`), `Problem`, `Notice` (crash recovery, suspect skipped, context unavailable, session or listen not saved), `ListenRecorded`, `Ended`.

- **Device reservation.** `PlayerConfig::reservation` (and `ServiceConfig::reservation`) is the `SharedReservation` the engine's ALSA backend uses to take a sound card from the sound server for exclusive output; the application passes the desktop crate's implementation. The default takes nothing.
- **Queue.** Entries (`EntryId`, `TrackRef`, whether they came with the context) in play order, with the context order beside it. Shuffle stores its permutation, so "Previous" retraces what played; turning it off restores context order. With repeat-all and shuffle, each cycle plays every entry once in a new order. `QueueSnapshot::splice_from` gives the list-model change between two versions.
- **Play modes.** Each entry carries keys looked up in the library when it is queued (album, folder, grouping or work, first artist, per-track flags; loose files have their folder as album). Album, folder and grouping shuffles permute blocks of entries with one key, each in context order, and the current block plays to its end first; "within each album" keeps the albums in context order and permutes their tracks. Repeat-album wraps inside the current album in play order. Stop-after the album or the queue stops where the next entry would leave the album or wrap, and goes back to Off once it has stopped playback; the entry that would have followed becomes current. Consume removes an entry once playback moves on from it. "Play only", the flags, unplayable entries and suspect tracks are passed over by automatic playback, "Next" and "Previous", never removed. With "Continue from the selected track" the entry of `SetCursor` follows the current one, once. `ModesChanged`, `StopAfterChanged`, `LoopChanged`, `SleepTimerChanged`, `CursorChanged` and `TrackFlagsChanged` report changes; `PlayerStatus` holds the A-B loop, the sleep timer and the cursor. The modes and stop-after are saved in the session.
- **A-B loop.** `LoopStartHere` and `LoopEndHere` take the playing position (an end without a start loops from the beginning); the engine's `SetLoop` joins the end to the start without a gap. The loop ends when another item starts.
- **Sleep timer.** Runs on the player's clock (`FakeClock` in tests). "At once" lowers the engine's volume over the last 30 s (`SLEEP_FADE`) without changing the volume setting, then pauses and restores it; "After the track" stops playback when the track playing at the time ends (`EndReason::SleepTimer`).
- **Gapless.** The next entry is named at `NearEnd`; when the queue, modes or stop-after change afterwards, the engine gets the new one.
- **Unplayable entries** are reported and skipped in the direction playback moved, and passed over for the rest of that queue.
- **Statistics.** With a library, each listen to a library track (also a file that is in the library) is measured from the engine's positions (seeks excluded) and recorded with `PlayRules` on a thread of its own: finished, skipped (moved on) or stopped. Long tracks start where they stopped when `resume_long_tracks` is on.
- **Session** (`SessionConfig`, `$XDG_STATE_HOME/helikodu`): `session.json` (queue, context order, position, modes, volume, output, suspect tracks) saved by atomic replace after changes (debounced), every 15 s while playing and at the end; restored paused. `playing.json` is the crash sentinel: it names what loads or plays and is removed when nothing plays and at a normal end. Found at the start, it adds a strike to each track it names and the session is restored paused with a notice; two strikes in a row make a track suspect, which automatic playback passes over until it plays to its end.

## Settings

`SettingsStore` reads `$XDG_CONFIG_HOME/helikodu/settings.toml` into `helikodu_model::settings::Settings` (tables `library`, `playback`, `output`, `loudness`, `dsp`, `appearance`, `advanced`, and `version`). Older files are migrated step by step before they are read; a broken file gives the defaults and is kept as `settings.toml.bak` when first replaced; a value out of range resets its table; a file from a newer version is read but never written. `update` validates, saves atomically and publishes `SettingsEvent::Changed` with the changed groups; `applies(key)` says when a change takes effect: every setting is live except the resampler quality and the LFE option of downmixes (`dsp.resampler`, `dsp.channel_mix.include_lfe`), which apply from the next track. The service passes a changed device, backend or mode to the player as `SetOutput`, the rest of `[output]`, `[loudness]` and `[dsp]` as `ConfigureAudio`, and `[playback.transitions]` as `ConfigureTransitions` (and `PlayerConfig::transitions` at start). Items carry their album (title and album artists from the tags) as `PlaybackItem::album`, so that consecutive tracks of one album are never crossfaded while the settings keep albums gapless.

## Library

`MusicLibrary` opens the database (`open`, `open_existing`, `in_memory`, `from_library`) and offers browsing (`albums`, `album`, `artists`, `genres`, `tracks`, `all_tracks`, `sorted_tracks` by `TrackOrder` keys such as format, rating and plays), `search`, `track_info`, `file_info`, `track_source` (file and CUE span), `track_details` (path, tags and audio properties from the database), static playlists (`playlists`, `playlist`, `playlist_items`, `create_playlist`, `rename_playlist`, `delete_playlist`, `add_to_playlist`, `move_playlist_items`, `remove_from_playlist`, `import_playlist`, `export_playlist` in the format the file's extension names (M3U8, M3U, PLS, XSPF; M3U8 otherwise) and `export_playlist_as`), statistics (`stats`, `record_listen`, `history`, `history_totals`, and `export_history`, which writes the history to a local file as ListenBrainz JSON Lines or, for a `.log` name, an Audioscrobbler `.scrobbler.log`; `write_history` formats `ExportedListen`s), `rate` (whole stars), tag edits (`write_tags`: `BatchItem`s with the `[library]` and `[advanced]` settings and the worker program → `TagWriteReport`, the files read again at once), `overview` and `scan`. The query language: `query_tracks` (a page and the total), `query_track_ids` (every id; a seed fixes `sort:random`), `query_count`, and the saved searches (`saved_searches` with the number of tracks each finds now as `SavedSearchEntry`s, `save_search`, `rename_saved_search`, `set_saved_search_query`, `delete_saved_search`, `move_saved_search`); `parse_query_lenient` reads the search entry (a plain search of the words, and the `SyntaxError` with its byte range, when the text cannot be read), `parse_query` and `print_query` turn text and the tree of `helikodu_model::query` into each other. The module `library::tag_editing` re-exports what a tag editor needs from the library (`load_for_editing`, `EditableFile`, `SelectionView`, `TagChanges`, `TagField`, `BatchItem`, `WriteError`, …).

`LibraryService` runs the same queries on its own thread and hands each result to a callback; `query_tracks` and `saved_searches` run the query language there; `write_tags` writes tags with its worker program and the settings, blocking the caller (not its thread), and announces `LibraryEvent::Changed`; `describe` gives what the queue panel shows; `thumbnail` answers from the cache or schedules the artwork pool (`LibraryEvent::ThumbnailReady`); `scan`, `watch`, `rescan` announce progress as `LibraryEvent::Scan`, `Watch`, `ScanEnded` and `Changed`.

## Loudness scanner

Copies (byte-identical files): `MusicLibrary::copies_of` (a track → `CopyGroup`, the main copy first, with path, format, size, library folder and `same_file_as` for two paths to one file), `copy_groups`, `set_main_copy`, `trash_copies` and `trash_duplicates` (explicit `FileId`s → a `TrashOutcome` each; the desktop trash of `Trash::of_user`, never a deletion), `find_duplicates` (`MatchBy`, `KeepRule` → `DuplicateGroup`s with suggested marks), `duplicate_counts`, `duplicate_settings` and `apply_duplicate_settings`. `LibraryService` offers the same with callbacks and announces `LibraryEvent::Changed` after a change. The `[library.duplicates]` settings are applied when the service starts and whenever they change (a background thread; skipping copies removes their tracks, adding them reads the copies again), announced as `LibraryEvent::Scan`, `DuplicatesApplied` and `Changed`.

`jobs::loudness` measures library tracks with the engine's EBU R128 / ReplayGain 2.0 scanner (`helikodu_engine::analysis`). `LoudnessJob::start(library, LoudnessRequest, events)` runs it on a thread of its own and returns a handle to `cancel` and `join` (→ `LoudnessSummary`); `run` does the same on the calling thread. `LibraryService::measure_loudness` starts one and announces its `LoudnessEvent`s as `LibraryEvent::Loudness` (then `Changed`); `stop_loudness` cancels it.

- **What.** `LoudnessSelection::Tracks` (the selected tracks), `Albums`, or `Unmeasured` ("Measure the rest": `LoudnessRequest::unmeasured()`, the tracks without a current measurement and the other tracks of their albums).
- **How.** A pool of threads (`threads`, by default half the processors) at nice 10 decodes each track through the engine's decoder registry, CUE sheet tracks as their spans of the file. When every track of a whole album is done, the album is measured as one programme and its tracks' results are stored together; a track picked without the rest of its album gets track values only. Cancelling stops the threads between blocks; albums finished before stay stored, half-measured ones are dropped.
- **Events.** `Started { tracks }`, `Progress { done, total, track, ok }` per track, `Stored(results)` per album or lone track, `Finished(summary)`.
- **Where to.** Always the library (`MusicLibrary::measured_loudness`, `loudness_coverage`). With `write_tags` (the `loudness.write_measured_to_tags` setting, off by default) also the files' tags, as ReplayGain 2.0 values (`REPLAYGAIN_TRACK_GAIN` `-6.30 dB`, `REPLAYGAIN_TRACK_PEAK` with the true peak, and the album pair), through `Library::write_tags`; the files are read again and the measurements restamped. Tracks of CUE sheets are never written. Opus files get the same `REPLAYGAIN_*` comments, which take precedence over `R128_*_GAIN` when read; writing `R128_*` itself needs the tag writer of `meta` to accept those keys, which it does not yet.
- **Playback.** When the player queues a library track with a current measurement, the measured values win over the file's tags (track values; album values when measured with the album), with true peaks for clipping prevention.

## Tests

```sh
cargo test -p helikodu-service
```

`tests/library.rs` fills a database file through the library crate and checks the facade: search with paths, track details and statistics, a library file that cannot be read, totals and the most played tracks, a missing library and a scan without folders, sorted track lists, playlist export in each format, and tags written into a FLAC file generated with the `ffmpeg` tool (skipped without it), with a stale stamp refused. The level of ReplayGain playback is checked end to end by `apps/cli/tests/replaygain.rs`.

`tests/query.rs` runs the query language through the facade: queries with paging, quick filters, the search entry's lenient parsing with the error's byte range, saved searches with their counts, and both through the library service's thread.

`tests/loudness.rs` runs the loudness job on generated WAV files: an album of three tracks measured with its album gain (the album as one programme, not the mean of its tracks), stored in the library with the files untouched, with its events in order; a track picked alone without album values; a job cancelled at once, keeping no half album; the job through `LibraryService` and the event bus; and, with the `ffmpeg` tool (skipped without it), FLAC and Opus files whose tags are written only when asked, the measurement current after the write. A unit test in `player/loudness.rs` checks that playback takes the measured values over the tags.

`tests/player/` covers the player through its public API: the queue, controls, play modes (stop-after, repeat, shuffle cycles, play next, remove, move, choose), the order of events and position thinning, the session saved and restored, the crash sentinel over three simulated abnormal ends, and statistics recorded in a library in memory with a fake clock. The queue model's own unit tests include property tests (`proptest`): every entry once per shuffled cycle, "Previous" retracing the history, and edits keeping the queue consistent. `tests/settings.rs` checks the settings store on disk and `tests/service.rs` the whole service.

The player tests play generated WAV files: to the WAV sink as fast as it takes them (the joins between files bit-exact, unplayable and missing files skipped, an empty queue), and in real time to the null sink (next, previous, pause, seek, volume, a move from the null sink to a WAV file while playing).

A test against the real sound servers is ignored by default. It never makes a sound:

```sh
# Needs a running PipeWire session with pipewire-pulse (`pactl`) and the ALSA `null`
# PCM. Loads two temporary PipeWire null sinks, moves playback of digital silence
# from the first to the ALSA `null` PCM and on to the second, and unloads the sinks.
cargo test -p helikodu-service --test output_switch -- --ignored
```
