-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Static playlists: an item refers to a track identity, not to a track row, so it
-- follows the track through moves and comes back with it after the track left the
-- library. An entry that names nothing in the library is kept as a dead item with what
-- the playlist file said about it, to show it and to relink it later.
--
-- - `id` names an item while its position changes.
-- - `position` counts from 0 without gaps; unique within a playlist.
-- - `identity_id` is the track's identity, or NULL for an entry that matched nothing.
-- - `ref_kind` says what `missing_ref` holds: 0 a path on this system (bytes), 1 a path
--   written for another system (a drive letter or a network share), 2 a URL, which is
--   never fetched.
-- - `missing_ref` is the path or URL the item was added or imported with, and `title`,
--   `artist`, `album` and `duration_ms` what was known about it then.
--
-- The table is rebuilt; the items of the previous form keep their tracks' identities.

CREATE TABLE playlist_items_new (
    id          INTEGER PRIMARY KEY,
    playlist_id INTEGER NOT NULL REFERENCES playlists ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    identity_id INTEGER REFERENCES identities,
    ref_kind    INTEGER NOT NULL DEFAULT 0,
    missing_ref BLOB,
    title       TEXT,
    artist      TEXT,
    album       TEXT,
    duration_ms INTEGER,
    UNIQUE (playlist_id, position)
) STRICT;

INSERT INTO playlist_items_new (playlist_id, position, identity_id, missing_ref)
    SELECT i.playlist_id, i.position, t.identity_id, i.missing_ref
    FROM playlist_items i LEFT JOIN tracks t ON t.id = i.track_id;

DROP TABLE playlist_items;
ALTER TABLE playlist_items_new RENAME TO playlist_items;
CREATE INDEX playlist_items_identity ON playlist_items (identity_id)
    WHERE identity_id IS NOT NULL;
