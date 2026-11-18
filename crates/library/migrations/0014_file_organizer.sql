-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The file organizer: its saved naming patterns and the journal of the files it
-- moved, so that a run can be undone and done again, even after a restart or a
-- crash in the middle of a run.
--
-- `file_runs` holds one row per run (organizing, renaming one file, restoring
-- files): `state` is 0 while it runs, 1 when done, 2 when undone.
--
-- `file_moves` holds the moves of a run in the order they were made. Each row is
-- written before its file moves (`state` 0, planned), and set to 1 (moved) in the
-- same transaction that moves the file's row in the library; 2 when the file could
-- not be moved (`error` says why) and 3 when the move was undone. `file_id` is the
-- library file, NULL for a cover or booklet that moved with it. A run found in
-- state 0 at start-up is finished from what is on the disk.
--
-- `file_run_dirs` holds the folders a run created, which undoing it removes again
-- when they are empty.
--
-- `naming_patterns` holds the organizer's saved patterns in the order of its list.

CREATE TABLE file_runs (
    id         INTEGER PRIMARY KEY,
    started_at INTEGER NOT NULL,
    -- 0: organize; 1: rename one file.
    kind       INTEGER NOT NULL DEFAULT 0,
    pattern    TEXT,
    state      INTEGER NOT NULL DEFAULT 0 CHECK (state IN (0, 1, 2))
) STRICT;

CREATE TABLE file_moves (
    run_id    INTEGER NOT NULL REFERENCES file_runs ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    file_id   INTEGER,
    from_path BLOB NOT NULL,
    to_path   BLOB NOT NULL,
    state     INTEGER NOT NULL DEFAULT 0 CHECK (state IN (0, 1, 2, 3)),
    error     TEXT,
    PRIMARY KEY (run_id, position)
) STRICT, WITHOUT ROWID;

CREATE TABLE file_run_dirs (
    run_id INTEGER NOT NULL REFERENCES file_runs ON DELETE CASCADE,
    path   BLOB NOT NULL,
    PRIMARY KEY (run_id, path)
) STRICT, WITHOUT ROWID;

CREATE TABLE naming_patterns (
    id       INTEGER PRIMARY KEY,
    name     TEXT NOT NULL,
    pattern  TEXT NOT NULL,
    position INTEGER NOT NULL
) STRICT;

INSERT INTO naming_patterns (name, pattern, position) VALUES
    ('Artist, album, track', '{albumartist}/{album}/[{disc}-]{track} {title}', 0),
    ('With year and disc', '{albumartist}/{year} {album}/[{disc}-]{track} {title}', 1),
    ('Classical', '{composer}/{album}/[{disc}-]{track} {title}', 2),
    ('One folder for everything', '{artist} - {album} - {track} {title}', 3);
