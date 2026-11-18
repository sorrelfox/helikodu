-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Loudness measured by the loudness scanner (EBU R128 / ITU-R BS.1770-4), kept apart
-- from the values the tags carry (`tracks.rg_*`, filled by the library scan).
--
-- - `version` is the version of the measurement; results of an older one count as
--   unmeasured. `measured_at` is when it was taken.
-- - `file_size`, `file_mtime_ns`, `start_frame` and `end_frame` are the file's
--   stamp and the track's span when it was measured. While they still match the
--   file and track rows, the measurement is current; after the file or its CUE
--   sheet changed, it counts as unmeasured and playback no longer uses it.
-- - `integrated` is the integrated loudness in LUFS, NULL for silence;
--   `loudness_range` in LU; `sample_peak` and `true_peak` linear (1.0 full scale).
-- - The `album_*` columns are the album the track was measured with, measured as one
--   programme; NULL when the track was measured on its own.
CREATE TABLE loudness (
    track_id             INTEGER PRIMARY KEY REFERENCES tracks ON DELETE CASCADE,
    version              INTEGER NOT NULL,
    measured_at          INTEGER NOT NULL,
    file_size            INTEGER,
    file_mtime_ns        INTEGER,
    start_frame          INTEGER,
    end_frame            INTEGER,
    integrated           REAL,
    loudness_range       REAL NOT NULL,
    sample_peak          REAL NOT NULL,
    true_peak            REAL NOT NULL,
    album_measured       INTEGER NOT NULL DEFAULT 0 CHECK (album_measured IN (0, 1)),
    album_integrated     REAL,
    album_loudness_range REAL,
    album_sample_peak    REAL,
    album_true_peak      REAL
) STRICT;
