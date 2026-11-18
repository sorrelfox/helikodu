-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Playlist folders, smart playlists, pins, undo history and playlist files found in
-- the library folders.
--
-- `playlists` already has the columns for folders (`kind` 0, `parent_id`) and smart
-- playlists (`kind` 2, `query` holding the printed query with its sort keys and
-- limit), and `locked`. New columns:
-- - `grouped` is 1 when the playlist is shown in groups by album.
-- - `source_path` is the playlist file (bytes) a playlist was imported from because
--   it was found in a library folder, and `source_mtime` the file's modification
--   time then, in nanoseconds; both NULL for every other playlist.
--
-- `pins` lists what the sidebar shows under "Pinned", in order: `kind` 0 an album,
-- 1 an artist (a person), 2 a playlist, with `target` its id.
--
-- `playlist_history` holds the states a playlist can go back to (`redo` 0) or forward
-- to again (`redo` 1): its name and query then, with its items in
-- `playlist_history_items`. The newest step of each kind is the one with the highest
-- id.

ALTER TABLE playlists ADD COLUMN grouped INTEGER NOT NULL DEFAULT 0;
ALTER TABLE playlists ADD COLUMN source_path BLOB;
ALTER TABLE playlists ADD COLUMN source_mtime INTEGER;
CREATE UNIQUE INDEX playlists_source ON playlists (source_path)
    WHERE source_path IS NOT NULL;

CREATE TABLE pins (
    id       INTEGER PRIMARY KEY,
    kind     INTEGER NOT NULL CHECK (kind IN (0, 1, 2)),
    target   INTEGER NOT NULL,
    position INTEGER NOT NULL,
    UNIQUE (kind, target)
) STRICT;

CREATE TRIGGER playlists_unpin AFTER DELETE ON playlists BEGIN
    DELETE FROM pins WHERE kind = 2 AND target = old.id;
END;

CREATE TABLE playlist_history (
    id          INTEGER PRIMARY KEY,
    playlist_id INTEGER NOT NULL REFERENCES playlists ON DELETE CASCADE,
    redo        INTEGER NOT NULL CHECK (redo IN (0, 1)),
    name        TEXT NOT NULL,
    query       TEXT,
    created_at  INTEGER NOT NULL
) STRICT;
CREATE INDEX playlist_history_playlist ON playlist_history (playlist_id, redo, id);

CREATE TABLE playlist_history_items (
    step_id     INTEGER NOT NULL REFERENCES playlist_history ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    identity_id INTEGER,
    ref_kind    INTEGER NOT NULL DEFAULT 0,
    missing_ref BLOB,
    title       TEXT,
    artist      TEXT,
    album       TEXT,
    duration_ms INTEGER,
    PRIMARY KEY (step_id, position)
) STRICT;
