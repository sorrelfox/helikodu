# Track analysis

Helikodu analyses every library track once, in the background, and keeps the result in the library database. Nothing is written into the files. Playback uses it for transitions that pick their overlap from the music, for trimming and skipping silence, and for a leveller for tracks without loudness values (see [How playback uses it](#how-playback-uses-it)).

## What is analysed

One decoding pass per track (a CUE sheet track or chapter is its own span of the file) gives a `TrackAnalysis` (`helikodu_model::analysis`). Positions are frames from the start of the track at its sample rate.

| Field | Meaning |
|---|---|
| `frames`, `rate` | The track's length in frames, and the rate they count in |
| `audio` | The frames from the first to the last audible sound; `None` for a silent track. Before it is the leading silence, after it the trailing silence |
| `silences` | Silent stretches inside `audio` of at least 2 s, the first 64 |
| `integrated_lufs` | EBU R128 integrated loudness of the track |
| `head_lufs`, `tail_lufs` | The loudness of the first and last 5 s of sound |
| `fade_in_end`, `fade_out_start` | Where the beginning reaches the track's body level, and where the ending leaves it for good |
| `envelope` | The loudness of each second (`LoudnessEnvelope`) |

**Silence.** A 10 ms window is silent when its RMS level (the mean square over all channels) is below −60 dBFS. The threshold is absolute: silence belongs to the signal (digital silence, dither, the noise floor of the recording), and a threshold relative to the track's loudness would call the soft passages of a quiet track silent. −60 dBFS lies above 16-bit dither (about −90 dBFS) and the hiss of tape and vinyl (−70 to −60 dBFS), below anything meant to be heard, and is the default of the common `silencedetect` tools. Shorter pauses than 2 s inside a track are part of the music and are not recorded.

**Loudness.** The audio is K-weighted as BS.1770 defines it, in 100 ms blocks. The envelope is the mean of each second. While decoding, the blocks of the first and the last 30 s of sound are kept at full resolution; from them come the head and tail loudness and the fade points. A fade point is the middle of the first (for a fade-in) or last (for a fade-out) 1 s window whose loudness is at least the integrated loudness less 3 LU. Without a fade, the points lie half a second inside the sound. When the ending stays below that level for its whole last 30 s (a quiet coda), the fade-out starts at the beginning of that span.

**Loudness measurement in the same pass.** The pass can also take the EBU R128 measurement of the loudness scanner (integrated loudness, loudness range, sample and true peak), so the track is not decoded a second time. The background job does this only for tracks that have no loudness values at all: no ReplayGain or R128 tags and no current measurement. A lone track gets track values. An album none of whose tracks has values is analysed as a whole and also gets album values, the album measured as one programme (up to 200 tracks; a larger album is left to the loudness scanner). Tracks that have values keep them, so the gains of tagged files never change behind the owner's back. If the job stops in the middle of such an album, the album is decoded whole again next time, for its album values.

The analysis has a version, `ANALYSIS_VERSION` in `helikodu_engine::analysis`. A change that changes the results bumps it, and the job then analyses every track again.

## How it is stored

Migration `0018_track_analysis.sql` adds the table `track_analysis`, one row per track (`ON DELETE CASCADE`). Like a loudness measurement, a row records the version, the file's stamp (size and modification time) and the track's span (`start_frame`, `end_frame`). The row is current while they match the file and track rows. A changed file or CUE sheet, or an older version, makes the track pending again. A track that cannot be decoded gets a row with `failure` set and the other values `NULL`, so it is not tried again until its file changes. When the duplicate handling moves a track to another copy, its row moves with it.

Two columns hold compact byte forms:

- `envelope`: byte 0 is the format (1), byte 1 the step in seconds, then one byte per step: 0 for no sound, otherwise `LUFS = value / 2 − 120` (0.5 LU steps from −119.5 to +7.5 LUFS). A track longer than an hour gets a step of 2, 4 and so on, so that no envelope has more than 3,600 values.
- `silences`: 16 bytes per stretch, the first and the end frame as little-endian `u64`; `NULL` when there are none.

A four-minute track takes about 330 bytes: 242 for the envelope and about 90 for the other columns and the row. That is about 33 MB for 100,000 tracks and 165 MB for 500,000, on disk only. The table is read only when a track's analysis is asked for, and browsing and searching never touch it.

The library API (`helikodu_library`): `store_analysis` (an `AnalysisOutcome`, analysed or failed), `track_analysis` (`StoredAnalysis` with `current`), `tracks_to_analyze` (pending tracks after a track id, a page at a time), `analysis_unit` (the tracks to analyse together under an `AnalysisPlan`) and `analysis_coverage` (`AnalysisCoverage`: tracks, analysed, failed).

## The job

`helikodu_service::jobs::analysis::run` works through the pending tracks one at a time, in pages of 64 track ids, and stores each track's result as soon as it is made. A stopped or killed job loses at most the track it was working on. Memory stays flat: the decoder's block, a few kilobytes of loudness state, 30 s of 100 ms blocks at each end, and the envelope of the track being analysed (4 bytes per second). An album measured for its loudness also keeps the small meter state of each of its tracks until the album is done. An `AnalysisGate` lets the caller pause before a track and slow the job down between decoded blocks.

The library service runs it in the background (`helikodu_service::library::BackgroundAnalysis`), when the `[library] analyze_in_background` setting is on (the default):

- **When.** 30 s after the start, and 5 s after every scan, rescan, application of the duplicate settings or other change of the library. Each run starts from the first pending track.
- **Priority.** One thread at nice 19, the lowest CPU priority. Linux derives the I/O priority of a thread that has none of its own from its nice value, so its reads also get the lowest best-effort I/O priority. The idle I/O class would need the `ioprio_set` system call, which no safe binding in the dependency tree offers.
- **Playback.** The player's events tell the job when playback runs (playing or buffering) and from which file. When the track being analysed is on the same device (`st_dev`) as that file, the job yields: it works in bursts of 50 ms and sleeps three times as long after each, 25 % of the time at most, so playback's reads and decoder threads are rarely kept waiting. Files on another device are analysed at full (low-priority) speed. Pausing completely while playing would mean that a library played from all day on one disk is never analysed.
- **Pauses.** While a scan or rescan runs, and while the computer runs on battery (a battery in `/sys/class/power_supply` reports `Discharging`).
- **Stopping.** Turning the setting off, or closing the program, stops the job within one decoded block. It resumes with the tracks still pending: at once when the setting is turned on again (after the start delay, if that has not passed), otherwise at the next start.
- **Progress.** `LibraryService::analysis_progress` returns an `AnalysisProgress`: total, analysed and failed tracks, and the state (`Off`, `Idle`, `Running`, `Yielding`, `Paused(Scanning | OnBattery)`). The same is announced as `LibraryEvent::Analysis` when the state changes and at most every 2 s while the job works. Settings › Library shows it under Track analysis (the front end's `Services::analysis_progress` and `Update::Analysis`), and nowhere else.
- **Showing it.** The tag editor's File info tab reads one track's stored analysis (`MusicLibrary::track_analysis`): a current analysis gives the loudness curve (one bar for every 2 s of the envelope, silent stretches flat, the fade-out marked) and the facts under it; a current failure says the file could not be read; anything else is "not analysed yet". A fade-out of less than 1 s is shown as none, since the analysis puts the fade point half a second inside the sound when there is no fade.

`helikodu-cli analyze [--no-loudness] [--json] [--database PATH]` runs the same job in the foreground on a library database and prints its progress.

## How playback uses it

**Getting it to the engine.** The player builds a `PlaybackItem` for each queue entry on its own thread (`helikodu_service::player`, `item_for`). As it does for measured loudness (`measured_loudness`), it reads the track's stored analysis there (`track_analysis`) and puts it on the item, `PlaybackItem::analysis` (an `Arc<TrackAnalysis>`, not serialized), when the row is current and analysed. The engine thread only reads what comes with the item; it never touches the database. Files outside the library, failed and outdated analyses give no analysis, and everything below falls back to what the engine did before.

**Settings.** Four keys in `[playback.transitions]` (`TransitionSettings`): `crossfade_from_analysis` (on), `trim_silence` (off), `skip_silence` (off) and `skip_silence_min_s` (10, 5 to 600). One in `[loudness]`: `leveller` (off). They reach the engine as the other transition and loudness settings do (`SetTransitions`, `UpdateSettings`) and apply from then on.

**Bit-perfect.** Nothing below happens on a bit-perfect path in exclusive mode, the rule the crossfade already followed (`Worker::bit_perfect_exclusive`: the stream is exclusive and the current item's plan is bit-perfect). For the leveller, `plan_for` in the transport makes the plan once with it and once without; in exclusive mode, when the plan without it is bit-perfect, that one is used.

**Gapless albums.** Consecutive tracks of one album while `keep_albums_gapless` is on (`Worker::gapless_join`, the test the crossfade already made) are neither crossfaded nor trimmed: neither the end of the first nor the start of the second changes. The start of an album run (a `Load`, or the join from another album) and its end may be trimmed.

**Silence at joins** (`transport/silence.rs`). When `SetNext` answers, `plan_joins` decides for the pair: with `trim_silence`, or with an analysed crossfade and both items analysed, the current item gets an end 10 ms after its sound (`Source::set_end`; the decoded block is cut there and the source ends) and the next one is moved to 10 ms before its sound, if it stands at its start. Neither for a gapless join; the last item of the queue is trimmed at its end. Until the answer comes, a freewheeling sink waits at the end of the sound (`trim_wait`), and in real time `NearEnd` comes earlier by the trailing silence (`tail_lead`), so the answer is normally there before the decoder reaches it; an answer that comes too late leaves the silence in. A `Load` from position zero with `trim_silence` starts after the leading silence once the item's plan is known (`trim_leading`); a seek goes where it is asked.

**Overlap-aware crossfades** (`transport/crossfade.rs`, `plan_crossfade`). With the crossfade between tracks on, `crossfade_from_analysis` on, both items analysed and the current item's end set by `plan_joins`, the window is `audio.end − fade_out_start` of the outgoing item plus the 10 ms margin, at most the configured length (and as before at most half of either item and what is left). It ends where the outgoing item now ends, so it starts at `fade_out_start`, and the incoming item starts with its sound. Without a fade the analysis puts `fade_out_start` half a second inside the sound, so the window is about half a second. Otherwise the fixed length applies as before.

**Skipping silence inside a track.** With `skip_silence`, each silent stretch of at least `skip_silence_min_s` that begins 15 s or more into the track becomes a jump from 1 s into it to 1 s before its end (`skips_for`, set on the source when it is attached). The source cuts the block where a jump begins and, once the chain has taken it, seeks the decoder to the jump's end without resetting the chain, as for the A-B loop. Jumps are reported in `Source::wraps`, and the worker records them in the timeline segment as rewinds (`note_wraps`), so positions after the jump are file positions. `remaining_frames` subtracts the jumps ahead and stops at the early end, so crossfade windows stay exact. A seek into a stretch plays from there (the jump starts must lie at or after the decoder's block); a seek before it still jumps. Not with an A-B loop on the item.

**Time and position.** File time throughout: `Position`, the seek target, and the length in `TrackStarted` are those of the file. A trimmed start reports the frame it starts at (the first `Position` after `Load` is where the sound starts), a jump moves the position forward like a seek (MPRIS sees the jump and sends `Seeked`), and a trimmed end simply ends the track before its length. The shown time therefore never disagrees with what a seek bar or another player would show for the same file.

**The leveller** (`pipeline/leveller.rs`, `Leveller`). A per-source stage after the loudness gain, before resampling. `resolve_track_gain` gives a track without loudness values `GainSource::Leveller` (with the track's own volume as its static gain) instead of the untagged preamp when `[loudness] leveller` is on and a mode is on; the plan then carries a `LevellerPlan` (target −18 LUFS plus the preamp of files with values; peak-safe when clipping is prevented by lowering the gain). Constants:

| Constant | Value |
|---|---|
| Step (gain decision interval) | 100 ms |
| Live short-term window (no analysis) | 3 s, at least 1 s before the first step |
| Envelope look-behind / look-ahead (analysis) | 1 s / 4 s |
| Gate (quieter seconds or steps do not count; nothing louder holds the gain) | −50 LUFS |
| Gain bound | ±12 dB |
| Rise / fall rate | 1 dB/s / 3 dB/s |
| Dead band | 0.5 dB |
| Ceiling for the loudest sample seen (peak-safe) | 0.989 (about −0.1 dBFS) |
| Release when switched off | 50 ms back to 0 dB |

Live loudness is K-weighted (BS.1770 filters at the source rate) and gated like the envelope. With an analysis, the first step goes straight to the gain of the start of the track. Between steps the gain moves in a straight line frame by frame; a block whose peak would pass the ceiling lowers it at once. The stage allocates when built and never while processing. Switching it on during a track starts it at 0 dB; switching it off ramps back to 0 dB. The signal path shows it as `leveller (no loudness values)`.
