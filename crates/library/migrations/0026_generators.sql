-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Generator playlists: playlists defined by steps (a source, then filters, sorts,
-- caps and a limit) and filled by running them, again on open or when asked.
--
-- - `name` is unique, without regard to case.
-- - `steps` is the text form of the steps, one per line, as the generator module
--   reads and writes it.
-- - `on_open` is 1 when the playlist is generated again each time it is opened, 0
--   when it keeps its last result until asked.
-- - `tracks` is the last result: a JSON array of track ids, in order; NULL before
--   the first run. `generated_at` is when it was made.

CREATE TABLE generators (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE COLLATE NOCASE,
    steps        TEXT NOT NULL,
    on_open      INTEGER NOT NULL DEFAULT 1 CHECK (on_open IN (0, 1)),
    tracks       TEXT,
    generated_at INTEGER,
    created_at   INTEGER NOT NULL,
    modified_at  INTEGER NOT NULL
) STRICT;
