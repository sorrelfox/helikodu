-- SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Generator playlists in the sidebar: in playlist folders and among the pins.
--
-- - `generators.parent_id` is the playlist folder a generator is in, NULL at the
--   top. Deleting a folder moves what is in it up a level, as for playlists; the
--   foreign key only catches a folder removed some other way.
-- - `pins.kind` 3 is a generator, with `target` its id. The table is made again
--   because SQLite cannot change a CHECK constraint.

ALTER TABLE generators ADD COLUMN parent_id INTEGER REFERENCES playlists ON DELETE SET NULL;

CREATE TABLE pins_new (
    id       INTEGER PRIMARY KEY,
    kind     INTEGER NOT NULL CHECK (kind IN (0, 1, 2, 3)),
    target   INTEGER NOT NULL,
    position INTEGER NOT NULL,
    UNIQUE (kind, target)
) STRICT;
INSERT INTO pins_new (id, kind, target, position) SELECT id, kind, target, position FROM pins;
DROP TRIGGER playlists_unpin;
DROP TABLE pins;
ALTER TABLE pins_new RENAME TO pins;

CREATE TRIGGER playlists_unpin AFTER DELETE ON playlists BEGIN
    DELETE FROM pins WHERE kind = 2 AND target = old.id;
END;

CREATE TRIGGER generators_unpin AFTER DELETE ON generators BEGIN
    DELETE FROM pins WHERE kind = 3 AND target = old.id;
END;
