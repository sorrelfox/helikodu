-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- The journal of the managed media folder: imports into it and consolidations of
-- library files into it, so that a run cut short (by a crash, a kill or the owner
-- stopping it) can be continued or rolled back from what is on the disk.
--
-- `managed_runs` holds one row per run. `operation` is 0 for an import, 1 for a
-- consolidation; `move_files` 1 when the sources go once their copies are in
-- place. `folder` is the managed folder; `bases` the folders and files given to an
-- import, separated by zero bytes, up to which a move tidies emptied folders.
-- `state` is 0 while the run works or after it was cut short, 1 when it is done,
-- 2 when it was rolled back.
--
-- `managed_ops` holds the files of a run in the order they are worked on. Every
-- row is written before its file is touched. `state`: 0 planned; 1 placed (the
-- copy is at `to_path`, checked, and the source is still to be removed); 2 done;
-- 3 skipped (`error` says why: an identical file was there, the source changed or
-- went, the disk refused); 4 rolled back. `dev` and `ino` name the file this row
-- placed (the temporary copy, whose identity the rename keeps, or the source
-- itself for a rename on one file system), so that recovery only ever touches
-- files the run made. `kept` is 1 when a moved file's source could not be removed
-- (a read-only source) and stays. A CUE sheet is written with its FILE lines
-- changed: `original` holds the sheet as it was, `content` as it is written.
-- `file_id` is the library file a consolidation moves, whose row follows it in the
-- transaction that marks the row done.

CREATE TABLE managed_runs (
    id         INTEGER PRIMARY KEY,
    started_at INTEGER NOT NULL,
    operation  INTEGER NOT NULL CHECK (operation IN (0, 1)),
    move_files INTEGER NOT NULL CHECK (move_files IN (0, 1)),
    folder     BLOB NOT NULL,
    bases      BLOB NOT NULL DEFAULT x'',
    state      INTEGER NOT NULL DEFAULT 0 CHECK (state IN (0, 1, 2))
) STRICT;

CREATE TABLE managed_ops (
    run_id    INTEGER NOT NULL REFERENCES managed_runs ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    kind      INTEGER NOT NULL,
    file_id   INTEGER,
    from_path BLOB NOT NULL,
    to_path   BLOB NOT NULL,
    move_file INTEGER NOT NULL CHECK (move_file IN (0, 1)),
    size      INTEGER NOT NULL,
    dev       INTEGER,
    ino       INTEGER,
    original  BLOB,
    content   BLOB,
    state     INTEGER NOT NULL DEFAULT 0 CHECK (state IN (0, 1, 2, 3, 4)),
    kept      INTEGER NOT NULL DEFAULT 0 CHECK (kept IN (0, 1)),
    error     TEXT,
    PRIMARY KEY (run_id, position)
) STRICT, WITHOUT ROWID;
