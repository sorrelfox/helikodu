-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The track analysis, made in the background after scans: where a track's sound
-- starts and ends, its silent stretches and its loudness envelope. Nothing of it
-- is written into the files.
--
-- - `version` is the version of the analysis; rows of an older one count as not
--   analysed. `analyzed_at` is when it was made.
-- - `file_size`, `file_mtime_ns`, `start_frame` and `end_frame` are the file's
--   stamp and the track's span when it was analysed. While they still match the
--   file and track rows, the row is current; after the file or its CUE sheet
--   changed, the track counts as not analysed.
-- - `failure` is why the track could not be analysed, NULL when it was. A failed
--   track is not tried again until its file changes or the version does; the
--   other columns are then NULL.
-- - Positions are frames from the start of the track at `sample_rate`:
--   `frames` is its length; `audio_start` and `audio_end` (exclusive) the
--   frames with audible sound, both NULL for a silent track; `fade_in_end` and
--   `fade_out_start` where the beginning reaches and the ending leaves the
--   track's body level.
-- - `integrated` is the integrated loudness in LUFS, `head_loudness` and
--   `tail_loudness` that of the first and last five seconds of sound; NULL for
--   silence.
-- - `silences` holds the silent stretches inside the sound, 16 bytes each: the
--   first and the end frame as little-endian unsigned 64-bit integers; NULL
--   when there are none.
-- - `envelope` is the loudness envelope: a format byte (1), the step in seconds,
--   then one byte per step, 0 for no sound and otherwise `LUFS = value / 2 - 120`.
CREATE TABLE track_analysis (
    track_id       INTEGER PRIMARY KEY REFERENCES tracks ON DELETE CASCADE,
    version        INTEGER NOT NULL,
    analyzed_at    INTEGER NOT NULL,
    file_size      INTEGER,
    file_mtime_ns  INTEGER,
    start_frame    INTEGER,
    end_frame      INTEGER,
    failure        TEXT,
    sample_rate    INTEGER,
    frames         INTEGER,
    audio_start    INTEGER,
    audio_end      INTEGER,
    fade_in_end    INTEGER,
    fade_out_start INTEGER,
    integrated     REAL,
    head_loudness  REAL,
    tail_loudness  REAL,
    silences       BLOB,
    envelope       BLOB
) STRICT;
