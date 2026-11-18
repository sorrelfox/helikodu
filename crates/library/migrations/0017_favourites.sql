-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Favourites: a built-in static playlist, first among the playlists, that the
-- heart on the deck adds the playing track to and takes it out of. It cannot be
-- renamed, moved, locked or deleted. `builtin` marks it; at most one row has it.
-- A static playlist named "Favourites" at the top, made by hand before, becomes
-- it, so that its tracks stay favourites; otherwise an empty one is made.

ALTER TABLE playlists ADD COLUMN builtin INTEGER NOT NULL DEFAULT 0;
CREATE UNIQUE INDEX playlists_builtin ON playlists (builtin) WHERE builtin != 0;

UPDATE playlists SET builtin = 1, locked = 0
WHERE id = (
    SELECT min(id) FROM playlists
    WHERE kind = 1 AND parent_id IS NULL AND source_path IS NULL
      AND name = 'Favourites' COLLATE NOCASE
);

INSERT INTO playlists (parent_id, name, kind, position, created_at, modified_at, builtin)
SELECT NULL, 'Favourites', 1, -1, CAST(strftime('%s', 'now') AS INTEGER),
       CAST(strftime('%s', 'now') AS INTEGER), 1
WHERE NOT EXISTS (SELECT 1 FROM playlists WHERE builtin != 0);
